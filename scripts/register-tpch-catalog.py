"""Register the generated TPC-H tables as Benchmarks.tpch_sf<N>.<table>.

Runs from an API pod with the Engine catalog credential, like
register-clickbench-catalog.py. The manifest is the generation Job's
output (docs/qualification/tpch/tables.json): Trino column types mapped to
Arrow types, and the exact row count each registration is verified against
with COUNT(*). No credentials or rows are printed.

    python register-tpch-catalog.py /scripts/tables.json
"""
import json
import os
import sys
from pathlib import Path
from urllib.parse import quote

import httpx

CATALOG = 'Benchmarks'
CATALOG_ID = 'aks-benchmarks'
ROOT = 'benchmarks'
ARROW = {'bigint': 'Int64', 'integer': 'Int32', 'smallint': 'Int16', 'double': 'Float64',
         'varchar': 'Utf8', 'date': 'Date32'}

ACCOUNT = os.environ['KAVEON_LAKE_ADLS_ACCOUNT']
ADLS = {'account': ACCOUNT, 'container': 'opensource', 'root_path': ROOT}
client = httpx.Client(base_url=os.environ['KAVEON_ENGINE_URL'],
                      verify=os.environ.get('KAVEON_ENGINE_CA_CERT') or True, timeout=1800,
                      headers={'Authorization': 'Bearer ' + os.environ['KAVEON_ENGINE_CATALOG_TOKEN'],
                               'x-kaveon-actor': 'benchmark-curation'})


def arrow_type(trino_type: str) -> str:
    base = trino_type.split('(')[0].strip().lower()
    if base not in ARROW:
        raise SystemExit(f'unsupported Trino type in manifest: {trino_type}')
    return ARROW[base]


def register(collection, path, body):
    existing = client.get(path)
    if existing.status_code == 404:
        response = client.post(collection, json={**body, 'revision': 1, 'lifecycle': 'Draft'})
        response.raise_for_status()
        current = response.json()
    else:
        existing.raise_for_status()
        current = existing.json()
    # The manifest is the source of truth for this catalog: a definition
    # that differs from it (an earlier generation's layout, say) is revised.
    changed = [key for key, value in body.items() if current.get(key) != value]
    if changed or current['lifecycle'] != 'Active':
        revision = current['revision']
        response = client.put(path, headers={'If-Match': str(revision)},
                              json={**body, 'revision': revision + 1, 'lifecycle': 'Active'})
        response.raise_for_status()
        if changed:
            print(json.dumps({'revised': path, 'fields': changed}), flush=True)


def main() -> int:
    manifest = json.loads(Path(sys.argv[1] if len(sys.argv) > 1 else '/scripts/tables.json').read_text(encoding='utf-8'))
    # The generation Job prints {"scale": N, "tables": [...]} (kept as
    # docs/qualification/tpch/tables.json); its TPCH_OUTPUT file is the bare list.
    if isinstance(manifest, dict):
        manifest = manifest['tables']
    register('/v1/catalog/definitions', '/v1/catalog/definitions/' + CATALOG_ID,
             {'id': CATALOG_ID, 'name': CATALOG, 'adapter': 'Native', 'storage': {'AdlsGen2': ADLS},
              'credential': {'kind': 'WorkloadIdentity', 'reference': 'kaveon-test-reader'}})
    for table in manifest:
        schema = table['schema']
        schema_id = CATALOG_ID + '-' + schema
        register(f'/v1/catalog/definitions/{CATALOG_ID}/schemas', '/v1/catalog/schemas/' + schema_id,
                 {'id': schema_id, 'catalog_id': CATALOG_ID, 'name': schema})
        table_id = schema_id + '-' + table['name']
        # The directory is container-relative in the manifest; the catalog
        # root is `benchmarks`, so the location is the remainder.
        location = table['directory'].removeprefix(ROOT + '/').rstrip('/')
        columns = [{'name': name, 'data_type': arrow_type(kind), 'nullable': True}
                   for name, kind in table['columns']]
        register('/v1/catalog/schemas/' + quote(schema_id, safe='') + '/tables',
                 '/v1/catalog/tables/' + quote(table_id, safe=''),
                 {'id': table_id, 'schema_id': schema_id, 'name': table['name'], 'location': location,
                  'access': 'Shortcut', 'format': 'Delta' if table.get('format') == 'delta' else 'Parquet',
                  'columns': columns})
        response = client.post('/v1/statement', headers={
            'Authorization': 'Bearer ' + os.environ['KAVEON_ENGINE_BRIDGE_TOKEN'],
            'x-kaveon-principal': 'benchmark-curation', 'x-kaveon-role': 'admin',
        }, json={'query': f"SELECT COUNT(*) FROM {schema}.{table['name']}", 'catalog': CATALOG,
                 'schema': schema, 'source': 'http-api'})
        response.raise_for_status()
        result = response.json()
        rows = result.get('data', result.get('rows'))
        if result.get('error') or rows != [[table['rows']]]:
            raise RuntimeError(f"Engine row-count mismatch for {table['name']}: {result.get('error', rows)}")
        print(json.dumps({'catalog': CATALOG, 'table': f"{schema}.{table['name']}", 'rows': table['rows'],
                          'elapsed_ms': result.get('elapsed_ms')}), flush=True)
    return 0


if __name__ == '__main__':
    sys.exit(main())
