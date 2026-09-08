"""Loopback-only, read-only Engine dashboard over authenticated AKS Run Command.

Run from the repository root. Browser requests require the Engine principal
token; credentials never enter URLs, logs or browser storage. Azure queries
are batched once per minute, independently of browser polling. Ctrl+C stops it.
"""
import argparse
import hmac
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[1]

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--port', type=int, default=18444)
    p.add_argument('--subscription', default='eaa4a83d-8511-497c-b0bc-40aa5f0deae1')
    p.add_argument('--resource-group', default='test-prproddu-test')
    p.add_argument('--cluster', default='kaveon-test-aks')
    p.add_argument('--private', type=Path, default=ROOT / 'tmp/aks-private-v2')
    p.add_argument('--interval', type=int, default=60)
    args = p.parse_args()
    if args.interval < 30:
        p.error('Refresh interval must be at least 30 seconds')
    token = json.loads((args.private / 'tokens.json').read_text())['principal']
    state = {'updated': None, 'error': 'Waiting for first Azure refresh', 'data': {}, 'last_access': time.time()}
    lock = threading.Lock()
    stopped = threading.Event()
    az = 'az.cmd' if os.name == 'nt' else 'az'
    paths = {'cluster': '/v1/cluster', 'queries': '/v1/query'}

    def refresh():
        # Existing private directory has restricted Windows ACLs; children inherit.
        with tempfile.TemporaryDirectory(prefix='ui-', dir=args.private) as directory:
            directory = Path(directory)
            files, commands = [], ['set -eu']
            for name, path in paths.items():
                config = directory / (name + '.curl')
                config.write_text(
                    f'url = "https://localhost:8080{path}"\n'
                    f'header = "Authorization: Bearer {token}"\n'
                    'fail\nsilent\nshow-error\nmax-time = 20\n', newline='\n')
                files.extend(['--file', str(config)])
                commands += [f'printf "RESULT {name} "',
                             f'kubectl -n kaveon exec -i kaveon-coordinator-0 -- curl --config - < {name}.curl',
                             "printf '\\n'"]
            snapshot = directory / 'snapshot.sh'
            snapshot.write_text('\n'.join(commands) + '\n', encoding='utf-8', newline='\n')
            files.extend(['--file', str(snapshot)])
            command = [az, 'aks', 'command', 'invoke', '--subscription', args.subscription,
                       '--resource-group', args.resource_group, '--name', args.cluster,
                       '--command', 'sh snapshot.sh', *files, '-o', 'json', '--only-show-errors']
            while not stopped.is_set():
                with lock:
                    idle = time.time() - state['last_access'] > 180
                if idle:
                    stopped.wait(5)
                    continue
                try:
                    result = subprocess.run(command, capture_output=True, text=True, timeout=120)
                    if result.returncode:
                        raise RuntimeError('Azure command failed; check Azure login and cluster access')
                    response = json.loads(result.stdout)
                    if response.get('exitCode') != 0:
                        raise RuntimeError('Engine snapshot failed; check coordinator readiness')
                    data = {}
                    for line in response.get('logs', '').splitlines():
                        if line.startswith('RESULT '):
                            _, name, body = line.split(' ', 2)
                            data[paths[name]] = json.loads(body)
                    if set(data) != set(paths.values()):
                        raise RuntimeError('Incomplete Engine snapshot')
                    with lock:
                        state.update(updated=time.time(), error=None, data=data)
                except Exception as error:
                    # Never expose CLI output, which may contain sensitive query data.
                    with lock:
                        state['error'] = str(error) if isinstance(error, RuntimeError) else 'Azure refresh failed (' + type(error).__name__ + ')'
                stopped.wait(args.interval)

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def send(self, status, body, mime='application/json'):
            self.send_response(status)
            self.send_header('Content-Type', mime)
            self.send_header('Cache-Control', 'no-store')
            self.send_header('X-Content-Type-Options', 'nosniff')
            self.send_header('Content-Security-Policy', "frame-ancestors 'none'")
            self.end_headers()
            self.wfile.write(body.encode())

        def do_GET(self):
            if self.headers.get('Host') not in [f'127.0.0.1:{args.port}', f'localhost:{args.port}']:
                return self.send(403, '{}')
            if self.path in ['/', '/ui']:
                html = (ROOT / 'engine/crates/server/src/ui.html').read_text(encoding='utf-8')
                banner = '<div style="padding:10px;text-align:center;background:#172554;color:#fff">Read-only AKS viewer · Azure snapshots every ' + str(args.interval) + ' seconds · <span id="aks-snapshot">Sign in to see refresh status</span></div>'
                script = """<script>
setInterval(async()=>{try{
 if(typeof accessToken==='undefined'||!accessToken)return;
 const r=await fetch('/bridge/status',{headers:{Authorization:'Bearer '+accessToken}});
 if(!r.ok)return;const s=await r.json();
 document.getElementById('aks-snapshot').textContent=s.error||('Updated '+new Date(s.updated*1000).toLocaleTimeString());
}catch{}},5000);
</script>"""
                return self.send(200, html.replace('<body>', '<body>' + banner).replace('</body>', script + '</body>'), 'text/html; charset=utf-8')
            if not hmac.compare_digest(self.headers.get('Authorization', ''), 'Bearer ' + token):
                return self.send(401, '{"error":"Sign in with the Engine token"}')
            with lock:
                state['last_access'] = time.time()
                if self.path == '/bridge/status':
                    return self.send(200, json.dumps({k: state[k] for k in ['updated', 'error']}))
                if self.path not in paths.values():
                    return self.send(404, '{}')
                if state['error'] or not state['updated'] or time.time() - state['updated'] > args.interval + 120:
                    return self.send(503, '{"error":"Snapshot unavailable; check refresh status"}')
                return self.send(200, json.dumps(state['data'][self.path]))

    server = ThreadingHTTPServer(('127.0.0.1', args.port), Handler)
    threading.Thread(target=refresh, daemon=True).start()
    print(f'Engine UI: http://localhost:{args.port}/ui (read-only; Azure refresh every {args.interval}s)', flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        stopped.set()
        server.server_close()

if __name__ == '__main__':
    main()
