"""Extract the original dashboard seed scripts without contacting a database.

The scripts are executed only with in-process stand-ins for psycopg2, requests,
and database.pool.  The stand-ins record INSERT payloads and discard every SQL
statement, including the original scripts' DELETE and CREATE statements.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import re
import runpy
import subprocess
import sys
import types
import uuid
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
SOURCES = {
    "product": ROOT / "data/kaveon-usage/create_dashboards.py",
    "climate_energy": ROOT / "data/climate-energy/create_3_dashboards.py",
    "demo": ROOT / "demo/build_dashboards.py",
}

# Legacy numeric IDs are source-script conventions, never import targets.  The
# mappings come from the adjacent dataset-registration scripts and physical
# table declarations; importers resolve the logical refs against the target API.
DECLARED_DATASETS = {
    "product": {
        142: {"logical_ref": "product/kaveon-product-analytics", "database_name": "kaveon", "schema_name": "public", "table_name": "kaveon_product_analytics"},
    },
    "climate_energy": {
        132: {"logical_ref": "climate-energy/energy-annual", "database_name": "kaveon", "schema_name": "climate_energy", "table_name": "energy_annual"},
        133: {"logical_ref": "climate-energy/temperature-monthly", "database_name": "kaveon", "schema_name": "climate_energy", "table_name": "temperature_monthly"},
        134: {"logical_ref": "climate-energy/climate-x-energy", "database_name": "kaveon", "schema_name": "climate_energy", "table_name": "climate_x_energy"},
    },
}


def _json(value: Any) -> Any:
    if isinstance(value, str):
        try:
            return json.loads(value)
        except json.JSONDecodeError:
            return value
    return value


def _slug(value: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-") or "chart"


def _normalize_type(chart_type: str, viz: dict[str, Any]) -> str:
    options = viz.get("chartTypeOptions") or {}
    if chart_type == "bar" and options.get("horizontal"):
        return "bar_horizontal"
    return {
        "bar": "bar_vertical",
        "line": "line_multi_series",
        "stacked_bar": "stacked_bar_vertical",
    }.get(chart_type, chart_type)


class Recorder:
    def __init__(self, family: str) -> None:
        self.family = family
        self.next_id = 1
        self.charts: list[dict[str, Any]] = []
        self.dashboards: list[dict[str, Any]] = []
        self.datasets: dict[int, dict[str, Any]] = dict(DECLARED_DATASETS.get(family, {}))

    def dataset_ref(self, dataset_id: Any) -> str | None:
        try:
            item = self.datasets.get(int(dataset_id))
        except (TypeError, ValueError):
            item = None
        return item.get("logical_ref") if item else None

    def dataset(self, params: list[Any]) -> int:
        dataset_id = self.next_id
        self.next_id += 1
        name, table = str(params[0]), str(params[2])
        self.datasets[dataset_id] = {
            "logical_ref": f"{self.family}/{_slug(name)}",
            "source_id": dataset_id,
            "name": name,
            "database_name": "neondb",
            "schema_name": "public",
            "table_name": table,
            "date_column": params[3] if len(params) > 3 else None,
            "columns": [],
        }
        return dataset_id

    def dataset_column(self, params: list[Any]) -> None:
        try:
            dataset = self.datasets[int(params[0])]
        except (KeyError, TypeError, ValueError):
            return
        dataset["columns"].append({
            "table_name": params[1], "column_name": params[2], "data_type": params[3],
            "is_dimension": bool(params[4]), "is_metric": bool(params[5]),
            "semantic_type": params[6] if len(params) > 6 else None,
        })

    def chart(self, params: list[Any]) -> int:
        # The three source helpers all begin their chart INSERT params with
        # (name, [description,] chart_type, query_config, viz_config, ...).
        name = str(params[0])
        chart_type_index = 2 if (
            len(params) > 2
            and isinstance(params[2], str)
            and not params[2].lstrip().startswith("{")
        ) else 1
        chart_type = str(params[chart_type_index])
        query_config = _json(params[chart_type_index + 1]) or {}
        viz_config = _json(params[chart_type_index + 2]) or {}
        if "dataset_id" in query_config:
            dataset_ref = self.dataset_ref(query_config["dataset_id"])
            if dataset_ref:
                query_config["dataset_ref"] = dataset_ref
        occurrence = sum(item["name"] == name for item in self.charts) + 1
        chart_id = self.next_id
        self.next_id += 1
        self.charts.append({
            "logical_ref": f"{self.family}/{_slug(name)}-{occurrence}",
            "source_id": chart_id,
            "name": name,
            "description": params[1] if chart_type_index == 2 else None,
            "source_chart_type": chart_type,
            "chart_type": _normalize_type(chart_type, viz_config),
            "query_config": query_config,
            "viz_config": viz_config,
        })
        return chart_id

    def dashboard(self, params: list[Any]) -> None:
        # All original helpers put ID then name then description/slug before
        # layout, charts, and optionally filters. Locate those JSON payloads by
        # shape instead of executing any source-owned database logic.
        decoded = [_json(value) for value in params]
        name = str(params[1])
        layout = next((value for value in decoded if isinstance(value, list) and value and isinstance(value[0], dict) and "type" in value[0]), [])
        chart_ids = next((value for value in decoded if isinstance(value, list) and value and all(isinstance(item, int) for item in value)), [])
        filters = next((value for value in decoded if isinstance(value, list) and value and isinstance(value[0], dict) and "appliesTo" in value[0]), [])
        refs = {item["source_id"]: item["logical_ref"] for item in self.charts}
        item_number = 0
        def remap(item: Any) -> Any:
            nonlocal item_number
            if isinstance(item, list):
                return [remap(value) for value in item]
            if isinstance(item, dict):
                copied = {key: remap(value) for key, value in item.items()}
                if "i" in copied:
                    item_number += 1
                    copied["i"] = f"item-{item_number}"
                if copied.get("type") == "chart" and copied.get("chartId") in refs:
                    copied["chart_ref"] = refs[copied.pop("chartId")]
                if "datasetId" in copied:
                    dataset_ref = self.dataset_ref(copied["datasetId"])
                    if dataset_ref:
                        copied["dataset_ref"] = dataset_ref
                return copied
            return item
        self.dashboards.append({
            "logical_ref": f"{self.family}/{_slug(name)}",
            "name": name,
            "description": str(params[3] if self.family == "demo" else params[2]),
            "layout": remap(layout),
            "chart_refs": [refs.get(chart_id, f"{self.family}/unknown-{chart_id}") for chart_id in chart_ids],
            "filters": remap(filters),
        })


class FakeCursor:
    def __init__(self, recorder: Recorder) -> None:
        self.recorder = recorder
        self.last_id: int | None = None

    def execute(self, sql: str, params: list[Any] | tuple[Any, ...] | None = None) -> None:
        params = list(params or [])
        text = " ".join(sql.lower().split())
        if "insert into charts" in text:
            self.last_id = self.recorder.chart(params)
        elif "insert into datasets" in text:
            self.last_id = self.recorder.dataset(params)
        elif "insert into dataset_columns" in text:
            self.recorder.dataset_column(params)
        elif "insert into dashboards" in text:
            self.recorder.dashboard(params)

    def fetchone(self) -> tuple[int] | None:
        return (self.last_id,) if self.last_id is not None else (1,)

    def fetchall(self) -> list[tuple[Any, ...]]:
        return []

    def close(self) -> None:
        pass


class FakeConnection:
    def __init__(self, recorder: Recorder) -> None:
        self.cursor_obj = FakeCursor(recorder)
        self.autocommit = False

    def cursor(self) -> FakeCursor:
        return self.cursor_obj

    def close(self) -> None:
        pass


def _fake_modules(recorder: Recorder) -> dict[str, types.ModuleType]:
    psycopg2 = types.ModuleType("psycopg2")
    psycopg2.connect = lambda *args, **kwargs: FakeConnection(recorder)  # type: ignore[attr-defined]
    requests = types.ModuleType("requests")
    requests.post = lambda *args, **kwargs: types.SimpleNamespace(ok=True, status_code=200, text="", json=lambda: {"rows": []})  # type: ignore[attr-defined]
    pool = types.ModuleType("database.pool")

    def execute_query(sql: str, database: str, params: list[Any] | None = None) -> dict[str, Any]:
        values = list(params or [])
        text = " ".join(sql.lower().split())
        if "insert into charts" in text:
            return {"rows": [[recorder.chart(values)]]}
        if "insert into datasets" in text:
            return {"rows": [[recorder.dataset(values)]]}
        if "insert into dataset_columns" in text:
            recorder.dataset_column(values)
        if "insert into dashboards" in text:
            recorder.dashboard(values)
        return {"rows": [[1]]}

    pool.execute_query = execute_query  # type: ignore[attr-defined]
    database = types.ModuleType("database")
    database.pool = pool  # type: ignore[attr-defined]
    return {"psycopg2": psycopg2, "requests": requests, "database": database, "database.pool": pool}


def extract(family: str, source: Path) -> dict[str, Any]:
    recorder = Recorder(family)
    original_modules = {name: sys.modules.get(name) for name in ("psycopg2", "requests", "database", "database.pool")}
    original_env = dict(os.environ)
    original_uuid4 = uuid.uuid4
    uuid_counter = 0
    def deterministic_uuid4() -> Any:
        nonlocal uuid_counter
        uuid_counter += 1
        return types.SimpleNamespace(hex=f"{uuid_counter:032x}")
    os.environ.update({"NEON_URL": "offline://fixture", "PGUSER": "offline", "PGPASSWORD": "offline"})
    uuid.uuid4 = deterministic_uuid4  # type: ignore[assignment]
    sys.modules.update(_fake_modules(recorder))
    try:
        with contextlib.redirect_stdout(sys.stderr):
            namespace = runpy.run_path(str(source), run_name=f"offline_extract_{family}")
            main = namespace.get("main")
            if callable(main):
                main()
    finally:
        os.environ.clear(); os.environ.update(original_env)
        uuid.uuid4 = original_uuid4
        for name, module in original_modules.items():
            if module is None:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = module
    return {"datasets": list(recorder.datasets.values()), "charts": recorder.charts, "dashboards": recorder.dashboards}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=ROOT / "data/dashboard-templates/original-dashboard-templates.json")
    args = parser.parse_args()
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    sources = {
        family: {"path": str(path.relative_to(ROOT)).replace("\\", "/"), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        for family, path in SOURCES.items()
    }
    templates = {family: extract(family, path) for family, path in SOURCES.items()}
    manifest = {
        "format": "kaveon.dashboard-template/v1",
        "provenance": {"repository_commit": commit, "sources": sources, "extraction": "offline stand-ins; no database or HTTP calls"},
        "templates": templates,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"Wrote {args.output}: {len(templates['product']['charts'])} product charts, {len(templates['climate_energy']['charts'])} climate charts, {len(templates['demo']['charts'])} demo charts")


if __name__ == "__main__":
    main()
