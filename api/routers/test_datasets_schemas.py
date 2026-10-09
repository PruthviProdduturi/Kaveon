"""GET /datasets/schemas — every visible dataset's askable shape in one read.

The chat workbench needs the columns and metrics of every dataset the caller
can see, so a question can be routed to the dataset that can answer it. It used
to collect that one dataset at a time, and each ``GET /datasets/{id}`` costs a
document read, three component reads and a shadow-read observation — so a
nine-dataset deployment paid that nine times before the composer would accept a
keystroke. These cases pin the shape of the single read that replaced it, and
that the route is reachable at all: declared after ``/datasets/{dataset_id}``,
"schemas" is taken for a dataset identifier and the request fails.
"""

import unittest
from unittest.mock import patch

from fastapi import Response
from fastapi.routing import APIRoute

from middleware.auth import UserContext
from routers import datasets as datasets_router
import services.datasets as svc

VIEWER = UserContext("viewer@example.com", "Viewer")

LISTED = [
    {
        "id": "136", "dataset_name": "AI Benchmark Scores", "description": "Scores",
        "database_name": "OpenSource", "schema_name": "ai_benchmarks",
        "table_name": "benchmark_scores",
    },
    {
        "id": "144", "dataset_name": "Climate and Energy", "description": None,
        "database_name": "OpenSource", "schema_name": "public",
        "table_name": "energy_mix",
    },
]

COLUMNS = {
    136: [
        {"table_name": "benchmark_scores", "column_name": "model_name",
         "data_type": "varchar(200)", "is_dimension": True, "is_metric": False,
         "semantic_type": None, "dataset_id": 136},
        {"table_name": "benchmark_scores", "column_name": "score",
         "data_type": "numeric(10,2)", "is_dimension": False, "is_metric": True,
         "semantic_type": None, "dataset_id": 136},
    ],
    144: [
        {"table_name": "energy_mix", "column_name": "reported_on",
         "data_type": "timestamp", "is_dimension": False, "is_metric": False,
         "semantic_type": None, "dataset_id": 144},
    ],
}

METRICS = {
    136: [{"metric_name": "Mean score", "expression": "AVG(score)",
           "metric_type": "avg", "format": None, "dataset_id": 136}],
}


def _bulk(sql, params):
    """Stands in for the metadata DB: one read per component table, not per dataset."""
    if "dataset_columns" in sql:
        rows = [row for did in params for row in COLUMNS.get(int(did), [])]
    elif "dataset_metrics" in sql:
        rows = [row for did in params for row in METRICS.get(int(did), [])]
    elif "dataset_dimensions" in sql:
        rows = []
    else:
        raise AssertionError(f"unexpected read: {sql}")
    return {"rows": rows}


class DatasetSchemasRoute(unittest.TestCase):
    def test_schemas_route_is_declared_before_the_dataset_identifier_route(self):
        paths = [route.path for route in datasets_router.router.routes
                 if isinstance(route, APIRoute) and "GET" in route.methods]
        self.assertIn("/datasets/schemas", paths)
        self.assertLess(paths.index("/datasets/schemas"),
                        paths.index("/datasets/{dataset_id}"))

    def test_router_returns_the_catalogue_with_its_count(self):
        with patch.object(datasets_router.svc, "list_dataset_schemas",
                          return_value=[{"id": "136"}]) as listed:
            body = datasets_router.datasets_schemas(Response(), VIEWER)
        listed.assert_called_once_with("viewer@example.com", "Viewer")
        self.assertEqual(body, {"count": 1, "schemas": [{"id": "136"}]})


class DatasetSchemasService(unittest.TestCase):
    def test_components_come_from_bulk_reads_not_one_read_per_dataset(self):
        with patch.object(svc, "list_datasets", return_value=LISTED), \
             patch.object(svc.db, "query", side_effect=_bulk) as query, \
             patch.object(svc, "get_dataset_by_id") as per_dataset:
            entries = svc.list_dataset_schemas("viewer@example.com", "Viewer")

        per_dataset.assert_not_called()
        self.assertEqual(query.call_count, 3)
        for call in query.call_args_list:
            self.assertIn("dataset_id IN (@param0, @param1)", call.args[0])
            self.assertEqual(call.args[1], [136, 144])

        self.assertEqual([entry["id"] for entry in entries], ["136", "144"])
        first = entries[0]
        self.assertEqual(first["name"], "AI Benchmark Scores")
        self.assertEqual(first["table"], "ai_benchmarks.benchmark_scores")
        self.assertEqual(first["columns"],
                         [{"name": "model_name", "data_type": "varchar(200)"},
                          {"name": "score", "data_type": "numeric(10,2)"}])
        self.assertEqual(first["metrics"],
                         [{"name": "Mean score", "expression": "AVG(score)"}])
        # A default schema is not worth naming, and a dataset with no metric
        # reports none rather than being dropped from the catalogue.
        self.assertEqual(entries[1]["table"], "energy_mix")
        self.assertEqual(entries[1]["metrics"], [])

    def test_documents_that_already_carry_components_cost_no_further_read(self):
        documents = [{
            "id": "136", "dataset_name": "AI Benchmark Scores",
            "database_name": "OpenSource", "schema_name": "dbo",
            "table_name": "benchmark_scores",
            "columns": [{"column_name": "model_name", "data_type": "varchar(200)"}],
            "metrics": [{"name": "Mean score", "expression": "AVG(score)"}],
        }]
        with patch.object(svc, "list_datasets", return_value=documents), \
             patch.object(svc.db, "query") as query:
            entries = svc.list_dataset_schemas("viewer@example.com", "Admin")

        query.assert_not_called()
        self.assertEqual(entries[0]["columns"],
                         [{"name": "model_name", "data_type": "varchar(200)"}])
        # dbo is a default schema: the table is named once, unqualified.
        self.assertEqual(entries[0]["table"], "benchmark_scores")

    def test_no_visible_dataset_reads_nothing_further(self):
        with patch.object(svc, "list_datasets", return_value=[]), \
             patch.object(svc.db, "query") as query:
            self.assertEqual(svc.list_dataset_schemas("viewer@example.com", "Viewer"), [])
        query.assert_not_called()

    def test_a_table_that_already_carries_its_schema_is_not_qualified_twice(self):
        self.assertEqual(svc._qualified_table("silver", "silver.trips"), "silver.trips")
        self.assertEqual(svc._qualified_table("silver", "trips"), "silver.trips")
        self.assertEqual(svc._qualified_table("public", "trips"), "trips")
        self.assertEqual(svc._qualified_table(None, None), "data")


if __name__ == "__main__":
    unittest.main()
