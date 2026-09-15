"""Capture or restore-qualify the seven-table PostgreSQL rollback baseline."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services.postgresql_baseline_cli import main  # noqa: E402

raise SystemExit(main())
