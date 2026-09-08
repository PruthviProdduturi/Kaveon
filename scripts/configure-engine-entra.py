"""Prepare a coordinator-only Entra Secret from generated private Engine secrets.

Does not register applications, grant consent, or change the cluster. Apply the
result with kubectl only after verifying tenant/app ownership and permissions.
"""
import argparse
import base64
import json
from pathlib import Path
import uuid

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--tenant-id', required=True)
p.add_argument('--client-id', required=True)
p.add_argument('--principal-object-id', required=True)
p.add_argument('--role', choices=['reader', 'analyst', 'admin'], default='admin')
p.add_argument('--private', type=Path, required=True)
args = p.parse_args()
for name in ['tenant_id', 'client_id', 'principal_object_id']:
    value = getattr(args, name)
    if str(uuid.UUID(value)) != value:
        p.error(name + ' must be a lowercase UUID')
source = json.loads((args.private / 'secrets.json').read_text(encoding='utf-8-sig'))
original = next(item for item in source['items'] if item['metadata']['name'] == 'kaveon-engine-auth')
config = json.loads(base64.b64decode(original['data']['security.json']))
config['entra'] = {'tenant_id': args.tenant_id, 'client_id': args.client_id,
                   'required_scope': 'access_as_user', 'principals': {args.principal_object_id: args.role}}
data = dict(original['data'])
data['security.json'] = base64.b64encode(json.dumps(config).encode()).decode()
secret = {'apiVersion': 'v1', 'kind': 'Secret', 'metadata': {'name': 'kaveon-coordinator-auth', 'namespace': 'kaveon'},
          'type': 'Opaque', 'data': data}
destination = args.private / 'entra-secret.json'
destination.write_text(json.dumps(secret), encoding='utf-8')
print('Prepared coordinator Entra configuration. Secret values omitted.')
