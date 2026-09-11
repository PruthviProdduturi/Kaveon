"""Create private Trino TLS and authentication Secret manifests."""

import argparse
import base64
import datetime as dt
import hashlib
import json
from pathlib import Path
import secrets

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID


def encoded(values):
    return {key: base64.b64encode(value).decode() for key, value in values.items()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--namespace", default="kaveon")
    parser.add_argument("--release", default="kaveon-benchmark")
    args = parser.parse_args()
    if args.output.exists():
        raise SystemExit("Refusing to overwrite an existing secret manifest")
    now = dt.datetime.now(dt.timezone.utc)
    ca_key = rsa.generate_private_key(public_exponent=65537, key_size=3072)
    ca_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Kaveon Trino benchmark CA")])
    ca = (x509.CertificateBuilder().subject_name(ca_name).issuer_name(ca_name).public_key(ca_key.public_key())
          .serial_number(x509.random_serial_number()).not_valid_before(now - dt.timedelta(minutes=5))
          .not_valid_after(now + dt.timedelta(days=90)).add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
          .sign(ca_key, hashes.SHA256()))
    key = rsa.generate_private_key(public_exponent=65537, key_size=3072)
    service = f"{args.release}-trino"
    names = [service, f"{service}.{args.namespace}", f"{service}.{args.namespace}.svc",
             f"{service}.{args.namespace}.svc.cluster.local"]
    cert = (x509.CertificateBuilder().subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, service)]))
            .issuer_name(ca_name).public_key(key.public_key()).serial_number(x509.random_serial_number())
            .not_valid_before(now - dt.timedelta(minutes=5)).not_valid_after(now + dt.timedelta(days=30))
            .add_extension(x509.SubjectAlternativeName([x509.DNSName(name) for name in names]), critical=False)
            .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
            .add_extension(x509.ExtendedKeyUsage([x509.oid.ExtendedKeyUsageOID.SERVER_AUTH]), critical=False)
            .sign(ca_key, hashes.SHA256()))
    key_pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption())
    cert_pem = cert.public_bytes(serialization.Encoding.PEM)
    ca_pem = ca.public_bytes(serialization.Encoding.PEM)
    password = secrets.token_urlsafe(48)
    salt = secrets.token_bytes(16)
    iterations = 10_000
    password_hash = hashlib.pbkdf2_hmac("sha1", password.encode(), salt, iterations, dklen=64)
    password_line = f"qualification:{iterations}:{salt.hex()}:{password_hash.hex()}\n"
    auth = {"internal-shared-secret": base64.b64encode(secrets.token_bytes(96)),
            "password.db": password_line.encode(), "client-password": password.encode()}
    items = [
        {"apiVersion": "v1", "kind": "Secret", "metadata": {"name": "kaveon-trino-benchmark-auth", "namespace": args.namespace},
         "type": "Opaque", "data": encoded(auth)},
        {"apiVersion": "v1", "kind": "Secret", "metadata": {"name": "kaveon-trino-benchmark-tls", "namespace": args.namespace},
         "type": "Opaque", "data": encoded({"server.pem": key_pem + cert_pem, "ca.crt": ca_pem})},
    ]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({"apiVersion": "v1", "kind": "List", "items": items}) + "\n", encoding="utf-8")
    print(f"Created benchmark TLS and authentication manifests for {args.namespace}/{args.release}; secret values omitted")


if __name__ == "__main__":
    main()
