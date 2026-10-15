"""A generator fault must be distinguishable from a configuration it declines.

Both query builders wrap several hundred lines in one `except Exception` and
answer None, and `routers/sql.py` turns None into a flat 400 "Failed to
generate SQL from configuration". A configuration the generator deliberately
cannot express is supposed to produce that 400 — but so did any fault inside
the builder, recorded nowhere, so a chart that would not render left the
operator with the same single sentence the user got.

These tests pin that a fault is recorded, that a declined configuration is
not, and that the record carries no user data: the configuration's values and
the statement being assembled both carry them.
"""

import logging
import unittest
from unittest.mock import patch

from services import query_generator


class GeneratorFaultDiagnosticsTests(unittest.TestCase):
    def _records(self, call):
        with self.assertLogs(query_generator.logger, level=logging.WARNING) as captured:
            result = call()
        return result, captured.output

    def test_a_declined_configuration_is_not_reported_as_a_fault(self):
        """No datasource is a configuration the builder cannot express. It is
        the expected input to a 400, not something to wake anyone for."""
        with patch.object(query_generator.logger, "warning") as warning:
            self.assertIsNone(query_generator.build_chart_preview_query(
                {"datasource": "", "db_type": "postgresql"}))
            self.assertIsNone(query_generator.build_distinct_filter_values_query(
                {"datasource": "", "column": "", "db_type": "postgresql"}))
        warning.assert_not_called()

    def test_a_fault_inside_the_builder_is_recorded(self):
        params = {"datasource": "orders", "metric": "amount",
                  "db_type": "postgresql", "secret_value": "ssn-123-45-6789"}
        with patch.object(query_generator, "normalize_column_name",
                          side_effect=KeyError("broken")):
            result, output = self._records(
                lambda: query_generator.build_chart_preview_query(params))
        self.assertIsNone(result, "the router's contract is unchanged")
        self.assertEqual(len(output), 1)
        self.assertIn("chart_preview_query_failed", output[0])
        self.assertIn("type=KeyError", output[0])
        self.assertIn("db_type=postgresql", output[0])

    def test_the_record_names_the_parameters_and_none_of_their_values(self):
        params = {"datasource": "patients", "column": "ssn",
                  "db_type": "postgresql", "limit": 100}
        with patch.object(query_generator, "normalize_column_name",
                          side_effect=RuntimeError("broken")):
            _, output = self._records(
                lambda: query_generator.build_distinct_filter_values_query(params))
        line = output[0]
        self.assertIn("params=column,datasource,db_type,limit", line)
        for value in ("patients", "ssn"):
            self.assertNotIn(value, line, "a parameter's value is user data")

    def test_both_builders_report_under_their_own_name(self):
        for builder, call in (
            ("chart_preview_query_failed",
             lambda: query_generator.build_chart_preview_query({"datasource": "t"})),
            ("distinct_filter_values_query_failed",
             lambda: query_generator.build_distinct_filter_values_query(
                 {"datasource": "t", "column": "c"})),
        ):
            with self.subTest(builder=builder), \
                 patch.object(query_generator, "normalize_column_name",
                              side_effect=ValueError("broken")):
                _, output = self._records(call)
                self.assertIn(builder, output[0])


if __name__ == "__main__":
    unittest.main()
