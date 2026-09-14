import os
import unittest
from unittest.mock import patch

from services import product_read_authority, product_store, theme


class ThemeKaveonDBCutoverTests(unittest.TestCase):
    def environment(self):
        return patch.dict(os.environ, {product_read_authority.ENVIRONMENT_KEY: "user_themes"}, clear=True)

    def test_create_writes_directly_without_postgresql(self):
        with self.environment(), patch.object(product_store, "read", return_value=None), \
             patch.object(product_store, "transact") as transact, patch.object(theme.db, "transaction") as postgres:
            theme.save_user_theme("alice@example.test", "#A1B2C3")
        mutation = transact.call_args.args[0][0]
        self.assertEqual(mutation.operation, "create"); self.assertEqual(mutation.document["theme_color"], "#a1b2c3")
        self.assertIsNone(mutation.expected_revision); postgres.assert_not_called()

    def test_update_uses_exact_current_revision(self):
        with self.environment(), patch.object(product_store, "read", return_value={"revision": 7}), \
             patch.object(product_store, "transact") as transact, patch.object(theme.db, "transaction") as postgres:
            theme.save_user_theme("alice@example.test", "#010203")
        mutation = transact.call_args.args[0][0]
        self.assertEqual((mutation.operation, mutation.expected_revision), ("update", 7)); postgres.assert_not_called()

    def test_delete_is_idempotent_and_cas_bound(self):
        with self.environment(), patch.object(product_store, "read", return_value=None), \
             patch.object(product_store, "transact") as transact, patch.object(theme.db, "transaction") as postgres:
            theme.delete_user_theme("alice@example.test")
        transact.assert_not_called(); postgres.assert_not_called()
        with self.environment(), patch.object(product_store, "read", return_value={"revision": 3}), \
             patch.object(product_store, "transact") as transact, patch.object(theme.db, "transaction") as postgres:
            theme.delete_user_theme("alice@example.test")
        mutation = transact.call_args.args[0][0]
        self.assertEqual((mutation.operation, mutation.expected_revision), ("delete", 3)); postgres.assert_not_called()

    def test_invalid_revision_and_color_fail_before_postgresql(self):
        with self.environment(), patch.object(product_store, "read", return_value={"revision": 0}), \
             patch.object(theme.db, "transaction") as postgres:
            with self.assertRaisesRegex(RuntimeError, "revision"):
                theme.save_user_theme("alice@example.test", "#ffffff")
            postgres.assert_not_called()
        with self.environment(), patch.object(product_store, "read") as read:
            with self.assertRaises(ValueError): theme.save_user_theme("alice@example.test", "red")
            read.assert_not_called()


if __name__ == "__main__": unittest.main()
