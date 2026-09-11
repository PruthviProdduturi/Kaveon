import tempfile
import unittest
from pathlib import Path

from services import postgresql_dependency_inventory as inventory
from services.postgresql_retirement_gate import AUTHORITY_FAMILIES


API_ROOT = Path(__file__).resolve().parents[1]


class DependencyInventoryTests(unittest.TestCase):
    def test_repository_call_sites_are_completely_classified(self):
        result = inventory.scan(API_ROOT)
        self.assertEqual(result["family_count"], len(AUTHORITY_FAMILIES))
        self.assertEqual({item["family"] for item in result["families"]}, set(AUTHORITY_FAMILIES))
        self.assertNotIn("connection_string", str(result).lower())

    def test_new_unclassified_authority_reference_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "rogue.py").write_text(
                'def read(db):\n    return db.query("SELECT * FROM datasets")\n', encoding="utf-8"
            )
            with self.assertRaisesRegex(RuntimeError, "rogue.py has unclassified.*datasets"):
                inventory.scan(root, {})

    def test_partial_classification_fails_for_other_observed_family(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "mixed.py").write_text(
                'def read(db):\n    return db.query("SELECT * FROM charts JOIN datasets ON true")\n',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(RuntimeError, "unclassified authority families: datasets"):
                inventory.scan(root, {"mixed.py": {"charts": "read"}})

    def test_missing_file_and_invalid_access_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "one.py").write_text(
                'def read(db):\n    return db.query("SELECT * FROM datasets")\n', encoding="utf-8"
            )
            with self.assertRaisesRegex(RuntimeError, "missing file"):
                inventory.scan(root, {"missing.py": {"datasets": "read"}, "one.py": {"datasets": "read"}})
            with self.assertRaisesRegex(RuntimeError, "invalid classification"):
                inventory.scan(root, {"one.py": {"datasets": "maybe"}})

    def test_tests_and_plain_text_mentions_are_not_authority_calls(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "notes.py").write_text('TABLE = "datasets"\n', encoding="utf-8")
            (root / "test_rogue.py").write_text(
                'def x(db): return db.query("SELECT * FROM datasets")\n', encoding="utf-8"
            )
            with self.assertRaisesRegex(RuntimeError, "has no classified application call site"):
                inventory.scan(root, {})


if __name__ == "__main__":
    unittest.main()
