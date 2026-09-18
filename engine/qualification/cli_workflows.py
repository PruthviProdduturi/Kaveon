"""Exercise the packaged CLI against a deterministic HTTP fixture (no Azure credentials).

Standard library only, so the Engine workflow can run it on every matrix
build right after the release binary is produced.
"""
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

WORKER_FAILURE = (
    "worker 'worker-1' failed task with 500 Internal Server Error: "
    "{\"error\":\"storage: projection references unknown column 'nope'\"}; "
    "worker 'worker-2' failed task with 500 Internal Server Error: "
    "{\"error\":\"storage: projection references unknown column 'nope'\"}"
)


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
        if self.path == '/v1/query/fixture-query/results/0':
            return self.respond({'id': 'fixture-query', 'data': [['p0']],
                'next_uri': '/v1/query/fixture-query/results/1', 'row_count': 2})
        if self.path == '/v1/query/fixture-query/results/1':
            return self.respond({'id': 'fixture-query', 'data': [['p1']], 'next_uri': None, 'row_count': 2})
        if self.path.startswith('/v1/query/'):
            return self.respond({'id': 'fixture-query', 'state': 'FINISHED', 'elapsed_ms': 20,
                'stages': [{'task_count': 2, 'completed_tasks': 2,
                'tasks': [{'node_id': 'worker-1'}, {'node_id': 'worker-2'}]}]})
        self.respond({'error': 'unknown fixture route'}, 404)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        seen.append(body)
        query = body['query']
        if 'NOPE' in query:
            return self.respond({'error': WORKER_FAILURE}, 500)
        if 'SYNTAX' in query:
            return self.respond({'error': 'SQL parse error: sql: Expected an expression, found: FROM at line 1, column 8',
                'code': 'SYNTAX_ERROR', 'position': {'line': 1, 'column': 8}}, 400)
        if 'BAD' in query:
            return self.respond({'error': 'intentional SQL failure'}, 400)
        if body.get('result_delivery') == 'paged':
            return self.respond({'id': 'fixture-query', 'state': 'FINISHED', 'elapsed_ms': 20,
                'columns': [{'name': 'value', 'type': 'VARCHAR'}], 'data': [], 'error': None,
                'next_uri': '/v1/query/fixture-query/results/0'})
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


def stderr_lines(result):
    return [line for line in result.stderr.splitlines() if line.strip()]


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

    # The aligned summary: one ` ✓ ` verdict line carrying the query id, no Nodes: line.
    result = run('-e', 'SELECT 1;', '--output-format', 'ALIGNED')
    assert result.returncode == 0 and result.stdout.endswith('\n\n'), result
    verdicts = [line for line in result.stdout.splitlines() if line.startswith(' ✓ ')]
    assert len(verdicts) == 1 and 'fixture-query'[:8] in verdicts[0], result.stdout
    assert 'Nodes:' not in result.stdout, result.stdout

    # .catalogs through -e: the list form and the catalog API summary.
    result = run('-e', '.catalogs', '--output-format', 'ALIGNED')
    assert result.returncode == 0, result
    lines = result.stdout.splitlines()
    assert '  lake' in lines, result.stdout
    assert any(line.startswith(' ✓ ') and line.endswith('catalog API') for line in lines), result.stdout

    # The singular SHOW kind is accepted; a misspelt one is refused with a suggestion.
    result = run('-e', 'SHOW CATALOG;', '--output-format', 'ALIGNED')
    assert result.returncode == 0 and '  lake' in result.stdout.splitlines(), result
    result = run('-e', 'SHOW CATALOGE;', '--output-format', 'ALIGNED')
    assert result.returncode == 1 and 'did you mean SHOW CATALOGS?' in result.stderr, result
    assert not result.stdout, result.stdout

    # A worker failure repeated on two workers is one stderr line naming the kind.
    seen.clear()
    result = run('-e', 'SELECT NOPE FROM orders;', '--output-format', 'ALIGNED')
    assert result.returncode == 1 and len(seen) == 1 and not result.stdout, (result, seen)
    assert stderr_lines(result) == [
        "error: Worker failure: storage: projection references unknown column 'nope'"], result.stderr

    # A SYNTAX_ERROR with a position: the kind, the message, the position stripped.
    result = run('-e', 'SELECT SYNTAX;', '--output-format', 'ALIGNED')
    assert result.returncode == 1 and not result.stdout, result
    assert stderr_lines(result) == ['error: SQL parse error: Expected an expression, found: FROM'], result.stderr

    # --paged: a machine format streams every page with the header once; a
    # table format collects the pages first; without the flag delivery stays inline.
    seen.clear()
    result = run('--paged', '-e', 'SELECT 1;', '--output-format', 'CSV_HEADER')
    assert result.returncode == 0 and result.stdout == '"value"\n"p0"\n"p1"\n', result
    assert seen[0]['result_delivery'] == 'paged', seen
    result = run('--paged', '-e', 'SELECT 1;', '--output-format', 'ALIGNED')
    assert result.returncode == 0 and '| p0    |\n| p1    |\n' in result.stdout and '2 rows' in result.stdout, result
    seen.clear()
    result = run('-e', 'SELECT 1;', '--output-format', 'CSV_HEADER')
    assert result.returncode == 0 and result.stdout == '"value"\n"a; b"\n', result
    assert seen[0]['result_delivery'] == 'inline', seen

    print('PASS: binary batch context, quoted semicolons, file/stdin, output formats, error exit codes, '
          'metadata commands, SHOW suggestions, one-line worker and parse failures, a truthful summary, '
          'and --paged streaming.')
finally:
    server.shutdown()
    server.server_close()
