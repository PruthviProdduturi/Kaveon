import io

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


def test_list_is_prefix_scoped_and_bounded():
    xml=b"""<EnumerationResults><Blobs><Blob><Name>active/head.json</Name><Properties><Etag>etag-1</Etag><Content-Length>12</Content-Length></Properties></Blob></Blobs><NextMarker /></EnumerationResults>"""
    def opener(request):
        assert "comp=list" in request.full_url and "prefix=active%2F" in request.full_url
        return Response(xml)
    client=AzureArtifactClient("acct","state",Credential(),opener)
    assert client.list("active")==[{"path":"active/head.json","etag":"etag-1","size":12}]
