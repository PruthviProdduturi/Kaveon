"""Record whether the resource-matched Kaveon/Trino benchmark can run now."""

import argparse
from datetime import datetime, timezone
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess


EXPECTED_CPUS = 4_000_000_000
EXPECTED_MEMORY = 8 * 1024**3
PYTHON_MODULES = ("duckdb", "pyarrow", "requests", "psutil", "trino")


def command_result(command, timeout=30, output_limit=4000):
    try:
        completed = subprocess.run(
            command,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
        stdout = completed.stdout.replace("\0", "").strip()
        stderr = completed.stderr.replace("\0", "").strip()
        if output_limit is not None:
            stdout = stdout[-output_limit:]
            stderr = stderr[-output_limit:]
        return {
            "returncode": completed.returncode,
            "stdout": stdout,
            "stderr": stderr,
        }
    except (OSError, subprocess.SubprocessError) as error:
        return {"returncode": None, "stdout": "", "stderr": f"{type(error).__name__}: {error}"}


def classify_docker_failure(*messages):
    combined = "\n".join(message for message in messages if message)
    if "DefenderforEndpointPlug-in" in combined and "E_ABORT" in combined:
        return {
            "code": "windows_wsl_defender_plugin_failure",
            "detail": "Microsoft Defender for Endpoint's WSL plug-in aborted VM creation.",
            "remediation": "Repair the managed Defender/WSL integration or run the benchmark on another 4-CPU, 8-GiB Docker host; do not weaken subscription or endpoint security policy.",
        }
    if "dockerDesktopLinuxEngine" in combined or "daemon" in combined.lower():
        return {
            "code": "docker_engine_unavailable",
            "detail": "The Docker client cannot reach a Linux container engine.",
            "remediation": "Start or repair Docker Desktop, then rerun this preflight.",
        }
    return {
        "code": "docker_engine_check_failed",
        "detail": "The Docker engine readiness check failed.",
        "remediation": "Inspect the recorded Docker error, restore the engine, and rerun this preflight.",
    }


def required_checks_pass(checks):
    return bool(checks) and all(value is True for value in checks.values())


def inspect_container(name):
    result = command_result(["docker", "inspect", name], output_limit=None)
    if result["returncode"] != 0:
        return None, result
    try:
        payload = json.loads(result["stdout"])
        container = payload[0]
        return container, {
            "returncode": result["returncode"],
            "stdout": container.get("Id", ""),
            "stderr": result["stderr"],
        }
    except (json.JSONDecodeError, IndexError, TypeError) as error:
        result["stderr"] = f"invalid docker inspect output: {error}"
        return None, result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--docker-image", default="kaveon-engine:qualification")
    parser.add_argument("--trino-container", default="kaveon-qualification-trino-1")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    dependencies_present = all(importlib.util.find_spec(name) is not None for name in PYTHON_MODULES)
    host_memory = None
    if importlib.util.find_spec("psutil") is not None:
        import psutil

        host_memory = psutil.virtual_memory().total
    checks = {
        "python_dependencies": dependencies_present,
        "host_logical_cpus_at_least_4": (os.cpu_count() or 0) >= 4,
        "host_memory_at_least_8_gib": host_memory is not None and host_memory >= EXPECTED_MEMORY,
        "docker_cli": shutil.which("docker") is not None,
        "docker_engine": False,
        "kaveon_image_present": None,
        "trino_running": None,
        "trino_4_cpu_8_gib": None,
        "trino_loopback_port_18080": None,
    }
    diagnostics = {}
    docker_failure = None

    if checks["docker_cli"]:
        engine = command_result(["docker", "info", "--format", "{{json .ServerVersion}}"])
        diagnostics["docker_info"] = engine
        checks["docker_engine"] = engine["returncode"] == 0 and bool(engine["stdout"])
        if not checks["docker_engine"]:
            wsl = None
            if os.name == "nt" and shutil.which("wsl.exe"):
                wsl = command_result(["wsl.exe", "-d", "docker-desktop", "-u", "root", "-e", "/bin/true"])
                diagnostics["docker_desktop_wsl"] = wsl
            docker_failure = classify_docker_failure(
                engine.get("stdout", ""),
                engine.get("stderr", ""),
                (wsl or {}).get("stdout", ""),
                (wsl or {}).get("stderr", ""),
            )

    if checks["docker_engine"]:
        image = command_result(["docker", "image", "inspect", args.docker_image, "--format", "{{.Id}}"])
        diagnostics["kaveon_image"] = image
        checks["kaveon_image_present"] = image["returncode"] == 0 and image["stdout"].startswith("sha256:")

        trino, trino_result = inspect_container(args.trino_container)
        diagnostics["trino_container"] = trino_result
        if trino:
            host = trino.get("HostConfig") or {}
            state = trino.get("State") or {}
            bindings = ((trino.get("NetworkSettings") or {}).get("Ports") or {}).get("8080/tcp") or []
            checks["trino_running"] = state.get("Running") is True
            checks["trino_4_cpu_8_gib"] = host.get("NanoCpus") == EXPECTED_CPUS and host.get("Memory") == EXPECTED_MEMORY
            checks["trino_loopback_port_18080"] = any(
                binding.get("HostIp") in {"127.0.0.1", "::1"} and binding.get("HostPort") == "18080"
                for binding in bindings
            )

    report = {
        "schema_version": 1,
        "checked_at": datetime.now(timezone.utc).isoformat(),
        "benchmark": "resource-matched single-node Kaveon versus Trino",
        "required_resources": {"cpus": 4, "memory_bytes": EXPECTED_MEMORY},
        "docker_image": args.docker_image,
        "trino_container": args.trino_container,
        "checks": checks,
        "ready": required_checks_pass(checks),
        "blocked_by": [name for name, passed in checks.items() if passed is not True],
        "diagnosis": docker_failure,
        "diagnostics": diagnostics,
        "claim_evidence": False,
        "limitation": "This is readiness evidence only. It is never evidence of a Kaveon performance ratio.",
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"ready={str(report['ready']).lower()}; blocked_by={','.join(report['blocked_by']) or 'none'}")
    if docker_failure:
        print(f"diagnosis={docker_failure['code']}: {docker_failure['detail']}")
    return 0 if report["ready"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
