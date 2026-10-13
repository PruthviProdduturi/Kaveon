"""The rebuilt events table, rehearsed small.

Covers the properties the rebuild exists to establish — rows exist because of
behaviour rather than by construction, `user_id` is an opaque UUID, `event_date`
is a date, the metrics are drawn rather than patterned, and the world is
covered — plus the invariants a half-billion-row publish depends on: the file
plan covers every user exactly once per day, a file never spans two days,
row-group statistics bound the pruning columns, the written files match what the
seeds predict, and the appender cannot drift from the generator's domains or
presence model.
"""
import importlib.util
import json
import re
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
# Deliberately not divisible by the per-day block count: remainders are spread,
# and a generator that can only take round user counts is how the published
# table ended up with exactly 3,000,000 users.
USERS = 2_413

UUID4 = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")


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


class IdentifierTests(unittest.TestCase):
    def test_identifiers_are_well_formed_uuid_v4(self):
        identifiers = builder.user_ids(builder._uuid_bytes(5_000), 0, 5_000)
        self.assertEqual(len(identifiers), 5_000)
        for identifier in identifiers[:500]:
            self.assertRegex(identifier, UUID4)

    def test_identifiers_are_unique_and_carry_no_order(self):
        identifiers = builder.user_ids(builder._uuid_bytes(20_000), 0, 20_000)
        self.assertEqual(len(set(identifiers)), 20_000)
        # A sequential key leaks the user count and the creation order. Sorting
        # the identifiers must not recover the order they were drawn in.
        self.assertNotEqual(identifiers, sorted(identifiers))

    def test_a_user_keeps_their_identifier_whatever_the_population(self):
        small = builder.user_ids(builder._uuid_bytes(1_000), 0, 1_000)
        large = builder.user_ids(builder._uuid_bytes(4_000), 0, 4_000)
        self.assertEqual(small, large[:1_000])

    def test_a_block_is_addressed_by_offset(self):
        raw = builder._uuid_bytes(1_000)
        self.assertEqual(builder.user_ids(raw, 400, 10),
                         builder.user_ids(raw, 0, 1_000)[400:410])


class PlanTests(unittest.TestCase):
    def test_a_file_never_spans_two_days(self):
        layout = builder.file_plan(DAYS, FILES, USERS)
        per_day = Counter(day for _, day, _, _, _ in layout)
        self.assertEqual(len(per_day), DAYS)
        self.assertEqual(sum(per_day.values()), FILES)
        blocks = {day: sorted(block for _, other, block, _, _ in layout if other == day)
                  for day in per_day}
        for day, indices in blocks.items():
            self.assertEqual(indices, list(range(len(indices))), day)

    def test_every_user_is_covered_exactly_once_per_day(self):
        layout = builder.file_plan(DAYS, FILES, USERS)
        for day in {day for _, day, _, _, _ in layout}:
            spans = sorted((first, count) for _, other, _, first, count in layout
                           if other == day)
            at = 0
            for first, count in spans:
                self.assertEqual(first, at, day)
                at += count
            self.assertEqual(at, USERS, day)

    def test_the_published_plan_covers_the_window(self):
        layout = builder.file_plan()
        self.assertEqual(len(layout), builder.FILES)
        self.assertEqual(len({day for _, day, _, _, _ in layout}), builder.DAYS)

    def test_a_busy_day_gets_more_files_than_a_quiet_one(self):
        allocated = builder.files_per_day()
        self.assertEqual(sum(allocated), builder.FILES)
        self.assertTrue(all(count >= 1 for count in allocated))
        weekend = [count for offset, count in enumerate(allocated)
                   if (builder.FIRST_DAY.weekday() + offset) % 7 >= 5]
        weekday = [count for offset, count in enumerate(allocated)
                   if (builder.FIRST_DAY.weekday() + offset) % 7 < 5]
        # Even file sizes are the point: a Sunday with as many files as a
        # Wednesday would hold a third of the rows in each of them.
        self.assertLess(max(weekend), min(weekday))

    def test_a_file_count_that_cannot_cover_the_days_is_refused(self):
        with self.assertRaises(SystemExit):
            builder.file_plan(DAYS, DAYS - 1, USERS)


class BuildTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls._directory = tempfile.TemporaryDirectory()
        cls.output = Path(cls._directory.name) / "activity"
        builder.build(cls.output, DAYS, FILES, USERS)
        cls.parts = sorted(cls.output.glob("*.parquet"))
        cls.manifest = json.loads(
            (cls.output / "build-manifest.json").read_text(encoding="utf-8"))

    @classmethod
    def tearDownClass(cls):
        cls._directory.cleanup()

    def rows_per_day(self) -> dict[date, int]:
        by_day: dict[date, int] = {}
        for part in self.parts:
            table = pq.read_table(part, columns=["event_date"])
            day = table.column("event_date")[0].as_py()
            by_day[day] = by_day.get(day, 0) + table.num_rows
        return by_day

    def test_the_files_hold_what_the_seeds_predict(self):
        self.assertEqual(len(self.parts), FILES)
        counted = builder.verify(self.output, DAYS, FILES, USERS)
        self.assertEqual(counted, self.manifest["row_count"])

    def test_the_table_is_not_a_cross_product(self):
        counted = sum(pq.ParquetFile(part).metadata.num_rows for part in self.parts)
        grid = USERS * DAYS * len(builder.SURFACES)
        # The defect this rebuild exists to remove: every user on every surface
        # on every day. A table that fills most of the grid is that table again.
        self.assertLess(counted, grid * 0.6)
        self.assertGreater(counted, grid * 0.1)

    def test_no_two_days_hold_the_same_number_of_rows(self):
        counts = list(self.rows_per_day().values())
        self.assertEqual(len(counts), DAYS)
        self.assertEqual(len(set(counts)), DAYS)

    def test_weekends_are_quieter_than_weekdays_in_rows_not_only_in_values(self):
        # 2026-07-04 is a Saturday, so the window opens on a weekend.
        by_day = self.rows_per_day()
        weekend = [rows for day, rows in by_day.items() if day.weekday() >= 5]
        weekday = [rows for day, rows in by_day.items() if day.weekday() < 5]
        self.assertTrue(weekend and weekday)
        self.assertLess(max(weekend), min(weekday))

    def test_most_users_are_absent_from_most_surfaces_on_a_day(self):
        table = pq.read_table(self.parts[0], columns=["user_id", "surface"])
        per_user = Counter(table.column("user_id").to_pylist())
        # A block's users each appear on between one and six surfaces, and the
        # typical user is nowhere near all six.
        self.assertLessEqual(max(per_user.values()), len(builder.SURFACES))
        self.assertLess(np.median(list(per_user.values())), 3)
        # And some of the block's users were not active at all that day.
        _, _, _, first, count = builder.file_plan(DAYS, FILES, USERS)[0]
        self.assertLess(len(per_user), count)

    def test_user_id_is_an_opaque_identifier_not_a_counter(self):
        schema = pq.ParquetFile(self.parts[0]).schema_arrow
        self.assertEqual(schema.field("user_id").type, pa.string())
        identifiers = pq.read_table(self.parts[0], columns=["user_id"]) \
            .column("user_id").to_pylist()
        for identifier in identifiers[:200]:
            self.assertRegex(identifier, UUID4)

    def test_a_user_keeps_one_set_of_dimensions_everywhere(self):
        # The dimensions are properties of the user, so they are drawn once and
        # never re-rolled per row. Re-rolling would make `segment` a property of
        # a row, and every per-user breakdown meaningless.
        columns = ["user_id", *builder.USER_DIMENSIONS]
        combinations: dict[str, set] = {}
        for part in self.parts:
            table = pq.read_table(part, columns=columns).to_pylist()
            for row in table:
                key = row["user_id"]
                combinations.setdefault(key, set()).add(
                    tuple(row[name] for name in builder.USER_DIMENSIONS))
        self.assertTrue(combinations)
        self.assertTrue(all(len(seen) == 1 for seen in combinations.values()))

    def test_event_date_is_a_date_not_text(self):
        schema = pq.ParquetFile(self.parts[0]).schema_arrow
        self.assertEqual(schema.field("event_date").type, pa.date32())
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
            again = Path(other) / "activity"
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

    def test_the_manifest_records_the_published_types_and_the_daily_shape(self):
        types = {column["name"]: column["data_type"] for column in self.manifest["columns"]}
        self.assertEqual(types["event_date"], "date32[day]")
        self.assertEqual(types["user_id"], "string")
        self.assertEqual(types["rows_scanned"], "int64")
        self.assertEqual(types["country"], "string")
        self.assertEqual(self.manifest["users"], USERS)
        # Not every drawn user was active in the window, so the distinct count
        # is below the population and the population is not recoverable from it.
        self.assertLess(self.manifest["distinct_users"], USERS)
        self.assertGreater(self.manifest["distinct_users"], USERS * 0.8)
        self.assertEqual(self.manifest["table"], "KaveonTelemetry.events.activity")
        self.assertEqual(len(self.manifest["rows_per_day"]), DAYS)
        self.assertEqual(sum(self.manifest["rows_per_day"].values()),
                         self.manifest["row_count"])
        self.assertEqual(len(self.manifest["countries"]), len(geography.COUNTRIES))

    def test_a_truncated_part_is_refused_rather_than_published(self):
        with tempfile.TemporaryDirectory() as other:
            copy = Path(other) / "activity"
            copy.mkdir()
            for part in self.parts:
                (copy / part.name).write_bytes(part.read_bytes())
            short = pq.read_table(self.parts[0]).slice(0, 10)
            pq.write_table(short, copy / self.parts[0].name)
            with self.assertRaises(SystemExit):
                builder.verify(copy, DAYS, FILES, USERS)

    def test_a_part_outside_the_plan_is_refused(self):
        with tempfile.TemporaryDirectory() as other:
            copy = Path(other) / "activity"
            copy.mkdir()
            for part in self.parts:
                (copy / part.name).write_bytes(part.read_bytes())
            (copy / "activity-2030-01-01-000.parquet").write_bytes(
                self.parts[0].read_bytes())
            with self.assertRaises(SystemExit):
                builder.verify(copy, DAYS, FILES, USERS)


class ShapeGateTests(unittest.TestCase):
    def test_a_flat_daily_distribution_is_refused(self):
        flat = {date(2026, 7, 6): 1_000, date(2026, 7, 7): 1_000}
        with self.assertRaises(SystemExit):
            builder.report_shape(flat, 100)

    def test_a_round_total_is_refused(self):
        rounded = {date(2026, 7, 6): 600_000, date(2026, 7, 7): 400_000}
        with self.assertRaises(SystemExit):
            builder.report_shape(rounded, 1_000_000)

    def test_a_busy_weekend_is_refused(self):
        inverted = {date(2026, 7, 4): 9_000, date(2026, 7, 6): 1_000}
        with self.assertRaises(SystemExit):
            builder.report_shape(inverted, 10_000)


class AppenderTests(unittest.TestCase):
    def test_the_appender_takes_its_domains_from_the_generator(self):
        self.assertIs(appender.SCHEMA, builder.SCHEMA)
        self.assertIs(appender.SURFACES, builder.SURFACES)
        self.assertIs(appender.present_users, builder.present_users)
        self.assertIs(appender.user_ids, builder.user_ids)

    def test_an_appended_day_matches_the_table_schema(self):
        batch = appender.build_day(date(2026, 8, 1), 2_000)
        self.assertEqual(batch.schema, builder.SCHEMA)
        self.assertEqual(batch.column("event_date")[0].as_py(), date(2026, 8, 1))

    def test_an_appended_day_is_not_a_cross_product_either(self):
        batch = appender.build_day(date(2026, 8, 1), 2_000)
        self.assertLess(batch.num_rows, 2_000 * len(builder.SURFACES) * 0.6)
        self.assertGreater(batch.num_rows, 2_000 * 0.1)

    def test_an_appended_day_uses_the_same_identifiers(self):
        batch = appender.build_day(date(2026, 8, 1), 2_000)
        expected = set(builder.user_ids(builder._uuid_bytes(2_000), 0, 2_000))
        self.assertTrue(set(batch.column("user_id").to_pylist()) <= expected)

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
        self.assertTrue(np.array_equal(small["user_uuid"],
                                       large["user_uuid"][:1_000]))


if __name__ == "__main__":
    unittest.main()
