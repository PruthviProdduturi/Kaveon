"""Exercise original product dashboards and their filters against the live Engine.

Authentication stays in memory. Reports contain query IDs/counts and screenshots,
never credentials or exported source rows.
"""
import argparse
import copy
import json
import os
import subprocess
import time
from pathlib import Path

from playwright.sync_api import sync_playwright


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--portal', required=True)
    parser.add_argument('--report', required=True, help='Importer result JSON')
    parser.add_argument('--output', default='tmp/rich-dashboard-browser-validation.json')
    args = parser.parse_args()
    definitions = json.loads(Path(args.report).read_text())
    templates = json.loads(Path('data/dashboard-templates/original-dashboard-templates.json').read_text())['templates']['product']
    chart_templates = {chart['logical_ref']: chart for chart in templates['charts']}
    evidence = []
    with sync_playwright() as p:
        browser = p.chromium.launch(channel='msedge', headless=True)
        context = browser.new_context(viewport={'width': 1600, 'height': 1200})
        base = args.portal.rstrip('/')
        config = context.request.get(base+'/api/auth/entra-config').json()
        auth = subprocess.run(['az.cmd' if os.name == 'nt' else 'az', 'account',
            'get-access-token', '--tenant', config['tenantId'], '--scope', config['scope'],
            '-o', 'json'], capture_output=True, text=True, check=True)
        csrf = context.request.get(base+'/api/auth/csrf').json()['csrfToken']
        context.request.post(base+'/api/auth/callback/entra-public', form={
            'csrfToken': csrf, 'token': json.loads(auth.stdout)['accessToken'],
            'callbackUrl': base+'/dashboards'}, headers={'X-Auth-Return-Redirect': '1'})
        assert context.request.get(base+'/api/auth/session').json().get('user', {}).get('role') == 'Admin'
        for dashboard in definitions['dashboards']:
            original = next(d for d in templates['dashboards'] if d['name'] == dashboard['name'])
            refs = dict(zip(original['chart_refs'], dashboard['charts'], strict=True))
            saved = context.request.get(base+'/api/kaveon/api/v1/dashboards/'+dashboard['id']).json()
            assert saved.get('is_published') is True, 'Dashboard is still a draft: '+dashboard['name']
            for field in ('layout', 'filters'):
                if isinstance(saved[field], str):
                    saved[field] = json.loads(saved[field])
            expected_layout = copy.deepcopy(original['layout'])
            expected_layout[0]['_kaveon_template_ref'] = original['logical_ref']
            for item in expected_layout:
                if 'chart_ref' in item:
                    item['chartId'] = refs[item.pop('chart_ref')]
            expected_filters = copy.deepcopy(original['filters'])
            for item in expected_filters:
                item.pop('dataset_ref')
                item['datasetId'] = definitions['dataset']['id']
            assert saved['layout'] == expected_layout, {'message': 'Saved layout differs from original template',
                'first_saved': saved['layout'][0], 'first_expected': expected_layout[0]}
            assert saved['filters'] == expected_filters, 'Saved filters differ from original template'
            for ref, chart_id in refs.items():
                source = chart_templates[ref]
                target = context.request.get(base+'/api/kaveon/api/v1/charts/'+chart_id).json()
                expected_config = copy.deepcopy(source['query_config'])
                expected_config.pop('dataset_ref')
                expected_config.pop('datasource', None)
                expected_config.update(dataset_id=definitions['dataset']['id'], _kaveon_template_ref=ref)
                assert target['query_config'] == expected_config, 'Saved query configuration differs: '+source['name']
                assert target['viz_config'] == source['viz_config'], 'Saved styling differs: '+source['name']
                assert target['chart_type'] == source['chart_type'], 'Saved chart type differs: '+source['name']
            page = context.new_page()
            errors, failures, queries, generated = [], [], [], []
            page.on('pageerror', lambda error: errors.append(str(error)))

            def capture(response):
                if '/api/v1/' not in response.url:
                    return
                if response.status >= 400:
                    failures.append({'path': response.url.split('/api/v1/')[-1], 'status': response.status,
                                     'request': response.request.post_data_json})
                if response.url.endswith('/sql/generate') and response.ok:
                    generated.append(response.json()['sql_text'])
                if response.url.endswith('/sql/engine') and response.ok:
                    result = response.json()
                    body = response.request.post_data_json or {}
                    queries.append({'chart_id': str(body.get('chart_id')), 'rows': result.get('rows', []),
                                    'query_id': result.get('query_id'), 'sql': body.get('sql_text', '')})

            page.on('response', capture)
            chart_count = len(dashboard['charts'])

            def settle(start):
                deadline = time.monotonic()+240
                while time.monotonic() < deadline:
                    assert not failures and not errors, {'api': failures, 'browser': errors}
                    if len(queries)-start >= chart_count:
                        page.wait_for_timeout(1200)
                        break
                    page.wait_for_timeout(500)
                assert len(queries)-start >= chart_count, {'dashboard': dashboard['name'], 'received': len(queries)-start,
                                                        'text': page.locator('body').inner_text()[-1800:]}
                batch = queries[start:]
                assert all(q['rows'] and q['query_id'] for q in batch), 'Empty or untraced chart result'
                return {q['chart_id']: q['rows'] for q in batch}

            page.goto(base+'/dashboards/'+dashboard['id']+'/view', wait_until='domcontentloaded')
            baseline = settle(0)
            print('Rendered baseline: '+dashboard['name'], flush=True)
            baseline_sql = sorted(generated)
            kpi_values = page.locator('.chart-builder-preview-inner div[style*="font-variant-numeric: tabular-nums"]')
            baseline_kpis = kpi_values.all_text_contents()
            assert len(baseline_kpis) == 4

            def reset_and_check():
                # Clear can legitimately reuse the browser's result cache.
                # Verify regenerated baseline SQL and actual rendered KPI values.
                start = len(generated)
                page.locator('.chart-filter-card button').filter(has_text='Clear').click()
                deadline = time.monotonic()+90
                while time.monotonic() < deadline:
                    assert not failures and not errors, {'api': failures, 'browser': errors}
                    if len(generated)-start >= chart_count and kpi_values.all_text_contents() == baseline_kpis:
                        break
                    page.wait_for_timeout(300)
                assert sorted(generated[start:]) == baseline_sql, 'Clear did not restore original SQL'
                assert kpi_values.all_text_contents() == baseline_kpis, 'Clear did not restore rendered values'

            assert page.locator('.chart-filter-chip').count() == 12, page.locator('.chart-filter-card').all_text_contents()
            assert page.locator('.dashboard-chart-component').count() == chart_count
            assert page.locator('canvas').count() >= chart_count-5, 'Chart plots did not render'
            assert page.get_by_text('This is synthetic data generated to showcase the platform.', exact=False).count()
            assert not page.get_by_text('Configure your chart to see preview', exact=False).count()
            # Open every categorical control: actual options must be available.
            option_counts = {}
            for index in range(11):
                chip = page.locator('.chart-filter-chip').nth(index)
                label = chip.inner_text()
                chip.click()
                page.locator('.chart-filter-popover input[type=checkbox]').first.wait_for(timeout=90000)
                option_counts[label] = page.locator('.chart-filter-popover input[type=checkbox]').count()
                page.get_by_role('button', name='Cancel', exact=True).click()
            print('Loaded all 11 categorical controls: '+dashboard['name'], flush=True)
            # A real license selection must rerun all charts and change values.
            page.locator('.chart-filter-chip').first.click()
            page.locator('.chart-filter-popover input[type=checkbox]').first.check()
            start = len(queries)
            page.get_by_role('button', name='Apply', exact=True).click()
            filtered = settle(start)
            assert filtered != baseline, 'License filter did not change chart results'
            print('License filter changed results: '+dashboard['name'], flush=True)
            reset_and_check()
            # Inclusive date bounds must reach Engine, not just change the chip.
            page.locator('.chart-filter-chip').nth(11).click()
            page.locator('input[type=date]').nth(0).fill('2026-01-01')
            page.locator('input[type=date]').nth(1).fill('2026-01-31')
            start = len(queries)
            page.get_by_role('button', name='Apply', exact=True).click()
            dated = settle(start)
            assert dated != baseline, 'Date filter did not change chart results'
            assert all('2026-01-01' in q['sql'] and '2026-01-31' in q['sql'] for q in queries[start:])
            reset_and_check()
            shot = Path('tmp')/('rich-dashboard-'+dashboard['id']+'.png')
            page.wait_for_timeout(2000)  # Let cached-result chart transitions finish.
            page.screenshot(path=str(shot), full_page=True)
            evidence.append({'id': dashboard['id'], 'name': dashboard['name'], 'chart_count': chart_count,
                'template_configuration_match': True,
                'filter_options': option_counts, 'license_filter': True, 'date_filter': True, 'reset': True,
                'queries': [{'chart_id': q['chart_id'], 'query_id': q['query_id'], 'rows': len(q['rows'])} for q in queries],
                'screenshot': str(shot)})
            Path(args.output).write_text(json.dumps(evidence, indent=2)+'\n')
            print('PASS: '+dashboard['name']+'; charts, 12 controls, license/date filters and reset', flush=True)
            page.close()
        browser.close()


if __name__ == '__main__':
    main()
