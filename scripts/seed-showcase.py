"""Create or refresh the managed OpenSource showcase through authenticated APIs.

Requires an existing localhost portal port-forward, Azure CLI sign-in, and
Playwright. No access tokens, cookies, or source data files are written to disk.
Without --apply, validates all source queries without creating product objects.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[1]
MARKER = 'OpenSource showcase.'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--portal', default='http://127.0.0.1:13009')
    parser.add_argument('--manifest', type=Path, default=ROOT / 'scripts/showcase-queries.json')
    parser.add_argument('--report', type=Path, default=ROOT / 'tmp/showcase-result.json')
    parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text(encoding='utf-8'))
    base = args.portal.rstrip('/')
    with sync_playwright() as p:
        request = p.request.new_context(base_url=base, timeout=180000)
        config = request.get('/api/auth/entra-config').json()
        # Use the portal's configured public-client scope, never a Graph token.
        auth = subprocess.run(
            ['az.cmd' if os.name == 'nt' else 'az', 'account', 'get-access-token',
             '--tenant', config['tenantId'],
             '--scope', config['scope'], '-o', 'json'],
            capture_output=True, text=True, check=True)
        token = json.loads(auth.stdout)['accessToken']
        csrf = request.get('/api/auth/csrf').json()['csrfToken']
        response = request.post('/api/auth/callback/entra-public', form={
            'csrfToken': csrf, 'token': token, 'callbackUrl': base+'/lab'},
            headers={'X-Auth-Return-Redirect': '1'})
        if not response.ok:
            raise RuntimeError('Portal sign-in failed')
        session = request.get('/api/auth/session').json()
        if session.get('user', {}).get('role') != 'Admin':
            raise RuntimeError('An authenticated Admin is required to publish the showcase')

        def api(method, path, body=None):
            response = request.fetch('/api/kaveon/api/v1/'+path, method=method, data=body)
            if not response.ok:
                raise RuntimeError(f'{method} {path}: {response.status} {response.text()[:700]}')
            return response.json()

        sources = api('GET', 'lab/engine/sources')['sources']
        source = next(s for s in sources if s['catalog'] == manifest['catalog'])
        checks = []
        expected_rows = {}
        for dashboard in manifest['dashboards']:
            for chart in dashboard['charts']:
                result = api('POST', 'lab/query', {'query': chart['sql'],
                    'engineSourceId': source['id'], 'engineSchema': chart['schema']})
                if not result.get('success') or not result.get('rows'):
                    raise RuntimeError(f"Empty/failed showcase query: {chart['title']}")
                expected_rows[chart['slug']] = result['rows']
                actual = result['columns']
                if actual != [c['name'] for c in chart['columns']]:
                    raise RuntimeError(f"Column mismatch: {chart['title']}: {actual}")
                if any(isinstance(row[0], str) and '<a ' in row[0].lower() for row in result['rows']):
                    raise RuntimeError(f"Use plain dimension labels, not HTML: {chart['title']}")
                checks.append({'key': chart['slug'], 'rows': len(result['rows']), 'columns': actual})
                print(f"Validated: {chart['title']} ({len(result['rows'])} rows)", flush=True)
        if not args.apply:
            print('All source queries validated; use --apply to publish.')
            return

        existing = {kind: api('GET', kind) for kind in ['datasets', 'charts', 'dashboards']}
        def upsert(kind, body):
            matches = [item for item in existing[kind] if item['name'] == body['name']]
            if len(matches) > 1:
                raise RuntimeError(f"Ambiguous existing {kind}: {body['name']}")
            if matches:
                item = matches[0]
                if MARKER not in (item.get('description') or ''):
                    raise RuntimeError(f"Refusing to overwrite unmanaged {kind}: {body['name']}")
                result = api('PUT', f"{kind}/{item['id']}", body)
            else:
                result = api('POST', kind, body)
                existing[kind].append(result)
            return result

        published = []
        for dashboard in manifest['dashboards']:
            chart_ids = []
            layout = [{'i': dashboard['slug']+'-intro', 'type': 'text', 'x': 0, 'y': 0, 'w': 12, 'h': 2,
                       'textConfig': {'content': dashboard['description'], 'fontSize': 14}}]
            for index, chart in enumerate(dashboard['charts']):
                dim, metric = chart.get('dimension'), chart['metric']
                dataset = upsert('datasets', {
                    'name': 'Showcase · '+chart['title'],
                    'description': MARKER+' '+chart['description'],
                    'database_name': manifest['catalog'], 'schema_name': chart['schema'],
                    'table_name': '', 'sql_text': chart['sql'], 'visibility': 'published',
                    'columns': [{'table_name': '', 'column_name': c['name'], 'data_type': c['data_type'],
                                 'is_dimension': c['name'] == dim, 'is_metric': c['name'] == metric}
                                for c in chart['columns']]})
                config = {'dataset_id': int(dataset['id']),
                          'metrics': [{'column': metric, 'aggregate': 'MAX', 'label': metric}],
                          'groupby': [dim] if dim else [], 'row_limit': 500}
                if dim:
                    config['sort_by'] = {'column': dim if chart.get('sort_direction') == 'asc' else metric,
                                         'direction': chart.get('sort_direction', 'desc')}
                generated = api('POST', 'sql/generate', {'dataset_id': int(dataset['id']),
                    'chart_type': chart['type'], 'config': config})
                print('Checking chart: '+chart['title'], flush=True)
                result = api('POST', 'sql/engine', {'sql_text': generated['sql_text'],
                    'database': manifest['catalog'], 'dataset_id': int(dataset['id']), 'source': 'chart-builder'})
                if not result.get('rows'):
                    raise RuntimeError(f"Generated chart query has no rows: {chart['title']}")
                if sorted(result['rows'], key=repr) != sorted(expected_rows[chart['slug']], key=repr):
                    raise RuntimeError(f"Generated chart values differ from source SQL: {chart['title']}")
                saved = upsert('charts', {'name': chart['title'], 'dataset_id': int(dataset['id']),
                    'description': MARKER+' '+chart['description'], 'chart_type': chart['type'],
                    'query_config': config, 'viz_config': {}, 'visibility': 'published'})
                chart_id = str(saved['id'])
                chart_ids.append(chart_id)
                layout.append({'i': 'chart-'+str(chart_id), 'type': 'chart', 'chartId': chart_id,
                               'exemptFromFilters': True,
                               'x': (index % 2)*6, 'y': 2+(index//2)*12,
                               'w': 12 if index == len(dashboard['charts'])-1 and index % 2 == 0 else 6, 'h': 12})
            saved = upsert('dashboards', {'name': dashboard['name'],
                'description': MARKER+' '+dashboard['description'], 'layout': layout,
                'charts': chart_ids, 'filters': [], 'visibility': 'published'})
            api('PUT', 'dashboards/'+saved['id'], {'is_published': True})
            published.append({'name': saved['name'], 'id': saved['id'], 'charts': chart_ids})
            print('Published: '+saved['name'], flush=True)
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps({'catalog': manifest['catalog'], 'checks': checks,
                                          'dashboards': published}, indent=2)+'\n')
        print(f"PASS: {len(published)} dashboards published with {sum(len(d['charts']) for d in published)} charts")
        request.dispose()


if __name__ == '__main__':
    main()
