from services import postgresql_source_state_probe as probe


class Transaction:
    def __init__(self, row):
        self.row = row
        self.statements = []

    def execute(self, sql):
        self.statements.append(sql)

    def query_one(self, sql):
        self.statements.append(sql)
        return self.row


class Context:
    def __init__(self, transaction):
        self.transaction = transaction

    def __enter__(self):
        return self.transaction

    def __exit__(self, *_args):
        pass


def test_collects_one_repeatable_read_bounded_source_observation(monkeypatch):
    transaction = Transaction({"query_id": 19, "source_snapshot": "19:19:",
                               "watermark": 42, "pending_events": 0})
    monkeypatch.setattr(probe.db, "transaction", lambda: Context(transaction))
    first = probe.collect()
    second = probe.collect()
    assert first == second
    assert first["query_id"] == 19
    assert first["watermark"] == 42
    assert first["pending_events"] == 0
    assert len(first["source_snapshot"]) == 64
    assert "REPEATABLE READ, READ ONLY" in transaction.statements[0]


def test_invalid_source_state_fails_closed(monkeypatch):
    transaction = Transaction({"query_id": 0, "source_snapshot": "", "watermark": -1,
                               "pending_events": -1})
    monkeypatch.setattr(probe.db, "transaction", lambda: Context(transaction))
    try:
        probe.collect()
        assert False, "invalid observation must fail"
    except RuntimeError as error:
        assert "invalid values" in str(error)
