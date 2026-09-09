"""Retire only the original, unused AKS synthetic catalogs.

Run inside the API pod with its existing Engine environment. A short-lived
Storage token is read from stdin to archive definitions in ADLS before removal.
No source data files are deleted. Re-running is safe for already retired roots.
"""
import hashlib
import json
import os
import sys
import uuid

import httpx
import database.metadata as db


EXPECTED = {
    'aks-kavedb': ('kavedb', {'aks-kavedb-bronze-customers', 'aks-kavedb-bronze-orders',
                           'aks-kavedb-silver-customers', 'aks-kavedb-silver-orders',
                           'aks-kavedb-gold-daily_sales'}),
    'aks-medallion': ('medallion', {'aks-customers', 'aks-orders', 'aks-daily_sales'}),
    'aks-medallion-gold': ('medallion_gold', {'aks-gold-daily_sales'}),
}


def main():
    token = sys.stdin.readline().strip()
    if not token:
        raise SystemExit('A Storage token is required on stdin; never supply it in arguments.')
    engine = httpx.Client(base_url=os.environ['KAVEON_ENGINE_URL'],
                          verify=os.environ['KAVEON_ENGINE_CA_CERT'], timeout=60,
                          headers={'Authorization': 'Bearer '+os.environ['KAVEON_ENGINE_CATALOG_TOKEN'],
                                   'x-kaveon-actor': 'legacy-catalog-cleanup'})
    response = engine.get('/v1/catalog/definitions')
    response.raise_for_status()
    definitions = response.json()
    if not isinstance(definitions, list):
        raise RuntimeError('Unexpected catalog response')
    sources = db.query('SELECT id,name,engine_catalog,lifecycle FROM catalog_sources')['rows']
    retired_source_ids = [str(s['id']) for s in sources if s['engine_catalog'] in {v[0] for v in EXPECTED.values()}]
    # The test deployment is empty of dependent product objects. Refuse a future
    # run once users add content rather than guessing at embedded SQL references.
    for table in ('dashboards', 'charts', 'datasets', 'saved_queries', 'data_sources'):
        if db.query_one('SELECT COUNT(*) AS n FROM '+table)['n']:
            raise RuntimeError('Product objects now exist; repeat the dependency review before retiring catalogs')
    archive = []
    for definition in definitions:
        if definition['id'] not in EXPECTED:
            continue
        expected_name, expected_tables = EXPECTED[definition['id']]
        if definition['name'] != expected_name:
            raise RuntimeError('A legacy catalog has been repurposed')
        response = engine.get('/v1/catalog/definitions/'+definition['id']+'/schemas')
        response.raise_for_status()
        schemas = response.json()
        children = []
        found_tables = set()
        for schema in schemas:
            response = engine.get('/v1/catalog/schemas/'+schema['id']+'/tables')
            response.raise_for_status()
            tables = response.json()
            found_tables.update(t['id'] for t in tables)
            children.append({'schema': schema, 'tables': tables})
        if not found_tables.issubset(expected_tables):
            raise RuntimeError('Legacy catalog contains additional tables; manual dependency review required')
        archive.append({'catalog': definition, 'children': children})
    body = json.dumps({'catalogs': archive, 'platform_sources': [s for s in sources if str(s['id']) in retired_source_ids]},
                      sort_keys=True, default=str).encode()
    archive_path = 'maintenance/catalog-cleanup/'+str(uuid.uuid4())+'.json'
    storage = httpx.Client(timeout=60, headers={'Authorization':'Bearer '+token, 'x-ms-version':'2023-11-03'})
    response = storage.put('https://kvtestegmf6oweugsno.blob.core.windows.net/opensource/'+archive_path,
                           content=body, headers={'If-None-Match':'*', 'x-ms-blob-type':'BlockBlob',
                                                  'Content-Type':'application/json',
                                                  'x-ms-meta-sha256':hashlib.sha256(body).hexdigest()})
    response.raise_for_status()
    removed = []
    for entry in archive:
        definition = entry['catalog']
        response = engine.delete('/v1/catalog/definitions/'+definition['id'], params={'cascade':'true'},
                                 headers={'If-Match':str(definition['revision'])})
        response.raise_for_status()
        removed.append(definition['name'])
    for source_id in retired_source_ids:
        db.execute("UPDATE catalog_sources SET lifecycle='deleted', modified_by='legacy-catalog-cleanup', modified_at=NOW() WHERE id=@param0", [source_id])
    db.execute("INSERT INTO activity (action,object_type,object_id,object_name,user_email,details) VALUES (@param0,@param1,@param2,@param3,@param4,@param5)",
               ['retire', 'catalog_cleanup', str(uuid.uuid4()), 'Legacy demo catalogs', 'system',
                json.dumps({'removed':removed, 'archive':archive_path, 'source_files_deleted':False})])
    print(json.dumps({'removed_catalogs':removed, 'retired_platform_sources':len(retired_source_ids),
                      'archive':archive_path, 'source_files_deleted':False}))


if __name__ == '__main__':
    main()
