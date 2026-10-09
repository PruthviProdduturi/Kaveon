"""The rebuilt events table, rehearsed small.

Covers the three properties the rebuild exists to establish — `event_date` is
a date, the metrics are drawn rather than patterned, and the world is covered —
plus the invariants a 504,000,000-row publish depends on: the file plan is
exact, a file never spans two days, row-group statistics bound the pruning
columns, and the appender cannot drift from the generator's domains.
"""
import importlib.util
import json
import sys
import tempfile
import unittest
from collections import Counter
from datetime import date
from pathlib import Path

import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build_kaveon_events_table as builder  # noqa: E402
import kaveon_events_geography as geography  # noqa: E402

APPENDER = importlib.util.spec_from_file_location(
    "append_events", Path(__file__).with_name("append-events.py"))
appender = importlib.util.module_from_spec(APPENDER)
sys.modules[APPENDER.name] = appender
APPENDER.loader.exec_module(appender)

DAYS = 4
FILES = 6
USERS = 2_400


class GeographyTests(unittest.TestCase):
    def test_every_country_renders_on_the_registered_map(self):
        geography.validate_against_geojson()

    def test_coverage_is_global_and_broad(self):
        self.assertGreaterEqual(len(geography.COUNTRIES), 100)
        per_region = Counter(geography.COUNTRY_REGION.values())
        self.assertEqual(set(per_region), set(geography.REGIONS))
        # No region may be represented by a token country or two, which is
        # what left the choropleth looking empty before.
        self.assertTrue(all(count >= 10 for count in per_region.values()), per_region)

    def test_shares_are_a_distribution_with_a_floor(self):
        shares = geography.shares()
        self.assertAlmostEqual(sum(shares), 1.0, places=9)
        floor = geography.FLOOR_MIXTURE / len(shares)
        self.assertTrue(all(share >= floor * 0.999 for share in shares))
        ranked = sorted(zip(geography.COUNTRY_NAMES, shares), key=lambda row: -row[1])
        # The weighting is online population x adoption, so the markets with
        # the largest technical audiences lead and no small state outranks one.
        self.assertEqual([name for name, _ in ranked[:3]],
                         ["United States", "India", "China"])

    def test_region_follows_the_country(self):
        attributes = builder.build_user_attributes(5_000)
        countries, country_index = attributes["country"]
        regions, region_index = attributes["region"]
        for position in range(0, 5_000, 97):
            self.assertEqual(regions[region_index[position]],
                             geography.COUNTRY_REGION[countries[country_index[position]]])


class PlanTests(unittest.TestCase):
    def test_the_published_plan_is_exact(self):
        layout = builder.file_plan()
        self.assertEqual(len(layout), builder.FILES)
        rows = sum(per_block * len(builder.SURFACES) for *_, per_block in layout)
        self.assertEqual(rows, builder.EXPECTED_ROWS)

    def test_a_file_never_spans_two_days(self):
        layout = builder.file_plan(DAYS, FILES, USERS)
        per_day = Counter(day for _, day, _, _ in layout)
        self.assertEqual(len(per_day), DAYS)
        self.assertEqual(sum(per_day.values()), FILES)
        blocks = {day: sorted(block for _, other, block, _ in layout if other == day)
                  for day in per_day}
        for day, indices in blocks.items():
            self.assertEqual(indices, list(range(len(indices))), day)

    def test_a_file_count_that_cannot_divide_the_users_is_refused(self):
        # 2,401 users do not divide into the 3 blocks a day would get here, so
        # a file would be short and the row count would miss by a remainder.
        with self.assertRaises(SystemExit):
            builder.file_plan(DAYS, DAYS * 3, USERS + 1)


class BuildTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls._directory = tempfile.TemporaryDirectory()
        cls.output = Path(cls._directory.name) / "events"
        builder.build(cls.output, DAYS, FILES, USERS)
        cls.parts = sorted(cls.output.glob("*.parquet"))

    @classmethod
    def tearDownClass(cls):
        cls._directory.cleanup()

    def test_row_count_and_file_count_are_what_was_planned(self):
        self.assertEqual(len(self.parts), FILES)
        counted = sum(pq.ParquetFile(part).metadata.num_rows for part in self.parts)
        self.assertEqual(counted, USERS * DAYS * len(builder.SURFACES))

    def test_event_date_is_a_date_not_text(self):
        schema = pq.ParquetFile(self.parts[0]).schema_arrow
        self.assertEqual(schema.field("event_date").type, pa.date32())
        # Everything else a reader sees is unchanged from the published table.
        self.assertEqual(schema.names, builder.SCHEMA.names)
        for name in builder.METRICS:
            self.assertEqual(schema.field(name).type, pa.int64())
        for name in builder.USER_DIMENSIONS:
            self.assertEqual(schema.field(name).type, pa.string())

    def test_one_row_group_per_surface_with_exact_statistics(self):
        metadata = pq.ParquetFile(self.parts[0]).metadata
        self.assertEqual(metadata.num_row_groups, len(builder.SURFACES))
        dates, surfaces = set(), []
        for index in range(metadata.num_row_groups):
            group = metadata.row_group(index)
            date_stats = group.column(0).statistics
            self.assertEqual(date_stats.min, date_stats.max)
            dates.add(date_stats.min)
            surface_stats = group.column(2).statistics
            self.assertEqual(surface_stats.min, surface_stats.max)
            surfaces.append(surface_stats.min)
        self.assertEqual(len(dates), 1)
        self.assertEqual(surfaces, list(builder.SURFACES))

    def test_the_build_is_reproducible(self):
        with tempfile.TemporaryDirectory() as other:
            again = Path(other) / "events"
            builder.build(again, DAYS, FILES, USERS)
            for first in self.parts:
                second = again / first.name
                self.assertEqual(pq.read_table(first), pq.read_table(second), first.name)

    def test_values_do_not_cluster_on_round_numbers(self):
        latency = pq.read_table(self.parts[0], columns=["latency_p75_ms"]) \
            .column("latency_p75_ms").to_numpy()
        rows = latency.shape[0]
        # A drawn value lands on a multiple of 100 about one time in a hundred.
        # The generator this replaces produced ranges whose bounds were round
        # and heavily hit, so a large excess here is the defect returning.
        self.assertLess((latency % 100 == 0).sum(), rows * 0.02)
        self.assertLess((latency % 50 == 0).sum(), rows * 0.04)
        # And the distribution is a long right tail, not a uniform band.
        self.assertGreater(np.percentile(latency, 99) / np.median(latency), 2.0)
        self.assertGreater(latency.max() / np.median(latency), 4.0)

    def test_metrics_are_plausible_and_bounded_below(self):
        table = pq.read_table(self.parts[0])
        self.assertTrue((table.column("actions").to_numpy() >= 1).all())
        self.assertTrue((table.column("sessions").to_numpy() >= 1).all())
        self.assertTrue((table.column("rows_scanned").to_numpy() >= 0).all())
        errors = table.column("errors").to_numpy()
        # Errors are rare, and most rows have none — a uniform draw over a
        # range would give every row an error far too often.
        self.assertGreater((errors == 0).mean(), 0.5)

    def test_weekends_are_quieter_than_weekdays(self):
        # 2026-07-04 is a Saturday, so the first day of the window is a weekend.
        by_day = {}
        for part in self.parts:
            table = pq.read_table(part, columns=["event_date", "actions"])
            day = table.column("event_date")[0].as_py()
            by_day[day] = by_day.get(day, 0) + int(
                table.column("actions").to_numpy().sum())
        self.assertLess(by_day[date(2026, 7, 4)], by_day[date(2026, 7, 6)])

    def test_the_manifest_records_the_published_types(self):
        manifest = json.loads((self.output / "build-manifest.json").read_text(encoding="utf-8"))
        types = {column["name"]: column["data_type"] for column in manifest["columns"]}
        self.assertEqual(types["event_date"], "date32[day]")
        self.assertEqual(types["rows_scanned"], "int64")
        self.assertEqual(types["country"], "string")
        self.assertEqual(manifest["row_count"], USERS * DAYS * len(builder.SURFACES))
        self.assertEqual(len(manifest["countries"]), len(geography.COUNTRIES))

    def test_a_short_build_is_refused_rather_than_published(self):
        with self.assertRaises(SystemExit):
            builder.verify(self.output, USERS * DAYS * len(builder.SURFACES) + 1)


class AppenderTests(unittest.TestCase):
    def test_the_appender_takes_its_domains_from_the_generator(self):
        self.assertIs(appender.SCHEMA, builder.SCHEMA)
        self.assertIs(appender.SURFACES, builder.SURFACES)

    def test_an_appended_day_matches_the_table_schema(self):
        batch = appender.build_day(date(2026, 8, 1), 500)
        self.assertEqual(batch.schema, builder.SCHEMA)
        self.assertEqual(batch.num_rows, 500 * len(builder.SURFACES))
        self.assertEqual(batch.column("event_date")[0].as_py(), date(2026, 8, 1))

    def test_an_appended_day_invents_no_value(self):
        batch = appender.build_day(date(2026, 8, 1), 2_000)
        for name in builder.USER_DIMENSIONS:
            values = set(batch.column(name).to_pylist())
            if name == "country":
                allowed = set(geography.COUNTRY_NAMES)
            elif name == "region":
                allowed = set(geography.REGIONS)
            else:
                allowed = {value for value, _ in builder.WEIGHTED_DIMENSIONS[name]}
            self.assertTrue(values <= allowed, f"{name}: {sorted(values - allowed)}")

    def test_a_user_keeps_their_attributes_across_an_append(self):
        small = builder.build_user_attributes(1_000)
        large = builder.build_user_attributes(4_000)
        for name in builder.USER_DIMENSIONS:
            values, indices = small[name]
            other_values, other_indices = large[name]
            self.assertEqual(values, other_values, name)
            self.assertTrue(np.array_equal(indices, other_indices[:1_000]), name)


if __name__ == "__main__":
    unittest.main()
