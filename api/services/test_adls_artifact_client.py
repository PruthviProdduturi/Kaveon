import io
from urllib.error import HTTPError
import pytest

from services.adls_artifact_client import AzureArtifactClient


class Credential:
    def get_token(self, scope):
        assert scope == "https://storage.azure.com/.default"
        return type("Token", (), {"token": "test-token"})()


class Response(io.BytesIO):
    def close(self):
        super().close()


def test_create_uses_conditional_blob_put_and_encodes_path():
    seen = {}

    def opener(request):
        seen.update(method=request.method, url=request.full_url,
                    body=request.data, headers=dict(request.headers))
        return Response()

    client = AzureArtifactClient("acct", "artifacts", Credential(), opener)
    client.create_if_absent("dataset/a b.json", b"{}")
    assert seen["method"] == "PUT"
    assert seen["url"].endswith("/artifacts/dataset/a%20b.json")
    assert seen["body"] == b"{}"
    assert seen["headers"]["If-none-match"] == "*"
    assert seen["headers"]["X-ms-blob-type"] == "BlockBlob"


def test_read_returns_bounded_content():
    def opener(request):
        assert request.method == "GET"
        assert request.headers["Range"] == "bytes=0-16"
        return Response(b"payload")

    client = AzureArtifactClient("acct", "artifacts", Credential(), opener)
    assert client.read("x", 16) == b"payload"


def test_read_zero_byte_object_omits_invalid_range():
    def opener(request):
        assert request.method == "GET"
        assert "Range" not in request.headers
        return Response(b"")

    client = AzureArtifactClient("acct", "artifacts", Credential(), opener)
    assert client.read("empty", 0) == b""


def test_http_error_preserves_read_only_status():
    failure = HTTPError("https://acct.blob.core.windows.net/artifacts/x", 416,
                        "invalid range", {}, None)

    client = AzureArtifactClient("acct", "artifacts", Credential(), lambda _: (_ for _ in ()).throw(failure))
    with pytest.raises(HTTPError) as raised:
        client.read("x", 0)
    assert raised.value is failure


def test_list_is_prefix_scoped_and_bounded():
    xml=b"""<EnumerationResults><Blobs><Blob><Name>active/head.json</Name><Properties><Etag>etag-1</Etag><Content-Length>12</Content-Length></Properties></Blob></Blobs><NextMarker /></EnumerationResults>"""
    def opener(request):
        assert "comp=list" in request.full_url and "prefix=active%2F" in request.full_url
        assert "include=metadata" in request.full_url
        return Response(xml)
    client=AzureArtifactClient("acct","state",Credential(),opener)
    assert client.list("active")==[{"path":"active/head.json","etag":"etag-1","size":12}]


def test_list_excludes_hierarchical_namespace_directory_markers():
    xml=b"""<EnumerationResults><Blobs>
    <Blob><Name>active/records</Name><Metadata><hdi_isfolder>true</hdi_isfolder></Metadata><Properties><Etag>dir-etag</Etag><Content-Length>0</Content-Length></Properties></Blob>
    <Blob><Name>active/records/empty.json</Name><Metadata /><Properties><Etag>file-etag</Etag><Content-Length>0</Content-Length></Properties></Blob>
    </Blobs><NextMarker /></EnumerationResults>"""
    client=AzureArtifactClient("acct","state",Credential(),lambda _: Response(xml))
    assert client.list("active")==[{"path":"active/records/empty.json","etag":"file-etag","size":0}]


def test_rejects_host_injection_and_path_escape_before_requesting_a_token():
    class UnusedCredential:
        def get_token(self, scope):
            raise AssertionError("credential must not be accessed for invalid destinations")
    with pytest.raises(ValueError, match="account"):
        AzureArtifactClient("evil.example/path", "state", UnusedCredential(), lambda _: None)
    with pytest.raises(ValueError, match="container"):
        AzureArtifactClient("acct", "state?comp=list", UnusedCredential(), lambda _: None)
    client = AzureArtifactClient("acct", "state", UnusedCredential(), lambda _: None)
    for path in ("../other", "/absolute", "active//head", "active/./head"):
        with pytest.raises(RuntimeError, match="path"):
            client.read(path, 10)
