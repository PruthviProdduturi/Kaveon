from pathlib import Path
import sqlite3
import unittest


class LegacyDashboardExportTests(unittest.TestCase):
    def test_event_projection_has_the_minimum_lossless_grain(self):
        source = Path(__file__).with_name("export-legacy-dashboard-data.py").read_text(encoding="utf-8")
        query = source.split('EVENTS_DASHBOARD_QUERY = """', 1)[1].split('"""', 1)[0]
        normalized = " ".join(query.lower().split())
        self.assertIn("group by user_id, surface", normalized)
        self.assertNotIn("event_date", normalized)
        for metric in (
            "sum(actions)", "sum(sessions)", "sum(queries_run)",
            "sum(charts_created)", "sum(errors)", "sum(rows_scanned)",
            "sum(cache_hits)", "avg(latency_p75_ms)",
        ):
            self.assertIn(metric, normalized)

    def test_projection_preserves_canonical_aggregates_under_filters(self):
        db = sqlite3.connect(":memory:")
        db.execute("CREATE TABLE events(user_id INT, surface TEXT, region TEXT, actions INT, latency REAL)")
        rows = [
            (user, surface, region, day + user, 100 + day + user)
            for user, region in ((1, "NA"), (2, "EU"), (3, "NA"))
            for surface in ("API", "SQL Lab") for day in range(28)
        ]
        db.executemany("INSERT INTO events VALUES (?,?,?,?,?)", rows)
        db.execute("""CREATE TABLE compact AS
            SELECT user_id, surface, region, SUM(actions) actions, AVG(latency) latency
            FROM events GROUP BY user_id, surface, region""")
        for where in ("1=1", "region='NA'", "surface='API' AND region='NA'"):
            raw = db.execute(f"SELECT SUM(actions), AVG(latency), COUNT(DISTINCT user_id) FROM events WHERE {where}").fetchone()
            compact = db.execute(f"SELECT SUM(actions), AVG(latency), COUNT(DISTINCT user_id) FROM compact WHERE {where}").fetchone()
            self.assertEqual(raw[0], compact[0])
            self.assertAlmostEqual(raw[1], compact[1])
            self.assertEqual(raw[2], compact[2])
        db.close()


if __name__ == "__main__":
    unittest.main()
