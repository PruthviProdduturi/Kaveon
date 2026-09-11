"""Machine-check PostgreSQL authority call sites against cutover ownership."""

import ast
import re
from pathlib import Path

from services.postgresql_retirement_gate import AUTHORITY_FAMILIES


# Paths are relative to api/. Access is intentionally conservative: a module
# with any mutation for a family is classified read-write for cutover planning.
DEPENDENCIES = {
    "database/pool.py": {"data_sources": "read"},
    "database/warmup.py": {"data_sources": "read"},
    "dlm/engine.py": {"catalog_sources": "read", "charts": "read", "dashboards": "read", "dlm_generation": "read-write"},
    "dlm/profiler.py": {"query_history": "read", "context_cache": "read-write"},
    "dlm/router.py": {"context_cache": "read-write"},
    "routers/catalog.py": {"datasets": "read", "charts": "read", "dashboards": "read", "dlm_generation": "read"},
    "routers/catalog_sources.py": {"catalog_sources": "read-write", "activity": "write"},
    "routers/chat.py": {"datasets": "read", "dataset_semantics": "read", "chat_history": "write"},
    "routers/chat_history.py": {"chat_history": "read-write"},
    "routers/data_sources.py": {"data_sources": "read-write", "favorites": "read-write"},
    "routers/lab.py": {"catalog_sources": "read", "data_sources": "read"},
    "routers/setup.py": {"datasets": "read-write"},
    "routers/sql.py": {"catalog_sources": "read"},
    "services/ai_service.py": {"ai_configuration": "read-write", "data_sources": "read", "datasets": "read", "dataset_semantics": "read"},
    "services/charts.py": {"charts": "read-write", "datasets": "read", "favorites": "read"},
    "services/chart_backfill.py": {"charts": "read"},
    "services/credentials.py": {"data_sources": "write"},
    "services/dashboards.py": {"dashboards": "read-write", "charts": "read", "favorites": "read"},
    "services/datasets.py": {"datasets": "read-write", "dataset_semantics": "read-write", "charts": "read", "favorites": "read"},
    "services/dlm_definition_backfill.py": {"datasets": "read", "dlm_generation": "read"},
    "services/dlm_run_backfill.py": {"datasets": "read", "dlm_generation": "read"},
    "services/favorites.py": {"favorites": "read-write", "charts": "read", "dashboards": "read", "datasets": "read"},
    "services/migrate_credentials.py": {"ai_configuration": "write"},
    "services/product_backfill.py": {"datasets": "read", "dataset_semantics": "read"},
    "services/query_history.py": {"query_history": "read-write"},
    "services/saved_queries.py": {"saved_queries": "read-write", "favorites": "read"},
    "services/theme.py": {"user_themes": "read-write"},
    "services/user_recents.py": {"user_recents": "read-write"},
}


def scan(api_root: Path, dependencies=None) -> dict:
    """Return inventory or fail when a table reference is not classified."""
    classifications = DEPENDENCIES if dependencies is None else dependencies
    table_family = {
        table.lower(): family
        for family, tables in AUTHORITY_FAMILIES.items()
        for table in tables
    }
    observed = {}
    for path in api_root.rglob("*.py"):
        if path.name.startswith("test_") or "__pycache__" in path.parts or "venv" in path.parts:
            continue
        relative = path.relative_to(api_root).as_posix()
        source = path.read_text(encoding="utf-8")
        tree = ast.parse(source, filename=str(path))
        named_sql = {}
        for node in ast.walk(tree):
            if isinstance(node, (ast.Assign, ast.AnnAssign)):
                target = node.targets[0] if isinstance(node, ast.Assign) and len(node.targets) == 1 else getattr(node, "target", None)
                value = node.value
                if isinstance(target, ast.Name) and isinstance(value, ast.Constant) and isinstance(value.value, str):
                    named_sql[target.id] = value.value
        sql_fragments = []
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Attribute):
                continue
            if node.func.attr not in {"query", "query_one", "execute"} or not node.args:
                continue
            argument = node.args[0]
            if isinstance(argument, ast.Constant) and isinstance(argument.value, str):
                sql_fragments.append(argument.value)
            elif isinstance(argument, ast.JoinedStr):
                sql_fragments.append("".join(
                    part.value for part in argument.values
                    if isinstance(part, ast.Constant) and isinstance(part.value, str)
                ))
            elif isinstance(argument, ast.Name) and argument.id in named_sql:
                sql_fragments.append(named_sql[argument.id])
        sql_source = "\n".join(sql_fragments)
        families = set()
        for table, family in table_family.items():
            if re.search(rf"(?<![A-Za-z0-9_])(?:dbo\.)?{re.escape(table)}(?![A-Za-z0-9_])", sql_source, re.IGNORECASE):
                families.add(family)
        if families:
            observed[relative] = families

    errors = []
    for path, families in observed.items():
        declared = set(classifications.get(path, {}))
        if missing := sorted(families - declared):
            errors.append(f"{path} has unclassified authority families: {', '.join(missing)}")
    for path, declared in classifications.items():
        if not (api_root / path).is_file():
            errors.append(f"{path} classification points to a missing file")
            continue
        for family, access in declared.items():
            if family not in AUTHORITY_FAMILIES or access not in {"read", "write", "read-write"}:
                errors.append(f"{path} has invalid classification {family}:{access}")
    if errors:
        raise RuntimeError("; ".join(sorted(errors)))

    families = []
    for family, tables in AUTHORITY_FAMILIES.items():
        call_sites = [
            {"path": path, "access": classifications[path][family]}
            for path in sorted(classifications)
            if family in classifications[path]
        ]
        if not call_sites:
            raise RuntimeError(f"{family} has no classified application call site")
        families.append({"family": family, "tables": list(tables), "call_sites": call_sites})
    return {"schema_version": 1, "family_count": len(families), "families": families}
