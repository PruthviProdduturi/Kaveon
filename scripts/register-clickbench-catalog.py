"""Register the ClickBench `hits` object as Benchmarks.clickbench.hits.

Runs from an API pod with the Engine catalog credential, like
register-curated-catalog.py. The column list is the upstream Trino schema
(docs/qualification/clickbench/trino-create.sql) mapped to Arrow types; the
registration is verified by an exact COUNT(*) against the published row count.
No credentials or rows are printed.
"""
import json
import os
import re
import sys
from pathlib import Path
from urllib.parse import quote

import httpx

CATALOG = 'Benchmarks'
CATALOG_ID = 'aks-benchmarks'
ROOT = 'benchmarks'
SCHEMA = 'clickbench'
TABLE = 'hits'
LOCATION = 'clickbench/hits.parquet'
ROW_COUNT = 99_997_497
ARROW = {'bigint': 'Int64', 'integer': 'Int32', 'smallint': 'Int16', 'varchar': 'Utf8', 'date': 'Date32'}

ACCOUNT = os.environ['KAVEON_LAKE_ADLS_ACCOUNT']
ADLS = {'account': ACCOUNT, 'container': 'opensource', 'root_path': ROOT}
client = httpx.Client(base_url=os.environ['KAVEON_ENGINE_URL'],
                      verify=os.environ.get('KAVEON_ENGINE_CA_CERT') or True, timeout=600,
                      headers={'Authorization': 'Bearer ' + os.environ['KAVEON_ENGINE_CATALOG_TOKEN'],
                               'x-kaveon-actor': 'benchmark-curation'})


def columns_from_create(path: Path) -> list[dict]:
    text = path.read_text(encoding='utf-8')
    body = text[text.index('hits_raw (') + len('hits_raw ('):]
    body = body[:body.index(')\nWITH')] if ')\nWITH' in body else body[:body.index(');')]
    columns = []
    for line in body.splitlines():
        match = re.match(r'\s*(\w+)\s+(bigint|integer|smallint|varchar|date)\s*,?\s*$', line)
        if match:
            columns.append({'name': match.group(1), 'data_type': ARROW[match.group(2)], 'nullable': True})
    if len(columns) != 105:
        raise SystemExit(f'expected 105 columns, parsed {len(columns)}')
    return columns


def register(collection, path, body):
    existing = client.get(path)
    if existing.status_code == 404:
        response = client.post(collection, json={**body, 'revision': 1, 'lifecycle': 'Draft'})
        response.raise_for_status()
        current = response.json()
    else:
        existing.raise_for_status()
        current = existing.json()
        for key, value in body.items():
            if current.get(key) != value:
                raise RuntimeError(f'Existing definition differs: {path}, field {key}')
    if current['lifecycle'] != 'Active':
        revision = current['revision']
        response = client.put(path, headers={'If-Match': str(revision)},
                              json={**body, 'revision': revision + 1, 'lifecycle': 'Active'})
        response.raise_for_status()


def main() -> int:
    columns = columns_from_create(Path(sys.argv[1]) if len(sys.argv) > 1
                                  else Path('/scripts/trino-create.sql'))
    register('/v1/catalog/definitions', '/v1/catalog/definitions/' + CATALOG_ID,
             {'id': CATALOG_ID, 'name': CATALOG, 'storage': {'AdlsGen2': ADLS},
              'credential': {'kind': 'WorkloadIdentity', 'reference': 'kaveon-test-reader'}})
    schema_id = CATALOG_ID + '-' + SCHEMA
    register(f'/v1/catalog/definitions/{CATALOG_ID}/schemas', '/v1/catalog/schemas/' + schema_id,
             {'id': schema_id, 'catalog_id': CATALOG_ID, 'name': SCHEMA})
    table_id = schema_id + '-' + TABLE
    register('/v1/catalog/schemas/' + quote(schema_id, safe='') + '/tables',
             '/v1/catalog/tables/' + quote(table_id, safe=''),
             {'id': table_id, 'schema_id': schema_id, 'name': TABLE, 'location': LOCATION,
              'access': 'Shortcut', 'format': 'Parquet', 'columns': columns})
    response = client.post('/v1/statement', headers={
        'Authorization': 'Bearer ' + os.environ['KAVEON_ENGINE_BRIDGE_TOKEN'],
        'x-kaveon-principal': 'benchmark-curation', 'x-kaveon-role': 'admin',
    }, json={'query': f'SELECT COUNT(*) FROM {SCHEMA}.{TABLE}', 'catalog': CATALOG,
             'schema': SCHEMA, 'source': 'http-api'})
    response.raise_for_status()
    result = response.json()
    rows = result.get('data', result.get('rows'))
    if result.get('error') or rows != [[ROW_COUNT]]:
        raise RuntimeError(f'Engine row-count mismatch: {result.get("error", rows)}')
    print(json.dumps({'catalog': CATALOG, 'table': f'{SCHEMA}.{TABLE}', 'rows': ROW_COUNT,
                      'elapsed_ms': result.get('elapsed_ms')}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
