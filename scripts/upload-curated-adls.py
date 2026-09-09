"""Upload an AKS-curated batch using a short-lived caller token, never logged."""
import hashlib
import json
import os
from pathlib import Path
import requests

root = Path('/work')
account = os.environ['ADLS_ACCOUNT']
container = os.environ.get('ADLS_CONTAINER', 'opensource')
if not account.isalnum() or container != 'opensource':
    raise SystemExit('Unexpected upload destination')
session = requests.Session()
session.headers.update({'Authorization': 'Bearer ' + os.environ['ADLS_UPLOAD_TOKEN'],
                        'x-ms-version': '2023-11-03'})
base = f'https://{account}.blob.core.windows.net/{container}'
response = session.put(base + '?restype=container', data=b'', timeout=60)
if response.status_code not in (201, 409):
    raise SystemExit(f'Container creation failed: HTTP {response.status_code}')

# Version the complete batch; registration points only to validated outputs.
prefix = 'snapshots/2026-09-09-v1'
uploaded = []
for path in sorted(root.rglob('*')):
    if not path.is_file() or path.suffix not in ('.parquet', '.csv', '.json'):
        continue
    relative = path.relative_to(root).as_posix()
    with path.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    url = base + '/' + prefix + '/' + relative
    # Avoid overwriting an existing snapshot with different source contents.
    current = session.head(url, timeout=60)
    if current.status_code == 200:
        if current.headers.get('x-ms-meta-sha256') != digest:
            raise SystemExit(f'Existing snapshot differs: {relative}; use a new version')
    elif current.status_code == 404:
        with path.open('rb') as stream:
            result = session.put(url, data=stream, timeout=300, headers={
                'x-ms-blob-type': 'BlockBlob', 'x-ms-meta-sha256': digest,
                'If-None-Match': '*',
            })
        if result.status_code != 201:
            raise SystemExit(f'Upload failed for {relative}: HTTP {result.status_code}')
    else:
        raise SystemExit(f'Blob check failed: HTTP {current.status_code}')
    uploaded.append({'path': relative, 'bytes': path.stat().st_size, 'sha256': digest})
print(json.dumps({'status': 'uploaded', 'container': container, 'prefix': prefix,
                  'files': uploaded}))
