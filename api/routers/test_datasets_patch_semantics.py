"""PATCH/PUT /datasets/{id} partial-update semantics.

A field the caller does not send is left as it stands; a field the caller
sends as `null` is cleared. Before this, the router dumped the body with
`exclude_none=True`, so an explicit `null` was indistinguishable from an
omitted field: a request to clear `date_column` answered 200 and changed
nothing."""

import unittest
from unittest.mock import patch

import pydantic
from fastapi import HTTPException

from middleware.auth import UserContext
from models.datasets import DatasetCreate, DatasetUpdate
from routers import datasets as datasets_router

ADMIN = UserContext("admin@example.com", "Admin")

EXISTING = {
    "id": "136", "name": "AI Benchmark Scores", "created_by": "admin@example.com",
    "database_name": "OpenSource", "schema_name": "ai_benchmarks",
    "table_name": "benchmark_scores", "date_column": "release_date",
    "description": "Scores per benchmark", "visibility": "published",
    "columns": [{"column_name": "model_name"}], "dimensions": [], "metrics": [],
}


class _Captured:
    """Runs a router update against a stubbed store and keeps the payload."""

    def __init__(self, existing=None):
        self.existing = existing if existing is not None else EXISTING
        self.payload = None

    def __call__(self, dataset_id, payload, actor):
        self.payload = payload
        return {**self.existing, **payload}

    def run(self, body, method="patch"):
        handler = (datasets_router.patch_dataset if method == "patch"
                   else datasets_router.update_dataset)
        with patch.object(datasets_router.svc, "get_dataset_by_id", return_value=self.existing), \
             patch.object(datasets_router.svc, "update_dataset", side_effect=self):
            return handler("136", DatasetUpdate(**body), ADMIN)


class PatchClearsOnlyWhatTheCallerSentTests(unittest.TestCase):
    def test_an_explicit_null_reaches_the_store_as_a_cleared_field(self):
        run = _Captured()
        result = run.run({"date_column": None})
        self.assertIn("date_column", run.payload)
        self.assertIsNone(run.payload["date_column"])
        self.assertIsNone(result["date_column"])

    def test_an_omitted_field_is_not_in_the_payload_at_all(self):
        run = _Captured()
        run.run({"name": "Renamed"})
        self.assertEqual(run.payload["name"], "Renamed")
        for untouched in ("date_column", "description", "visibility", "columns",
                          "dimensions", "metrics", "table_name", "schema_name"):
            self.assertNotIn(untouched, run.payload)

    def test_description_is_clearable_and_distinct_from_omitted(self):
        cleared = _Captured()
        cleared.run({"description": None})
        self.assertIsNone(cleared.payload["description"])

        omitted = _Captured()
        omitted.run({"name": "Renamed"})
        self.assertNotIn("description", omitted.payload)

    def test_database_column_and_source_are_clearable(self):
        for field in ("database_name", "date_column", "description", "sql_text", "source"):
            run = _Captured()
            run.run({field: None})
            self.assertIn(field, run.payload, field)
            self.assertIsNone(run.payload[field], field)

    def test_put_shares_the_same_partial_semantics_as_patch(self):
        run = _Captured()
        run.run({"date_column": None}, method="put")
        self.assertIn("date_column", run.payload)
        self.assertIsNone(run.payload["date_column"])

    def test_clearing_the_date_column_of_an_engine_bound_dataset_is_not_refilled(self):
        """`apply_binding` fills a missing date column from the Engine's table
        definition. A caller clearing one must not have it handed straight
        back, so the stored value is what the binding consults."""
        run = _Captured({**EXISTING, "source": {"kind": "engine", "table_id": "t1"}})
        run.run({"date_column": None})
        self.assertIsNone(run.payload["date_column"])

    def test_a_viewer_without_write_access_is_still_refused(self):
        with patch.object(datasets_router.svc, "get_dataset_by_id", return_value=EXISTING), \
             patch.object(datasets_router.svc, "update_dataset") as store:
            with self.assertRaises(HTTPException) as error:
                datasets_router.patch_dataset(
                    "136", DatasetUpdate(date_column=None),
                    UserContext("other@example.com", "Viewer"),
                )
        self.assertEqual(error.exception.status_code, 403)
        store.assert_not_called()

    def test_a_missing_dataset_is_a_404_before_any_write(self):
        with patch.object(datasets_router.svc, "get_dataset_by_id", return_value=None), \
             patch.object(datasets_router.svc, "update_dataset") as store:
            with self.assertRaises(HTTPException) as error:
                datasets_router.patch_dataset("999", DatasetUpdate(date_column=None), ADMIN)
        self.assertEqual(error.exception.status_code, 404)
        store.assert_not_called()


class UnclearableFieldsAreRefusedTests(unittest.TestCase):
    """A dataset always has a name, a table, a schema and a visibility, and a
    component collection is emptied with `[]`. `null` for one of those is a
    caller mistake, refused at the model rather than reaching a NOT NULL
    column or being silently dropped."""

    def test_every_unclearable_field_is_refused_as_null(self):
        for field in DatasetUpdate.NOT_CLEARABLE:
            with self.assertRaises(pydantic.ValidationError, msg=field) as error:
                DatasetUpdate(**{field: None})
            self.assertIn("cannot be cleared", str(error.exception))

    def test_a_collection_is_emptied_with_an_empty_list(self):
        run = _Captured()
        run.run({"columns": [], "dimensions": [], "metrics": []})
        self.assertEqual(run.payload["columns"], [])
        self.assertEqual(run.payload["dimensions"], [])
        self.assertEqual(run.payload["metrics"], [])

    def test_omitting_an_unclearable_field_is_still_fine(self):
        self.assertEqual(DatasetUpdate().model_dump(exclude_unset=True), {})


class StudioCallerShapesTests(unittest.TestCase):
    """The two Studio callers of PATCH /datasets/{id}, as they send it.

    The dataset editor (`studio/app/datasets/new/page.tsx`) hydrates every
    field it sends from the dataset it loaded, so a `null` there is the user
    having cleared the input — exactly the clear this change enables. It never
    sends `null` for a field it does not mean to clear. The inline rename
    (`studio/app/datasets/[id]/page.tsx`) sends the name alone."""

    EDITOR_BODY = {
        "name": "AI Benchmark Scores",
        "description": None,
        "table_name": "benchmark_scores",
        "schema_name": "ai_benchmarks",
        "database_name": "OpenSource",
        "date_column": None,
        "dimensions": [],
        "columns": [{"table_name": "benchmark_scores", "column_name": "model_name",
                     "is_dimension": True}],
        "metrics": [],
    }

    def test_the_dataset_editor_clears_exactly_the_inputs_the_user_emptied(self):
        run = _Captured()
        run.run(self.EDITOR_BODY)
        self.assertIsNone(run.payload["date_column"])
        self.assertIsNone(run.payload["description"])
        self.assertEqual(run.payload["table_name"], "benchmark_scores")
        self.assertEqual(run.payload["schema_name"], "ai_benchmarks")
        # Never sent by the editor, so never touched by it.
        self.assertNotIn("visibility", run.payload)
        self.assertNotIn("sql_text", run.payload)
        self.assertNotIn("source", run.payload)

    def test_the_inline_rename_touches_nothing_but_the_name(self):
        run = _Captured()
        run.run({"name": "Benchmark scores"})
        self.assertEqual(run.payload, {"name": "Benchmark scores"})

    def test_an_editor_body_with_a_filled_date_column_keeps_it(self):
        run = _Captured()
        run.run({**self.EDITOR_BODY, "date_column": "measured_at"})
        self.assertEqual(run.payload["date_column"], "measured_at")


class CreateKeepsDroppingNullsTests(unittest.TestCase):
    """A create has no prior value, so an omitted field and one sent as `null`
    mean the same thing. Dropping the null is also what keeps `schema_name`
    and `database_name` — NOT NULL columns with store-side defaults — valid."""

    def test_nulls_are_dropped_from_a_create_payload(self):
        body = DatasetCreate(name="New dataset", schema_name=None, database_name=None,
                             date_column=None, description=None)
        payload = body.model_dump(exclude_none=True)
        self.assertEqual(payload, {"name": "New dataset"})

    def test_create_does_not_inherit_the_update_null_refusal(self):
        self.assertEqual(DatasetCreate(name="New dataset", description=None).description, None)


if __name__ == "__main__":
    unittest.main()
