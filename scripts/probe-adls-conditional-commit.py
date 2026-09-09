"""Probe ADLS conditional-write semantics from an allowed network.

Run in the API pod. Supply a short-lived Storage token on stdin, never argv.
Only a unique qualification object is created; its current ETag guards cleanup.
This tests storage behavior, not full Kaveon transaction or migration readiness.
"""
import argparse
import json
import re
import sys
import uuid

import httpx


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--account', required=True)
    parser.add_argument('--container', default='opensource')
    args = parser.parse_args()
    if not re.fullmatch(r'[a-z0-9]{3,24}', args.account) or args.container != 'opensource':
        raise SystemExit('Unexpected qualification destination')
    token = sys.stdin.readline().strip()
    if not token:
        raise SystemExit('Storage token required on stdin')
    url = (f'https://{args.account}.blob.core.windows.net/{args.container}/'
           f'qualification/transactions/{uuid.uuid4()}/head.json')
    client = httpx.Client(timeout=60, headers={
        'Authorization': 'Bearer ' + token, 'x-ms-version': '2023-11-03',
        'x-ms-blob-type': 'BlockBlob', 'Content-Type': 'application/json',
    })
    created = False
    try:
        initial = client.put(url, content=b'{"generation":0}', headers={'If-None-Match': '*'})
        assert initial.status_code == 201, f'create status {initial.status_code}'
        created = True
        stale = initial.headers['etag']
        duplicate = client.put(url, content=b'{"generation":99}', headers={'If-None-Match': '*'})
        assert duplicate.status_code in (409, 412), f'duplicate status {duplicate.status_code}'
        assert client.get(url).json() == {'generation': 0}, 'duplicate altered committed content'
        updated = client.put(url, content=b'{"generation":1}', headers={'If-Match': stale})
        assert updated.status_code == 201, f'CAS status {updated.status_code}'
        rejected = client.put(url, content=b'{"generation":99}', headers={'If-Match': stale})
        assert rejected.status_code == 412, f'stale CAS status {rejected.status_code}'
        current = client.get(url)
        assert current.status_code == 200 and current.json() == {'generation': 1}
        assert current.headers['etag'] == updated.headers['etag']
        print(json.dumps({'create_only': True, 'duplicate_rejected': True,
                          'etag_cas': True, 'stale_writer_rejected': True,
                          'read_matches_committed_version': True}))
    finally:
        if created:
            current = client.head(url)
            if current.status_code == 200:
                deleted = client.delete(url, headers={'If-Match': current.headers['etag']})
                assert deleted.status_code == 202, f'cleanup status {deleted.status_code}'
                print('Qualification object removed.')
        client.close()


if __name__ == '__main__':
    main()
