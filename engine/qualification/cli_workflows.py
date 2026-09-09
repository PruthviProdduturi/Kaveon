"""Exercise the packaged CLI against a deterministic HTTP fixture (no Azure credentials)."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

parser = argparse.ArgumentParser()
parser.add_argument('--cli', type=Path, required=True)
args = parser.parse_args()
seen = []

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def respond(self, value, status=200):
        body = json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == '/v1/catalog':
            return self.respond({'catalogs': ['lake']})
        if self.path == '/v1/catalog/lake/schema':
            return self.respond({'schemas': ['gold']})
        if self.path == '/v1/catalog/lake/schema/gold/table':
            return self.respond({'tables': ['orders']})
        if self.path.startswith('/v1/query/'):
            return self.respond({'stages': [{'task_count': 2, 'completed_tasks': 2,
                'tasks': [{'node_id': 'worker-1'}, {'node_id': 'worker-2'}]}]})
        self.respond({'error': 'unknown fixture route'}, 404)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        seen.append(body)
        if 'BAD' in body['query']:
            return self.respond({'error': 'intentional SQL failure'}, 400)
        self.respond({'id': 'fixture-query', 'state': 'FINISHED', 'elapsed_ms': 20,
            'columns': [{'name': 'value', 'type': 'VARCHAR'}], 'data': [['a; b']], 'error': None})

server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
env = dict(os.environ)
for key in ('KAVEON_CONFIG', 'KAVEON_ACCESS_TOKEN', 'KAVEON_CA_CERT', 'KAVEON_PAGER'):
    env.pop(key, None)

def run(*extra, input=None):
    return subprocess.run([str(args.cli.resolve()), '--server', f'http://127.0.0.1:{server.server_port}',
        '--auth', 'none', *extra], input=input, text=True, encoding='utf-8', capture_output=True,
        env=env, timeout=15)

try:
    result = run('-e', "USE lake.gold; SELECT 'a; b'; SELECT 2;", '--output-format', 'JSON')
    assert result.returncode == 0, result.stderr
    assert [json.loads(line) for line in result.stdout.splitlines()] == [{'value': 'a; b'}] * 2, result.stdout
    assert all(r['catalog'] == 'lake' and r['schema'] == 'gold' and r['client'] == 'kaveon-cli' for r in seen)
    seen.clear()
    result = run('-e', 'SELECT 1; BAD; SELECT 2;', '--output-format', 'NULL')
    assert result.returncode == 1 and len(seen) == 2 and not result.stdout, (result, seen)
    seen.clear()
    result = run('-e', 'SELECT 1; BAD; SELECT 2;', '--ignore-errors', '--output-format', 'NULL')
    assert result.returncode == 1 and len(seen) == 3 and not result.stdout, (result, seen)
    seen.clear()
    result = run('--output-format', 'json', input='SELECT 1;\nSELECT 2;')
    assert result.returncode == 0 and len(seen) == 2 and 'kaveon:' not in result.stdout, result
    with tempfile.TemporaryDirectory() as folder:
        script = Path(folder) / 'query.sql'
        script.write_text("-- semicolon ; in comment\nSELECT 'a; b';", encoding='utf-8')
        result = run('--file', str(script), '--output-format', 'CSV_HEADER')
        assert result.returncode == 0 and result.stdout == '"value"\n"a; b"\n', result
    result = run('-e', 'SELECT 1;', '--output-format', 'ALIGNED')
    assert result.returncode == 0 and 'fixture-query' in result.stdout and 'Nodes: 2' in result.stdout and result.stdout.endswith('\n\n'), result
    print('PASS: binary batch context, quoted semicolons, file/stdin, output formats, error exit codes, and truthful summary.')
finally:
    server.shutdown()
    server.server_close()
