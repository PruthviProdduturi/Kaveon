import unittest

from transaction_compare import document_hash, state_hash, summarize


class TransactionComparisonTests(unittest.TestCase):
    def test_state_hash_is_order_independent_and_value_sensitive(self):
        left = [["b", 1, "bbb"], ["a", 1, "aaa"]]
        self.assertEqual(state_hash(left), state_hash(list(reversed(left))))
        self.assertNotEqual(state_hash(left), state_hash([["a", 2, "aaa"], ["b", 1, "bbb"]]))

    def test_document_hash_uses_canonical_object_key_order(self):
        self.assertEqual(document_hash({"b": 2, "a": 1}), document_hash({"a": 1, "b": 2}))

    def test_summary_requires_samples_and_matching_state(self):
        good = summarize("insert", [2.0] * 30, [3.0] * 30, "abc", "abc")
        self.assertTrue(good["passed"])
        self.assertEqual(good["samples"], 30)
        self.assertEqual(good["latency_ms"]["kaveon"]["p95"], 2.0)
        self.assertFalse(summarize("insert", [2.0], [3.0], "abc", "def")["passed"])


if __name__ == "__main__":
    unittest.main()
