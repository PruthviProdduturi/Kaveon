import json
from datetime import datetime, timezone

import pytest

from services import postgresql_baseline_operator as operator
from services import postgresql_baseline_identity as identity
from services import postgresql_baseline_cli as cli
from services import postgresql_operational_evidence as operational


class Cursor:
    def __init__(self, connection):
        self.connection = connection
        self.results = []

    def execute(self, statement, parameters=None):
        if statement.startswith("SET ") or statement.startswith("LOCK TABLE"):
            self.results = []
        elif "information_schema.columns" in statement:
            self.results = [(column["name"], column["type"],
                             "YES" if column["nullable"] else "NO", column["ordinal"])
                            for column in self.connection.schemas[parameters[0]]]
        elif "FROM pg_index" in statement:
            self.results = [(key,) for key in self.connection.keys[parameters[0]]]
        elif statement.startswith("SELECT COUNT(*)"):
            table = statement.split('"')[1]
            self.results = [(len(self.connection.rows[table]),)]
        elif statement.startswith("SELECT "):
            table = statement.split(' FROM "')[1].split('"')[0]
            names = [part.strip('"') for part in
                     statement[7:].split(' FROM ')[0].split(",")]
            self.results = [tuple(row[name] for name in names)
                            for row in self.connection.rows[table]][:parameters[0]]
        elif statement.startswith("INSERT INTO"):
            table = statement.split('"')[1]
            names = [item.strip('"') for item in
                     statement.split("(", 1)[1].split(")", 1)[0].split(",")]
            self.connection.rows[table].append(dict(zip(names, parameters)))
        else:
            raise AssertionError(statement)

    def fetchall(self): return list(self.results)
    def fetchone(self): return self.results[0]
    def close(self): pass


class Connection:
    def __init__(self, rows):
        self.schemas = {}
        self.keys = {}
        for table in operator.TABLES:
            names = (["dataset_id", "manifest"] if table == "dlm_artifact" else ["id"])
            self.schemas[table] = [
                {"name": name, "type": "text", "nullable": False, "ordinal": index}
                for index, name in enumerate(names, 1)]
            self.keys[table] = [names[0]]
        self.rows = {table: [dict(row) for row in rows.get(table, [])]
                     for table in operator.TABLES}
        self.autocommit = True
        self.commits = 0
        self.rollbacks = 0

    def cursor(self): return Cursor(self)
    def commit(self): self.commits += 1
    def rollback(self): self.rollbacks += 1


def source_rows():
    return {"dlm_artifact": [{"dataset_id": "17",
                              "manifest": '{"name":"Climate × Energy"}'}]}


def test_capture_restore_and_recapture_exact_seven_table_identity():
    source = Connection(source_rows())
    payload = operator.capture(source, "snapshot-17")
    assert payload["manifest"]["table_count"] == 7
    assert payload["manifest"]["row_count"] == 1
    assert source.commits == 1 and source.autocommit is True

    target = Connection({})
    result = operator.restore_and_qualify(target, payload)
    assert result["restore_verified"] is True
    assert result["source_inventory_sha256"] == payload["manifest"]["global_sha256"]
    assert target.rows["dlm_artifact"] == source_rows()["dlm_artifact"]
    assert target.commits == 1 and target.autocommit is True
    observations = {
        "postgresql_baseline_identity": identity.baseline_observation(payload),
        "baseline_restore_qualification": identity.restore_qualification_observation(
            payload, result, "isolated-db-1"),
        "exact_post_rollback_identity": identity.post_rollback_observation(payload, result),
    }
    for gate, observation in observations.items():
        receipt = operational.receipt_from_observation(gate, observation,
            checked_at="2026-09-14T23:00:00Z", evidence_id=f"{gate}-1")
        assert operational._validate_observation(gate, receipt["observation"],
            receipt["details"], max_rollback_seconds=900) is None
    assert observations["postgresql_baseline_identity"]["baseline_sha256"] == \
        identity.baseline_sha256(payload)


def test_restore_refuses_nonempty_target_and_rolls_back():
    payload = operator.capture(Connection(source_rows()), "snapshot-17")
    target = Connection({"dlm_router": [{"id": "occupied"}]})
    with pytest.raises(RuntimeError, match="not empty"):
        operator.restore_and_qualify(target, payload)
    assert target.rollbacks == 1 and target.commits == 0


def test_capture_rolls_back_when_global_row_bound_is_exceeded(monkeypatch):
    monkeypatch.setattr(operator.identity, "MAX_ROWS", 0)
    connection = Connection(source_rows())
    with pytest.raises(RuntimeError, match="row bound"):
        operator.capture(connection, "snapshot-17")
    assert connection.rollbacks == 1 and connection.autocommit is True


def test_capture_requires_dataset17_utf8_sentinel():
    connection = Connection({})
    with pytest.raises(RuntimeError, match="dataset 17"):
        operator.capture(connection, "snapshot-empty")
    assert connection.rollbacks == 1


def test_atomic_output_and_bounded_input(tmp_path):
    path = tmp_path / "nested" / "baseline.json"
    operator.write_atomic(path, {"name": "Climate × Energy"})
    assert json.loads(path.read_text(encoding="utf-8"))["name"] == "Climate × Energy"
    assert not list(path.parent.glob("*.tmp"))
    oversized = tmp_path / "oversized.json"
    oversized.write_text("x" * 20, encoding="utf-8")
    original = operator.MAX_FILE_BYTES
    try:
        operator.MAX_FILE_BYTES = 10
        with pytest.raises(RuntimeError, match="oversized"):
            operator.read_payload(oversized)
    finally:
        operator.MAX_FILE_BYTES = original


def test_capture_cli_writes_durable_gate_receipt(tmp_path, monkeypatch):
    connection = Connection(source_rows())
    class Wrapper:
        def __init__(self): self.connection = connection
        def connect(self): pass
    class Pool:
        db_type = "postgresql"
        def __init__(self): self.wrapper = Wrapper(); self.returned = False
        def get_connection(self): return self.wrapper
        def return_connection(self, wrapper): self.returned = wrapper is self.wrapper
    pool = Pool()
    monkeypatch.setenv("METADATA_DATABASE", "kaveonmeta")
    monkeypatch.setattr(cli, "get_connection_pool", lambda database: pool)
    baseline, receipt = tmp_path / "baseline.json", tmp_path / "receipt.json"
    assert cli.main(["capture", "--source-id", "snapshot-17", "--output", str(baseline),
                     "--receipt", str(receipt), "--evidence-id", "baseline-live-1",
                     "--checked-at", "2026-09-14T23:00:00Z"]) == 0
    loaded = operational.load_receipt(receipt, "postgresql_baseline_identity",
        now=datetime(2026, 9, 14, 23, 1, tzinfo=timezone.utc), max_age_hours=1,
        max_rollback_seconds=900)
    assert loaded["observation"]["baseline_sha256"] == \
        identity.baseline_sha256(operator.read_payload(baseline))
    assert pool.returned is True
