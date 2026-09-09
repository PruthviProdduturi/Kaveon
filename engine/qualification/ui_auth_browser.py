"""Browser regression for Entra UI wiring (mock identity provider, no live sign-in).

Requires Python Playwright and installed Microsoft Edge. Run from any directory.
"""
import json
from pathlib import Path
from playwright.sync_api import sync_playwright

html = (Path(__file__).resolve().parents[1] / 'crates/server/src/ui.html').read_text(encoding='utf-8')
screenshots = Path(__file__).resolve().parents[2] / 'tmp' / 'ui-browser'
screenshots.mkdir(parents=True, exist_ok=True)
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
    page.context.grant_permissions(['clipboard-read', 'clipboard-write'], origin='https://engine.test')
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
    assert page.locator('#advanced-auth').is_hidden()
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
    page.locator('.qd-back').click()
    page.evaluate('''() => {
      const context={client:'kaveon-cli',source:'interactive',user:'alice@example.test',principal:'entra:tenant:object',catalog:'medallion',schema:'test',engine_version:'0.1.0',environment:'browser-test',client_tags:['review']};
      queries={
        'finished-query':{id:'finished-query',sql:'SELECT customer_id, SUM(amount_cents) AS total\\nFROM medallion.test.orders\\nGROUP BY customer_id\\nORDER BY total DESC\\nLIMIT 5',state:'FINISHED',elapsed_ms:128,submitted_at_ms:1735689600000,completed_at_ms:1735689600128,columns:[{name:'customer_id',type:'Int64'},{name:'total',type:'Int64'}],rows:[[42,8412],[7,5160]],context,timings:{analysis_us:120,planning_us:84,execution_us:127000,result_serialization_us:630}},
        'running-query':{id:'running-query',sql:'SELECT COUNT(*) FROM medallion.test.orders',state:'RUNNING',elapsed_ms:862,submitted_at_ms:1735689600000,columns:[],rows:[],context},
        'failed-query':{id:'failed-query',sql:'SELECT missing_column FROM medallion.test.orders',state:'FAILED',elapsed_ms:4,submitted_at_ms:1735689600000,columns:[],rows:[],error:'column missing_column was not found',context}
      };qorder=['running-query','finished-query','failed-query'];renderHistory();
    }''')
    page.evaluate('window.scrollTo(0,0)')
    page.screenshot(path=str(screenshots / 'query-history-desktop.png'), full_page=True)
    page.locator('[data-query-id="finished-query"]').click()
    page.locator('[data-t="results"]').click()
    assert page.locator('#qdt-results').is_visible()
    assert page.locator('#copy-query-id').is_visible()
    assert page.locator('#copy-query-sql').is_visible()
    page.locator('#copy-query-id').click()
    page.wait_for_function("document.getElementById('copy-status').textContent==='Query ID copied.'")
    page.evaluate('window.scrollTo(0,0)')
    page.screenshot(path=str(screenshots / 'query-detail-desktop.png'), full_page=True)
    page.set_viewport_size({'width': 390, 'height': 844})
    page.locator('.qd-back').click()
    page.evaluate('window.scrollTo(0,0)')
    page.screenshot(path=str(screenshots / 'query-history-mobile.png'), full_page=True)
    page.locator('[data-query-id="finished-query"]').click()
    page.evaluate('window.scrollTo(0,0)')
    page.screenshot(path=str(screenshots / 'query-detail-mobile.png'), full_page=True)
    page.set_viewport_size({'width': 1280, 'height': 720})
    assert page.evaluate("queryClient({context:{}})") == 'HTTP API'
    assert page.evaluate("queryUser({context:{principal:'legacy'}})") == 'legacy'
    page.evaluate('''() => {
      queries['finished-query'].context.user='<img src=x onerror=alert(1)>';
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
    page.locator('#advanced-auth').evaluate('(e)=>{e.hidden=false;e.open=true}')
    page.locator('#auth-token').fill('wrong-token')
    page.locator('#auth-form button[type=submit]').click()
    page.wait_for_function("document.getElementById('auth-status').textContent.includes('Access denied')")
    assert page.evaluate('pollTimer') is None
    assert not errors, errors
    browser.close()
print('PASS: mocked Microsoft sign-in, delegated token header, memory cache, disconnect, renewal failure and 401 stop polling. Live Entra login not tested.')
