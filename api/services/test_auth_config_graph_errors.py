"""An upstream error body is not ours to put in a response.

`routers/auth_config.py` maps a ValueError from `setup_azure_app_roles`
straight into an HTTP 400 detail, and the whole undecoded Microsoft Graph
error body used to go into it — `innerError`, `request-id`,
`client-request-id`, the timestamp and the echoed request. None of that helps
the person who pressed the button, and it is our tenant's internals in a
response body.
"""

import json
import unittest

from services import auth_config


GRAPH_BODY = json.dumps({
    "error": {
        "code": "Authorization_RequestDenied",
        "message": "Insufficient privileges to complete the operation.",
        "innerError": {
            "date": "2026-10-15T09:00:00",
            "request-id": "8f3c0d1e-0000-4a2b-9c77-1a2b3c4d5e6f",
            "client-request-id": "1b2c3d4e-1111-4f5a-8888-99aabbccddee",
        },
    }
})


class GraphReasonTests(unittest.TestCase):
    def test_only_the_message_is_repeated(self):
        reason = auth_config._graph_reason(GRAPH_BODY)
        self.assertEqual(reason, "Insufficient privileges to complete the operation.")
        for internal in ("request-id", "client-request-id", "innerError",
                         "8f3c0d1e", "1b2c3d4e", "2026-10-15T09:00:00"):
            self.assertNotIn(internal, reason)

    def test_a_body_that_is_not_graphs_shape_is_described_not_quoted(self):
        for body in ("<html><title>502 Bad Gateway</title>nginx/1.2.3</html>",
                     "", "null", "[]", json.dumps({"error": "a string"}),
                     json.dumps({"error": {"code": "x"}})):
            with self.subTest(body=body[:24]):
                reason = auth_config._graph_reason(body)
                self.assertEqual(
                    reason, "Microsoft Graph returned an error with no readable message.")

    def test_the_repeated_message_is_bounded(self):
        body = json.dumps({"error": {"message": "x" * 5000}})
        reason = auth_config._graph_reason(body)
        self.assertLessEqual(len(reason), auth_config.MAX_GRAPH_REASON_CHARS + 1)
        self.assertTrue(reason.endswith("…"))

    def test_a_multiline_message_becomes_one_line(self):
        body = json.dumps({"error": {"message": "Line one.\n\tLine two.  Line three."}})
        self.assertEqual(auth_config._graph_reason(body),
                         "Line one. Line two. Line three.")


if __name__ == "__main__":
    unittest.main()
