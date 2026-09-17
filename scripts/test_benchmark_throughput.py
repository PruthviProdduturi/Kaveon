import importlib.util
import json
import threading
from pathlib import Path

import pytest

SCRIPT = Path(__file__).with_name("benchmark-throughput.py")
SPEC = importlib.util.spec_from_file_location("benchmark_throughput", SCRIPT)
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)

REPORT = Path(__file__).with_name("benchmark-rounds-report.py")
REPORT_SPEC = importlib.util.spec_from_file_location("benchmark_rounds_report", REPORT)
report = importlib.util.module_from_spec(REPORT_SPEC)
REPORT_SPEC.loader.exec_module(report)

STATEMENTS = [
    {"id": "q01", "sql": "SELECT COUNT(*) FROM hits"},
    {"id": "q02", "sql": "SELECT a, COUNT(*) FROM hits GROUP BY a", "kaveon_sql": "SELECT a, COUNT(*) FROM hits GROUP BY 1",
     "trino_sql": "SELECT a, count(*) FROM hits GROUP BY a"},
    {"id": "q03", "sql": "SELECT a FROM hits ORDER BY a LIMIT 3"},
    {"id": "q04", "sql": "SELECT b FROM hits"},
]
IDS = [s["id"] for s in STATEMENTS]


class FakeClock:
    def __init__(self):
        self.now = 0.0
        self.slept = []

    def __call__(self):
        return self.now

    def sleep(self, seconds):
        self.slept.append(seconds)
        self.now += seconds


class Executor:
    """A fake engine: results by statement id, scripted exceptions, a call log."""

    def __init__(self, clock, results, seconds=1.0):
        self.clock = clock
        self.results = results
        self.seconds = seconds
        self.calls = []
        self.scripted = {}
        self.lock = threading.Lock()

    def __call__(self, sql, client_index):
        sid = next(s["id"] for s in STATEMENTS if sql in (s["sql"], s.get("kaveon_sql"), s.get("trino_sql")))
        with self.lock:
            self.calls.append((client_index, sid))
            self.clock.now += self.seconds
            queue = self.scripted.get(sid)
            if queue:
                outcome = queue.pop(0)
                if isinstance(outcome, BaseException):
                    raise outcome
                return outcome
        return self.results[sid]


RESULTS = {"q01": [[100]], "q02": [["x", 3], ["y", 7]], "q03": [["a"], ["b"], ["c"]], "q04": [[1.5], [2]]}


def rounds_stop(rounds):
    return lambda passes: passes >= rounds


def test_permutation_is_fixed_per_client_and_a_permutation():
    for index in range(8):
        order = module.permutation(IDS, index)
        assert sorted(order) == sorted(IDS)
        assert order == module.permutation(IDS, index)
    assert module.permutation(IDS, 0) != module.permutation(IDS, 1) or module.permutation(IDS, 0) != module.permutation(IDS, 2)


def test_each_client_runs_its_permutation_the_requested_rounds():
    clock = FakeClock()
    executor = Executor(clock, RESULTS)
    ledger = module.Ledger(IDS, clients=3)
    for index in range(3):
        passes = module.run_client(index, STATEMENTS, "kaveon", executor, ledger, rounds_stop(2), "measured",
                                   clock=clock, sleep=clock.sleep)
        assert passes == 2
        mine = [sid for client, sid in executor.calls if client == index]
        assert mine == module.permutation(IDS, index) * 2
    assert all(c["executions"] == 8 for c in ledger.clients)
    assert all(entry["executions"] == 6 for entry in ledger.statements.values())


def test_dialect_column_is_used_per_engine():
    clock = FakeClock()
    seen = []

    def execute(sql, client_index):
        seen.append(sql)
        return RESULTS["q02"]

    statements = [STATEMENTS[1]]
    module.run_client(0, statements, "kaveon", execute, module.Ledger(["q02"], 1), rounds_stop(1), "measured",
                      clock=clock, sleep=clock.sleep)
    module.run_client(0, statements, "trino", execute, module.Ledger(["q02"], 1), rounds_stop(1), "measured",
                      clock=clock, sleep=clock.sleep)
    assert seen == [STATEMENTS[1]["kaveon_sql"], STATEMENTS[1]["trino_sql"]]


def test_duration_bound_stops_between_statements():
    clock = FakeClock()
    executor = Executor(clock, RESULTS, seconds=1.0)
    ledger = module.Ledger(IDS, clients=1)
    deadline = 5.5
    module.run_client(0, STATEMENTS, "kaveon", executor, ledger, lambda passes: clock() >= deadline, "measured",
                      clock=clock, sleep=clock.sleep)
    # Six executions of one second each start before 5.5 s; the seventh does not.
    assert len(executor.calls) == 6
    assert ledger.clients[0]["executions"] == 6


def test_digest_mismatch_is_a_failed_execution_against_the_first_seen_digest():
    clock = FakeClock()
    executor = Executor(clock, RESULTS)
    executor.scripted["q02"] = [RESULTS["q02"], [["x", 3], ["y", 8]]]
    ledger = module.Ledger(IDS, clients=1)
    module.run_client(0, STATEMENTS, "kaveon", executor, ledger, rounds_stop(3), "measured",
                      clock=clock, sleep=clock.sleep)
    q02 = ledger.statements["q02"]
    assert (q02["executions"], q02["failures"]) == (2, 1)
    assert q02["errors"][0]["kind"] == "mismatch"
    assert ledger.reference["q02"] == module.result_hash(RESULTS["q02"], False)
    assert ledger.clients[0] == {"executions": 11, "failures": 1, "rejections": 0}


def test_unordered_results_digest_the_same_in_any_row_order_and_ordered_ones_do_not():
    rows = [["x", 3], ["y", 7]]
    assert module.result_hash(rows, False) == module.result_hash(list(reversed(rows)), False)
    assert module.result_hash(rows, True) != module.result_hash(list(reversed(rows)), True)
    assert module.result_hash([[2.0], [1.5]], False) == module.result_hash([[2], [1.5]], False)


def test_rejections_are_retried_with_backoff_and_counted_apart():
    clock = FakeClock()
    executor = Executor(clock, RESULTS)
    executor.scripted["q01"] = [module.Rejected("MEMORY_ADMISSION_REJECTED"), module.Rejected("again"), RESULTS["q01"]]
    ledger = module.Ledger(IDS, clients=1)
    module.run_client(0, STATEMENTS, "kaveon", executor, ledger, rounds_stop(1), "measured",
                      clock=clock, sleep=clock.sleep)
    assert ledger.statements["q01"] ["rejections"] == 2
    assert ledger.statements["q01"]["executions"] == 1
    assert ledger.statements["q01"]["failures"] == 0
    assert clock.slept == [module.BACKOFF_SECONDS[0], module.BACKOFF_SECONDS[1]]
    assert ledger.clients[0]["rejections"] == 2


def test_rejection_retry_stops_at_the_deadline():
    clock = FakeClock()
    executor = Executor(clock, RESULTS, seconds=0.0)
    executor.scripted["q01"] = [module.Rejected("full")] * 50
    order = module.permutation(IDS, 0)
    ledger = module.Ledger(IDS, clients=1)
    statements = [STATEMENTS[0]]
    module.run_client(0, statements, "kaveon", executor, ledger, lambda passes: clock() >= 12.0, "measured",
                      clock=clock, sleep=clock.sleep)
    assert ledger.statements["q01"]["executions"] == 0
    assert 0 < ledger.statements["q01"]["rejections"] < 50
    assert order  # the permutation is unaffected by the retry loop


def test_errors_fail_the_execution_and_a_statement_failing_everywhere_is_listed():
    clock = FakeClock()
    executor = Executor(clock, RESULTS)
    executor.scripted["q04"] = [RuntimeError("Engine query failed")] * 10
    ledger = module.Ledger(IDS, clients=2)
    for index in range(2):
        module.run_client(index, STATEMENTS, "kaveon", executor, ledger, rounds_stop(2), "measured",
                          clock=clock, sleep=clock.sleep)
    record = module.summarise(ledger, 2, elapsed=32.0, config={"engine": "kaveon"})
    assert record["failed_everywhere"] == ["q04"]
    assert record["failures"] == 4
    assert record["successful"] == 12
    assert record["executions_per_second"] == pytest.approx(12 / 32.0)
    assert record["per_client"] == [{"executions": 6, "failures": 2, "rejections": 0}] * 2
    q04 = next(s for s in record["statements"] if s["id"] == "q04")
    assert q04["p50_seconds"] is None and q04["executions"] == 0 and len(q04["errors"]) == 3


def test_warmup_establishes_the_reference_but_is_not_counted():
    clock = FakeClock()
    executor = Executor(clock, RESULTS)
    ledger = module.Ledger(IDS, clients=1)
    module.run_client(0, STATEMENTS, "kaveon", executor, ledger, rounds_stop(1), "warmup", clock=clock, sleep=clock.sleep)
    assert ledger.warmup == {"executions": 4, "failures": 0, "rejections": 0}
    assert ledger.clients[0]["executions"] == 0
    assert set(ledger.reference) == set(IDS)
    executor.scripted["q01"] = [[[101]]]
    module.run_client(0, STATEMENTS, "kaveon", executor, ledger, rounds_stop(1), "measured", clock=clock, sleep=clock.sleep)
    assert ledger.statements["q01"]["failures"] == 1
    record = module.summarise(ledger, 1, elapsed=4.0, config={})
    assert record["warmup"]["executions"] == 4
    assert record["successful"] == 3


def test_summary_percentiles_are_nearest_rank():
    assert module.percentile([5, 1, 4, 2, 3], 0.5) == 3
    assert module.percentile([5, 1, 4, 2, 3], 0.95) == 5
    assert module.percentile(list(range(1, 101)), 0.95) == 95
    assert module.percentile([7], 0.95) == 7
    clock = FakeClock()
    ledger = module.Ledger(["q01"], 1)
    for seconds in (0.2, 0.4, 0.6, 0.8, 1.0):
        ledger.completed("q01", 0, "measured", seconds, RESULTS["q01"], False)
    summary = module.summarise(ledger, 1, elapsed=3.0, config={})["statements"][0]
    assert (summary["p50_seconds"], summary["p95_seconds"], summary["max_seconds"]) == (0.6, 1.0, 1.0)
    assert summary["rows"] == 1 and summary["sample"] == [["100"]]
    assert clock.now == 0.0


def test_concurrent_clients_share_one_ledger_safely():
    clock = FakeClock()
    executor = Executor(clock, RESULTS, seconds=0.0)
    ledger = module.Ledger(IDS, clients=8)
    threads = [threading.Thread(target=module.run_client,
                                args=(i, STATEMENTS, "kaveon", executor, ledger, rounds_stop(25), "measured"),
                                kwargs={"clock": clock, "sleep": clock.sleep}) for i in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert ledger.totals() == {"executions": 8 * 25 * 4, "failures": 0, "rejections": 0}
    assert len(ledger.reference) == 4


def test_trino_next_uri_stays_on_the_configured_origin():
    base = "https://trino.kaveon.svc.cluster.local:8443"
    good = base + "/v1/statement/executing/abc/1?slug=x"
    assert module.validated_next_uri(base, good) == good
    for value in ("https://evil.example/v1/statement/abc", base + "/ui/", base + "/v1/statement/abc#fragment",
                  "https://user:pass@trino.kaveon.svc.cluster.local:8443/v1/statement/abc"):
        with pytest.raises(RuntimeError, match="unsafe next URI"):
            module.validated_next_uri(base, value)


def test_rounds_report_reads_throughput_records(tmp_path, capsys):
    def record(engine, rate, failures, rejections, failed, digest):
        return {"engine": engine, "executions_per_second": rate, "failures": failures, "rejections": rejections,
                "failed_everywhere": failed,
                "statements": [{"id": "q01", "result_sha256": digest}, {"id": "q02", "result_sha256": "same"}]}
    for n, rate in enumerate((2.0, 2.4, 2.2), start=1):
        (tmp_path / f"kaveon-throughput-4-round{n}.json").write_text(
            json.dumps(record("kaveon", rate, 1, 3, ["q33"], "k")), encoding="utf-8")
        (tmp_path / f"trino-throughput-4-round{n}.json").write_text(
            json.dumps(record("trino", rate / 2, 0, 0, [], "t")), encoding="utf-8")
    summary = report.report_throughput(tmp_path)
    out = capsys.readouterr().out
    assert summary["4"]["kaveon"]["executions_per_second"] == 2.2
    assert summary["4"]["trino"]["round_rates"] == [1.0, 1.2, 1.1]
    assert (summary["4"]["kaveon"]["failures"], summary["4"]["kaveon"]["rejections"]) == (3, 9)
    assert summary["4"]["kaveon"]["failed_everywhere"] == ["q33"]
    assert (summary["4"]["digests_agree"], summary["4"]["digests_compared"]) == (1, 2)
    assert "| 4 | K 3, T 3 | 2.200 (2.000–2.400) | 1.100 (1.000–1.200) | 2.00× | 3 / 9 | 0 / 0 | 1 of 2 | Kaveon: q33 |" in out
    assert report.report_throughput(tmp_path / "empty") is None
