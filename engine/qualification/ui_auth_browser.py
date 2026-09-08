"""Browser regression for Entra UI wiring (mock identity provider, no live sign-in).

Requires Python Playwright and installed Microsoft Edge. Run from any directory.
"""
import json
from pathlib import Path
from playwright.sync_api import sync_playwright

html = (Path(__file__).resolve().parents[1] / 'crates/server/src/ui.html').read_text(encoding='utf-8')
mock = '''window.testAuth={};window.msal={PublicClientApplication:class{
 constructor(config){window.testAuth.config=config;}
 async initialize(){}
 async loginPopup(request){window.testAuth.request=request;return {accessToken:'test-access-token',account:{homeAccountId:'test'}};}
 async acquireTokenSilent(){if(window.testAuth.expired)throw Error('expired');return {accessToken:'test-access-token'};}
 async clearCache(){window.testAuth.cleared=true;}
}};'''
config = {'entra': {'tenant_id': '72f988bf-86f1-41af-91ab-2d7cd011db47',
                    'client_id': '11111111-1111-4111-8111-111111111111',
                    'scope': 'api://11111111-1111-4111-8111-111111111111/access_as_user'}}
node = {'node_id': 'test', 'version': 'test', 'uptime_secs': 100, 'memory_rss_bytes': 1024,
        'address': 'https://worker:8080', 'role': 'worker'}
cluster = {'environment': 'browser-test', 'coordinator': dict(node, role='coordinator'),
           'workers': [dict(node, node_id=str(i)) for i in range(3)], 'active_workers': 3, 'total_nodes': 4}
with sync_playwright() as p:
    browser = p.chromium.launch(channel='msedge', headless=True)
    page = browser.new_page()
    errors, headers = [], []
    page.on('pageerror', lambda error: errors.append(str(error)))
    def route(request):
        path = request.request.url.removeprefix('https://engine.test')
        if path == '/ui':
            return request.fulfill(content_type='text/html', body=html)
        if path == '/ui/msal-browser.min.js':
            return request.fulfill(content_type='application/javascript', body=mock)
        if path == '/v1/auth/config':
            return request.fulfill(json=config)
        auth = request.request.headers.get('authorization')
        headers.append(auth)
        if auth != 'Bearer test-access-token':
            return request.fulfill(status=401, json={})
        return request.fulfill(json=cluster if path == '/v1/cluster' else [])
    page.route('https://engine.test/**', route)
    page.goto('https://engine.test/ui')
    page.locator('#microsoft-sign-in').wait_for(state='visible')
    assert not page.locator('#advanced-auth').evaluate('(e)=>e.open')
    page.locator('#microsoft-sign-in').click()
    page.wait_for_function("document.getElementById('g-workers').textContent==='3'")
    assert headers and all(h == 'Bearer test-access-token' for h in headers)
    assert page.evaluate('testAuth.config.cache.cacheLocation') == 'memoryStorage'
    assert page.evaluate('testAuth.config.cache.temporaryCacheLocation') == 'memoryStorage'
    assert page.evaluate('testAuth.request.scopes') == [config['entra']['scope']]
    assert page.evaluate('localStorage.length+sessionStorage.length') == 0
    # Client metadata is display-only; user comes from authenticated server context.
    page.evaluate('''() => {
      queries={'attribution-test':{id:'attribution-test',sql:'SELECT 1',state:'FINISHED',
        elapsed_ms:1,submitted_at_ms:1,columns:[],rows:[],context:{
          client:'kaveon-cli',user:'alice@example.test',principal:'entra:tenant:object'}}};
      qorder=['attribution-test'];renderHistory();
    }''')
    assert page.locator('#qarea').inner_text().find('Kaveon CLI') >= 0
    assert 'alice@example.test' in page.locator('#qarea').inner_text()
    page.locator('[data-query-id="attribution-test"]').click()
    assert 'alice@example.test' in page.locator('#view-detail').inner_text()
    assert 'entra:tenant:object' in page.locator('#view-detail').inner_text()
    assert page.evaluate("queryClient({context:{}})") == 'HTTP API'
    assert page.evaluate("queryUser({context:{principal:'legacy'}})") == 'legacy'
    page.evaluate('''() => {
      queries['attribution-test'].context.user='<img src=x onerror=alert(1)>';
      renderHistory();
    }''')
    assert page.locator('#qarea img').count() == 0
    page.locator('#disconnect').click()
    assert page.locator('#g-workers').inner_text() == '0'
    assert page.evaluate('testAuth.cleared')
    page.locator('#microsoft-sign-in').click()
    page.wait_for_function("document.getElementById('g-workers').textContent==='3'")
    page.evaluate('testAuth.expired=true;refresh()')
    page.wait_for_function("document.getElementById('auth-status').textContent.includes('needs sign-in')")
    assert page.locator('#g-workers').inner_text() == '0'
    page.locator('#advanced-auth').evaluate('(e)=>e.open=true')
    page.locator('#auth-token').fill('wrong-token')
    page.locator('#auth-form button[type=submit]').click()
    page.wait_for_function("document.getElementById('auth-status').textContent.includes('Access denied')")
    assert page.evaluate('pollTimer') is None
    assert not errors, errors
    browser.close()
print('PASS: mocked Microsoft sign-in, delegated token header, memory cache, disconnect, renewal failure and 401 stop polling. Live Entra login not tested.')
