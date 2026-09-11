import os, unittest
from types import SimpleNamespace
from unittest.mock import patch
from services import source_secret_store as store


class Response:
    def __init__(self,status,payload,content=b"{}"):self.status_code=status;self.payload=payload;self.content=content
    def json(self):return self.payload
class Client:
    def __init__(self,responses):self.responses=list(responses);self.calls=[]
    def request(self,*args,**kwargs):self.calls.append((args,kwargs));return self.responses.pop(0)
class Credential:
    def get_token(self,scope):self.scope=scope;return SimpleNamespace(token="token-not-a-secret-value")


class SourceSecretStoreTests(unittest.TestCase):
    def test_strict_vault_and_reference_validation(self):
        for value in ["http://x.vault.azure.net","https://vault.azure.net/path","https://evil.example"]:
            with self.assertRaises(store.SourceSecretError):store.vault_url(value)
        base="https://unit.vault.azure.net"
        for ref in ["https://other.vault.azure.net/secrets/name","https://unit.vault.azure.net/keys/name","https://unit.vault.azure.net/secrets/a?x=1","https://user@unit.vault.azure.net/secrets/a"]:
            with self.assertRaises(store.SourceSecretError):store.validate_reference(ref,base)
    def test_name_is_deterministic_and_safe(self):
        name=store.secret_name("data","42")
        self.assertEqual(name,store.secret_name("data","42"));self.assertRegex(name,r"^[A-Za-z0-9-]+$")
        self.assertNotIn("42",name)
    def test_set_get_delete_are_bounded_and_use_default_scope(self):
        base="https://unit.vault.azure.net";name=store.secret_name("data","42")
        ref=f"{base}/secrets/{name}/version1";client=Client([Response(200,{"id":ref}),Response(200,{"value":"secret"}),Response(202,{})]);credential=Credential()
        target=store.SourceSecretStore(credential=credential,client=client,configured_vault=base)
        self.assertEqual(target.set("data","42","secret"),ref);self.assertEqual(target.get(ref),"secret");self.assertIsNone(target.delete(ref))
        self.assertEqual(credential.scope,store.SCOPE);self.assertEqual([call[0][0] for call in client.calls],["PUT","GET","DELETE"])
        with self.assertRaises(store.SourceSecretError):target.set("data","42","x"*(store.MAX_SECRET_BYTES+1))
    def test_failures_never_echo_secret_token_or_response(self):
        sensitive="do-not-leak";client=Client([Response(403,{"error":sensitive},sensitive.encode())])
        target=store.SourceSecretStore(credential=Credential(),client=client,configured_vault="https://unit.vault.azure.net")
        with self.assertRaises(store.SourceSecretError) as raised:target.set("catalog","x",sensitive)
        message=str(raised.exception)
        self.assertNotIn(sensitive,message);self.assertNotIn("token-not-a-secret-value",message)
    def test_default_credential_is_lazy_and_environment_driven(self):
        with patch.dict(os.environ,{"KAVEON_KEY_VAULT_URL":"https://unit.vault.azure.net"}),patch.object(store,"DefaultAzureCredential",return_value=Credential()) as factory:
            target=store.SourceSecretStore(client=Client([]))
        self.assertEqual(target.vault,"https://unit.vault.azure.net");factory.assert_called_once_with()


if __name__=="__main__":unittest.main()
