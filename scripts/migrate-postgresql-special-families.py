"""Publish a canonical seven-family PostgreSQL baseline to ADLS by ETag CAS."""

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services.postgresql_special_family_migration_cli import main  # noqa: E402

try: print(json.dumps(main(), sort_keys=True, separators=(",", ":")))
except Exception as error:
    print(json.dumps({"passed": False, "error": str(error)}, sort_keys=True,
                     separators=(",", ":")))
    raise SystemExit(1)
