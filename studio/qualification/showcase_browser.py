"""Verify every seeded showcase dashboard using a real authenticated browser."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time
from playwright.sync_api import sync_playwright

parser = argparse.ArgumentParser()
parser.add_argument('--portal', default='http://127.0.0.1:13010')
parser.add_argument('--report', default='tmp/showcase-result.json')
args = parser.parse_args()
report = json.loads(Path(args.report).read_text())
with sync_playwright() as p:
    browser = p.chromium.launch(channel='msedge', headless=True)
    context = browser.new_context(viewport={'width': 1440, 'height': 1100})
    base = args.portal.rstrip('/')
    config = context.request.get(base+'/api/auth/entra-config').json()
    auth = subprocess.run(['az.cmd' if os.name == 'nt' else 'az', 'account', 'get-access-token',
        '--tenant', config['tenantId'], '--scope', config['scope'], '-o', 'json'],
        capture_output=True, text=True, check=True)
    token = json.loads(auth.stdout)['accessToken']
    csrf = context.request.get(base+'/api/auth/csrf').json()['csrfToken']
    context.request.post(base+'/api/auth/callback/entra-public', form={
        'csrfToken': csrf, 'token': token, 'callbackUrl': base+'/dashboards'},
        headers={'X-Auth-Return-Redirect': '1'})
    assert context.request.get(base+'/api/auth/session').json().get('user', {}).get('role') == 'Admin'
    evidence = []
    for dashboard in report['dashboards']:
        page = context.new_page()
        errors, queries, failed = [], [], []
        page.on('pageerror', lambda error: errors.append(str(error)))
        def capture(response):
            if '/api/v1/' not in response.url:
                return
            if response.status >= 400:
                failed.append({'path': response.url.split('/api/v1/')[-1], 'status': response.status})
            if response.url.endswith('/sql/engine'):
                result = response.json()
                queries.append({'columns': result.get('columns'), 'rows': len(result.get('rows', [])),
                                'query_id': result.get('query_id')})
        page.on('response', capture)
        page.goto(base+'/dashboards/'+dashboard['id']+'/view', wait_until='domcontentloaded')
        deadline = time.monotonic()+120
        while len(queries) < len(dashboard['charts']) and time.monotonic() < deadline and not failed:
            page.wait_for_timeout(500)
        assert not failed, failed
        assert len(queries) == len(dashboard['charts']), {'dashboard': dashboard['name'], 'queries': queries, 'text': page.locator('body').inner_text()[-3000:]}
        assert all(q['rows'] > 0 and q['query_id'] for q in queries), queries
        page.wait_for_timeout(1800)
        assert page.locator('canvas').count() >= len(dashboard['charts']), page.locator('body').inner_text()[-2000:]
        assert not errors, errors
        assert page.get_by_text('AllUp', exact=False).count() == 0, 'Exempt snapshot charts must not generate ineffective filters'
        path = Path('tmp')/('showcase-'+dashboard['id']+'.png')
        page.screenshot(path=str(path), full_page=True)
        page.get_by_title('Refresh all charts', exact=True).click()
        page.wait_for_timeout(2000)
        assert not failed and not errors, {'failed': failed, 'errors': errors}
        evidence.append({'dashboard': dashboard['name'], 'id': dashboard['id'], 'queries': queries, 'screenshot': str(path)})
        print('PASS: '+dashboard['name']+' — '+str(len(dashboard['charts']))+' rendered Engine charts', flush=True)
        page.close()
    Path('tmp/showcase-browser-validation.json').write_text(json.dumps(evidence, indent=2)+'\n')
    browser.close()
