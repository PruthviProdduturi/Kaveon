import hashlib
import tempfile
import unittest
from pathlib import Path

from services.dlm_artifact_publisher import MAX_ARTIFACT_BYTES, Publisher


class Client:
    def __init__(self, *, create_error=None, remote=None):
        self.create_error, self.remote, self.creates = create_error, remote, []

    def create_if_absent(self, path, content):
        self.creates.append((path, content))
        if self.create_error:
            raise self.create_error
        self.remote = content

    def read(self, path, max_bytes):
        return self.remote


class DlmArtifactPublisherTests(unittest.TestCase):
    def test_create_only_publish_and_exact_reconciliation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "dlm" / "7" / "v2" / "manifest.json"
            source.parent.mkdir(parents=True)
            source.write_bytes(b'{"schema":1}')
            digest = hashlib.sha256(source.read_bytes()).hexdigest()
            client = Client()
            result = Publisher(client, root).publish("dlm/7/v2/manifest.json", digest)
        self.assertEqual((result.disposition, result.sha256), ("created", digest))
        self.assertEqual(client.creates, [("dlm/7/v2/manifest.json", b'{"schema":1}')])

    def test_ambiguous_create_resolves_only_when_remote_is_exact(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "dlm" / "7" / "v2" / "manifest.json"
            source.parent.mkdir(parents=True)
            source.write_bytes(b"exact")
            digest = hashlib.sha256(b"exact").hexdigest()
            result = Publisher(Client(create_error=RuntimeError("ambiguous"), remote=b"exact"), root).publish(
                "dlm/7/v2/manifest.json", digest)
            self.assertEqual(result.disposition, "already_present")
            with self.assertRaisesRegex(RuntimeError, "ambiguous"):
                Publisher(Client(create_error=RuntimeError("ambiguous"), remote=b"other"), root).publish(
                    "dlm/7/v2/manifest.json", digest)

    def test_missing_hash_size_path_and_post_create_divergence_fail_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "artifact"
            source.write_bytes(b"value")
            publisher = Publisher(Client(), root)
            for path, digest, message in (("missing", "0" * 64, "missing"),
                                           ("artifact", "0" * 64, "hash mismatch"),
                                           ("../artifact", hashlib.sha256(b"value").hexdigest(), "path")):
                with self.subTest(message=message), self.assertRaisesRegex(RuntimeError, message):
                    publisher.publish(path, digest)
            source.write_bytes(b"x" * (MAX_ARTIFACT_BYTES + 1))
            with self.assertRaisesRegex(RuntimeError, "byte bound"):
                publisher.publish("artifact", hashlib.sha256(source.read_bytes()).hexdigest())
            source.write_bytes(b"value")

            class Divergent(Client):
                def create_if_absent(self, path, content): self.remote = b"changed"

            with self.assertRaisesRegex(RuntimeError, "reconciliation"):
                Publisher(Divergent(), root).publish("artifact", hashlib.sha256(b"value").hexdigest())


if __name__ == "__main__": unittest.main()
