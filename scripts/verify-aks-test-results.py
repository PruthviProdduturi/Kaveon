"""Verify AKS command output against independently generated SQL expectations."""
import argparse
import json
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('result', type=Path)
p.add_argument('--expected', type=Path, default=Path('tmp/aks-medallion/expected-results.json'))
p.add_argument('--report', type=Path, default=Path('tmp/aks-validation-report.json'))
args = p.parse_args()
result = json.loads(args.result.read_text(encoding='utf-8-sig'))
assert result['exitCode'] == 0, 'AKS command failed'
responses = {}
for line in result['logs'].splitlines():
    if line.startswith('RESULT '):
        _, name, body = line.split(' ', 2)
        assert name not in responses, f'Duplicate result {name}'
        responses[name] = json.loads(body)
assert responses['nodes']['active_workers'] == 3, 'Expected three active workers'
checks = []
for case in json.loads(args.expected.read_text()):
    actual = responses[case['name']]
    assert not actual.get('error'), actual.get('error')
    assert actual['data'] == case['rows'], f"SQL mismatch: {case['name']}"
    checks.append({'name': case['name'], 'passed': True, 'rows': actual['data']})
assert 'UNAUTHORIZED 401' in result['logs'].splitlines(), 'Missing unauthenticated rejection'
report = {'passed': True, 'active_workers': 3, 'unauthenticated_status': 401,
          'finished_at': result['finishedAt'], 'checks': checks}
args.report.write_text(json.dumps(report, indent=2) + '\n')
print(f'PASS: {len(checks)} exact SQL results, three active workers, unauthenticated request rejected.')
