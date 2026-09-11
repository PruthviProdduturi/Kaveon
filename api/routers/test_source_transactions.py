import sys, unittest
from types import SimpleNamespace
from unittest.mock import patch
sys.modules.setdefault("pyodbc",SimpleNamespace(Error=Exception))
from fastapi import HTTPException
from routers import catalog_sources, data_sources
from services import source_mutations


class Transaction:
    def __init__(self, rows=()): self.rows=list(rows);self.calls=[];self.committed=False;self.rolled_back=False
    def __enter__(self): return self
    def __exit__(self,typ,value,tb): self.rolled_back=typ is not None;self.committed=typ is None
    def query_one(self,sql,params=None): self.calls.append(("query",sql,params));return self.rows.pop(0) if self.rows else None
    def execute(self,sql,params=None): self.calls.append(("execute",sql,params));return 1


class SourceTransactionTests(unittest.TestCase):
    def test_data_create_outbox_failure_rolls_back_and_payload_is_public(self):
        row={"id":7,"name":"db","type":"PostgreSQL","database_name":"db","region":"WW","description":None,"created_by":"owner","is_active":True}
        tx=Transaction([row]);captured=[]
        def fail(transaction,family,operation,value,actor):
            captured.append(source_mutations.data_document(value));raise RuntimeError("outbox failed")
        with patch.object(data_sources.db,"transaction",return_value=tx),patch.object(data_sources,"encrypt",return_value="ciphertext"),patch.object(data_sources.source_mutations,"enqueue",side_effect=fail):
            with self.assertRaises(HTTPException) as raised:
                data_sources.create_data_source({"name":"db","type":"PostgreSQL","connection_string":"raw-secret","database_name":"db","region":"WW"},SimpleNamespace(email="owner"))
        self.assertEqual(raised.exception.status_code,500)
        self.assertTrue(tx.rolled_back);self.assertFalse(tx.committed)
        self.assertNotIn("raw-secret",str(captured));self.assertNotIn("ciphertext",str(captured))

    def test_data_delete_restricts_favorites_before_source_or_outbox_delete(self):
        row={"id":7,"name":"db","type":"PostgreSQL","database_name":"db","region":"WW","description":None,"created_by":"owner","is_active":True}
        tx=Transaction([row,{"count":1}])
        with patch.object(data_sources.db,"transaction",return_value=tx),patch.object(data_sources.source_mutations,"enqueue") as enqueue:
            with self.assertRaises(HTTPException) as raised:data_sources.delete_data_source("7",SimpleNamespace(email="owner"))
        self.assertEqual(raised.exception.status_code,409);self.assertTrue(tx.rolled_back);enqueue.assert_not_called()
        self.assertFalse(any(call[0]=="execute" and "DELETE FROM data_sources" in call[1] for call in tx.calls))

    def test_catalog_transition_rejects_stale_state_under_lock(self):
        initial={"id":"c","name":"Lake","lifecycle":"active"};locked={**initial,"lifecycle":"suspended"}
        tx=Transaction([locked])
        with patch.object(catalog_sources.db,"query_one",return_value=initial),patch.object(catalog_sources.db,"transaction",return_value=tx),patch.object(catalog_sources.source_mutations,"enqueue") as enqueue:
            with self.assertRaises(HTTPException) as raised:catalog_sources.transition_lifecycle("c",{"lifecycle":"suspended"},SimpleNamespace(email="owner"))
        self.assertEqual(raised.exception.status_code,409);self.assertTrue(tx.rolled_back);enqueue.assert_not_called()

    def test_catalog_document_rejects_non_key_vault_credential(self):
        with self.assertRaisesRegex(ValueError,"Key Vault"):
            source_mutations.catalog_document({"id":"c","credential_ref":"plaintext"})


if __name__ == "__main__": unittest.main()
