"""Generate test-only PKI and engine credentials into an ignored private directory.

Requires cryptography. Existing credentials are never overwritten. Keep this
directory private; apply the generated Secret JSON via kubectl stdin/file.
"""
import argparse
import base64
import datetime as dt
import json
import pathlib
import secrets
import subprocess

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID

p = argparse.ArgumentParser()
p.add_argument('--output', type=pathlib.Path, required=True)
p.add_argument('--image', default='kaveon-engine:aks-d567232', help='Local Engine image used to obtain public CA roots')
args = p.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
if any(args.output.iterdir()):
    raise SystemExit('Refusing to replace existing credentials')
now = dt.datetime.now(dt.timezone.utc)
ca_key = rsa.generate_private_key(public_exponent=65537, key_size=3072)
ca_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, 'Kaveon test CA')])
ca = (x509.CertificateBuilder().subject_name(ca_name).issuer_name(ca_name)
      .public_key(ca_key.public_key()).serial_number(x509.random_serial_number())
      .not_valid_before(now - dt.timedelta(minutes=5)).not_valid_after(now + dt.timedelta(days=90))
      .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
      .add_extension(x509.KeyUsage(False, False, False, False, False, True, True, False, False), critical=True)
      .sign(ca_key, hashes.SHA256()))
key = rsa.generate_private_key(public_exponent=65537, key_size=3072)
names = ['localhost', 'coordinator', 'coordinator.kaveon', 'coordinator.kaveon.svc',
         'kaveon-coordinator.kaveon.svc.cluster.local', '*.kaveon-workers.kaveon.svc.cluster.local',
         'coordinator.kaveon.svc.cluster.local', '*.workers.kaveon.svc.cluster.local',
         '*.workers.kaveon.svc', '*.workers.kaveon', 'workers']
cert = (x509.CertificateBuilder().subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, 'coordinator')]))
        .issuer_name(ca_name).public_key(key.public_key()).serial_number(x509.random_serial_number())
        .not_valid_before(now - dt.timedelta(minutes=5)).not_valid_after(now + dt.timedelta(days=30))
        .add_extension(x509.SubjectAlternativeName([x509.DNSName(n) for n in names]), critical=False)
        .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
        .add_extension(x509.ExtendedKeyUsage([x509.oid.ExtendedKeyUsageOID.SERVER_AUTH]), critical=False)
        .sign(ca_key, hashes.SHA256()))
tls = {'tls.crt': cert.public_bytes(serialization.Encoding.PEM),
       'tls.key': key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()),
       'ca.crt': ca.public_bytes(serialization.Encoding.PEM)}
tokens = {k: secrets.token_urlsafe(48) for k in ['principal', 'exchange', 'catalog']}
security = {'principals': [{'token': tokens['principal'], 'principal': 'prproddu-test', 'role': 'admin'}]}
credentials = {'security.json': json.dumps(security),
               'exchange-token': tokens['exchange'],
               'catalog-token': tokens['catalog']}
tls['ca.crt'] += subprocess.check_output(['docker', 'run', '--rm', '--entrypoint', 'cat',
                                       args.image, '/etc/ssl/certs/ca-certificates.crt'])
items = []
for name, data in [('kaveon-engine-tls', tls), ('kaveon-engine-auth', {k: v.encode() for k, v in credentials.items()})]:
    items.append({'apiVersion': 'v1', 'kind': 'Secret', 'metadata': {'name': name, 'namespace': 'kaveon'},
                  'type': 'Opaque', 'data': {k: base64.b64encode(v).decode() for k, v in data.items()}})
(args.output / 'secrets.json').write_text(json.dumps({'apiVersion': 'v1', 'kind': 'List', 'items': items}), encoding='utf-8')
(args.output / 'tokens.json').write_text(json.dumps(tokens), encoding='utf-8')
(args.output / 'ca.crt').write_bytes(tls['ca.crt'])
print('Created test credentials; server certificate expires in 30 days. Secret values omitted.')
