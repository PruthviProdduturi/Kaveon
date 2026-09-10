import unittest
from unittest.mock import Mock

import requests

from transaction_compare import Kaveon, document_hash, state_hash, summarize


class FakeResponse:
    def __init__(self, status, body): self.status_code, self.body = status, body
    def raise_for_status(self):
        if self.status_code >= 400:
            error = requests.HTTPError(str(self.status_code)); error.response = self; raise error
    def json(self): return self.body


class FakeSession:
    def __init__(self, response): self.response, self.calls = response, []
    def get(self, url, **kwargs): self.calls.append(("GET", url, kwargs)); return self.response


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

    def test_point_read_uses_only_bounded_get_and_never_begins_transaction(self):
        client = Kaveon("https://engine.example", "token")
        transport = FakeSession(FakeResponse(200, {"id": "orders/1", "revision": 2,
                                                    "document": {"name": "Orders"}}))
        client.session = transport
        client.begin = Mock(side_effect=AssertionError("point read must not BEGIN"))
        self.assertEqual(client.point("dataset", "orders/1"),
                         ["orders/1", 2, {"name": "Orders"}])
        self.assertEqual(len(transport.calls), 1)
        self.assertIn("/v1/product/dataset/orders%2F1", transport.calls[0][1])
        client.begin.assert_not_called()

    def test_missing_bounded_endpoint_fails_closed(self):
        client = Kaveon("https://engine.example", "token")
        client.session = FakeSession(FakeResponse(404, {"error": "product record not found"}))
        client.begin = Mock(side_effect=AssertionError("fallback snapshot scan forbidden"))
        with self.assertRaises(requests.HTTPError):
            client.point("dataset", "missing")
        client.begin.assert_not_called()


if __name__ == "__main__":
    unittest.main()
