"""In-image operator CLI for the durable compiled-DLM backfill."""

import argparse
import json
import os
from pathlib import Path

from services import (adls_artifact_client, dlm_artifact_publisher,
                      dlm_run_backfill_operation, migration_checkpoint_store)


_APPLY_ENV = (
    "KAVEON_DLM_RUN_MIGRATION_ENABLED", "KAVEON_DLM_ARTIFACT_PUBLISH_ENABLED",
    "KAVEON_ADLS_ACCOUNT", "KAVEON_ADLS_CONTAINER",
    "KAVEON_MIGRATION_CHECKPOINT_ADLS_ACCOUNT",
    "KAVEON_MIGRATION_CHECKPOINT_ADLS_CONTAINER",
    "KAVEON_MIGRATION_CHECKPOINT_ADLS_PREFIX",
)


def _preflight(apply: bool) -> None:
    if not apply:
        return
    if os.getenv("KAVEON_MIGRATION_CHECKPOINT_MODE") != "adls":
        raise RuntimeError("DLM run apply requires durable ADLS checkpoint mode")
    missing = [name for name in _APPLY_ENV if not os.getenv(name)]
    if missing:
        raise RuntimeError("DLM run apply configuration is incomplete: " + ", ".join(missing))
    for name in _APPLY_ENV[:2]:
        if os.getenv(name) != "true":
            raise RuntimeError(f"{name} must be exactly true")


def run(checkpoint: Path, artifact_root: Path, *, apply: bool) -> dict:
    _preflight(apply)
    publisher = None
    if apply:
        publisher = dlm_artifact_publisher.Publisher(
            adls_artifact_client.AzureArtifactClient.from_env(), artifact_root,
        )
    return migration_checkpoint_store.run(
        dlm_run_backfill_operation, checkpoint, apply=apply,
        invoke=lambda hydrated: dlm_run_backfill_operation.run(
            hydrated, artifact_root, apply=apply, resume=hydrated.exists(),
            publisher=publisher,
        ),
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--checkpoint", required=True, type=Path)
    parser.add_argument("--artifact-root", required=True, type=Path)
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    try:
        print(json.dumps(run(args.checkpoint, args.artifact_root, apply=args.apply),
                         sort_keys=True, separators=(",", ":")))
        return 0
    except Exception as error:
        print(json.dumps({"passed": False, "error": str(error)},
                         sort_keys=True, separators=(",", ":")))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
