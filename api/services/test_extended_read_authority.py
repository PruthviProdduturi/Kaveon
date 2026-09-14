import os
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

from routers import chat_history, data_sources
from services import favorites, product_read_authority as authority, query_history, saved_queries, theme, user_recents


class ExtendedReadAuthorityTests(unittest.TestCase):
    def env(self, family):
        return patch.dict(os.environ, {authority.ENVIRONMENT_KEY: family}, clear=True)

    def test_personal_startup_reads_do_not_touch_postgresql(self):
        cases = (
            ("saved_queries", lambda: saved_queries.list_saved_queries("alice"), saved_queries.db),
            ("user_recents", lambda: user_recents.get_recents("alice"), user_recents.db),
            ("favorites", lambda: favorites.list_favorites("alice"), favorites.db),
            ("query_history", lambda: query_history.list_history("alice", 5), query_history.db),
        )
        document = {"id": "1", "user_email": "alice", "created_by": "alice",
                    "object_type": "dataset", "object_id": "7", "created_at": "2026-01-01"}
        for family, operation, database in cases:
            with self.subTest(family=family), self.env(family), \
                 patch.object(authority.product_store, "list_records",
                              side_effect=lambda kind, *args, **kwargs: [] if kind == "favorite" else [{"document": document}]), \
                 patch.object(database, "query") as postgres:
                operation()
                postgres.assert_not_called()

    def test_theme_cutover_bypasses_cache_and_postgresql(self):
        target = {"document": {"user_email": "alice", "theme_color": "#123456"}}
        with self.env("user_themes"), patch.object(authority.product_store, "read", return_value=target), \
             patch.object(theme.db, "query_one") as postgres:
            self.assertEqual(theme.get_user_theme("alice"), {"theme_color": "#123456"})
            postgres.assert_not_called()

    def test_chat_lists_and_point_reads_do_not_touch_postgresql(self):
        session = {"id": "4", "user_email": "alice", "title": "Chat",
                   "created_at": "2026-01-01", "updated_at": "2026-01-02"}
        message = {"id": "8", "session_id": "4", "user_email": "alice", "role": "user",
                   "content": "hello", "created_at": "2026-01-02"}
        ctx = SimpleNamespace(email="alice", role="Viewer")
        def records(kind, *args, **kwargs): return [{"document": message if kind == "chat_message" else session}]
        def point(kind, *args, **kwargs): return {"document": session} if kind == "chat_session" else None
        with self.env("chat_history"), patch.object(authority.product_store, "list_records", side_effect=records), \
             patch.object(authority.product_store, "read", side_effect=point), \
             patch.object(chat_history.db, "query") as query, patch.object(chat_history.db, "query_one") as query_one:
            self.assertEqual(chat_history.list_sessions(ctx=ctx)["count"], 1)
            self.assertEqual(chat_history.get_session(4, ctx=ctx)["messages"][0]["content"], "hello")
            query.assert_not_called(); query_one.assert_not_called()

    def test_source_startup_list_uses_only_kaveondb(self):
        source = {"source_kind": "data", "source_id": "data-2", "name": "Warehouse",
                  "source_type": "PostgreSQL", "database_name": "warehouse", "region": "US",
                  "description": None, "is_active": True}
        response = SimpleNamespace(headers={})
        with self.env("sources"), patch.object(authority.product_store, "list_records", return_value=[{"document": source}]), \
             patch.object(data_sources.db, "query") as postgres, patch.object(data_sources, "_add_table_counts", side_effect=lambda rows: rows):
            result = data_sources.list_data_sources(Mock(), response, "alice")
            self.assertEqual(result["dataSources"][0]["id"], "2")
            postgres.assert_not_called()


if __name__ == "__main__": unittest.main()
