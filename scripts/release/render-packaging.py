#!/usr/bin/env python3
"""Render the winget manifests and the Homebrew formula for a tagged CLI release.

Standard library only. The release workflow runs it after the archives are
built and hashed:

    python scripts/release/render-packaging.py --version 0.3.0 \
        --sums release/SHA256SUMS --out release/packaging

It reads the SHA256SUMS written next to the release assets, fills the
templates under packaging/winget/templates and packaging/homebrew/Formula,
and writes

    <out>/winget/manifests/p/PruthviProdduturi/Kaveon/<version>/
        PruthviProdduturi.Kaveon.yaml
        PruthviProdduturi.Kaveon.installer.yaml
        PruthviProdduturi.Kaveon.locale.en-US.yaml
    <out>/homebrew/Formula/kaveon.rb

Nothing is checked in from this output; the rendered files are release assets.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import re
import string
import sys
from pathlib import Path

REPOSITORY = "PruthviProdduturi/Kaveon"
PACKAGE_IDENTIFIER = "PruthviProdduturi.Kaveon"
SEMVER = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$")
SHA256 = re.compile(r"^[0-9a-fA-F]{64}$")

# Template variable -> release asset name (the version is substituted later).
ASSETS = {
    "SHA256_WINDOWS_X64": "kaveon-{version}-x86_64-pc-windows-msvc.zip",
    "SHA256_LINUX_X64": "kaveon-{version}-x86_64-unknown-linux-gnu.tar.gz",
    "SHA256_MACOS_ARM64": "kaveon-{version}-aarch64-apple-darwin.tar.gz",
    "SHA256_MACOS_X64": "kaveon-{version}-x86_64-apple-darwin.tar.gz",
}

WINGET_TEMPLATES = (
    f"{PACKAGE_IDENTIFIER}.yaml",
    f"{PACKAGE_IDENTIFIER}.installer.yaml",
    f"{PACKAGE_IDENTIFIER}.locale.en-US.yaml",
)


def parse_sums(path: Path) -> dict[str, str]:
    """Parse `sha256sum` output: one `<hex>  <name>` (or `<hex> *<name>`) per line."""
    sums: dict[str, str] = {}
    for number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(None, 1)
        if len(parts) != 2 or not SHA256.match(parts[0]):
            raise SystemExit(f"{path}:{number}: not a sha256sum line: {raw!r}")
        name = parts[1].lstrip("*").strip()
        if name in sums:
            raise SystemExit(f"{path}:{number}: duplicate entry for {name}")
        sums[name] = parts[0].lower()
    if not sums:
        raise SystemExit(f"{path}: no checksums found")
    return sums


def render(template_path: Path, values: dict[str, str]) -> str:
    template = string.Template(template_path.read_text(encoding="utf-8"))
    try:
        return template.substitute(values)
    except KeyError as error:
        raise SystemExit(f"{template_path}: unknown placeholder {error}") from None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--version", required=True, help="release version, e.g. 0.3.0 (no prefix)")
    parser.add_argument("--sums", required=True, type=Path, help="SHA256SUMS file for the release assets")
    parser.add_argument("--out", required=True, type=Path, help="output directory")
    parser.add_argument(
        "--release-date",
        default=_dt.datetime.now(_dt.timezone.utc).date().isoformat(),
        help="winget ReleaseDate (YYYY-MM-DD, default: today UTC)",
    )
    parser.add_argument("--repo", default=REPOSITORY, help="GitHub owner/name that hosts the release")
    parser.add_argument(
        "--templates",
        type=Path,
        default=Path(__file__).resolve().parents[2] / "packaging",
        help="packaging directory holding winget/templates and homebrew/Formula",
    )
    args = parser.parse_args(argv)

    version: str = args.version
    if version.startswith("cli-v"):
        version = version[len("cli-v"):]
    elif version.startswith("v"):
        version = version[1:]
    if not SEMVER.match(version):
        raise SystemExit(f"--version {args.version!r} is not a semantic version")
    if not re.match(r"^\d{4}-\d{2}-\d{2}$", args.release_date):
        raise SystemExit(f"--release-date {args.release_date!r} is not YYYY-MM-DD")

    sums = parse_sums(args.sums)
    values = {
        "VERSION": version,
        "RELEASE_DATE": args.release_date,
        "DOWNLOAD_BASE": f"https://github.com/{args.repo}/releases/download/cli-v{version}",
    }
    missing = []
    for variable, pattern in ASSETS.items():
        asset = pattern.format(version=version)
        if asset not in sums:
            missing.append(asset)
            continue
        # winget-pkgs convention is upper-case hex; Homebrew accepts either.
        values[variable] = sums[asset].upper() if variable == "SHA256_WINDOWS_X64" else sums[asset]
    if missing:
        raise SystemExit(f"{args.sums}: missing checksums for: {', '.join(missing)}")

    winget_dir = args.out / "winget" / "manifests" / "p" / "PruthviProdduturi" / "Kaveon" / version
    brew_dir = args.out / "homebrew" / "Formula"
    winget_dir.mkdir(parents=True, exist_ok=True)
    brew_dir.mkdir(parents=True, exist_ok=True)

    written: list[Path] = []
    for name in WINGET_TEMPLATES:
        target = winget_dir / name
        target.write_text(render(args.templates / "winget" / "templates" / name, values), encoding="utf-8", newline="\n")
        written.append(target)
    formula = brew_dir / "kaveon.rb"
    formula.write_text(render(args.templates / "homebrew" / "Formula" / "kaveon.rb", values), encoding="utf-8", newline="\n")
    written.append(formula)

    for target in written:
        print(target.as_posix())
    return 0


if __name__ == "__main__":
    sys.exit(main())
