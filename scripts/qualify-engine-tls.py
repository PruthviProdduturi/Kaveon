"""Exercise native TLS with ephemeral certificates; never disables verification.

Run using api/venv/Scripts/python.exe scripts/qualify-engine-tls.py.
"""
import datetime
import ipaddress
import http.client
import json
import os
from pathlib import Path
import secrets
import socket
import ssl
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID


def main():
    root = Path(__file__).resolve().parents[1]
    server = root / "engine/target/debug/kaveon-server.exe"
    if not server.exists():
        server = root / "engine/target/debug/kaveon-server"
    with tempfile.TemporaryDirectory(prefix="kaveon-tls-test-") as temp:
        temp = Path(temp)
        key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "localhost")])
        now = datetime.datetime.now(datetime.timezone.utc)
        cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name).public_key(key.public_key())
                .serial_number(x509.random_serial_number()).not_valid_before(now - datetime.timedelta(minutes=1))
                .not_valid_after(now + datetime.timedelta(hours=1))
                .add_extension(x509.BasicConstraints(ca=True, path_length=None), critical=True)
                .add_extension(x509.SubjectAlternativeName([x509.DNSName("localhost"), x509.IPAddress(ipaddress.ip_address("127.0.0.1"))]), critical=False)
                .sign(key, hashes.SHA256()))
        cert_path, key_path = temp / "cert.pem", temp / "key.pem"
        cert_path.write_bytes(cert.public_bytes(serialization.Encoding.PEM))
        key_path.write_bytes(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        token = secrets.token_urlsafe(32)
        env = {k: v for k, v in os.environ.items() if not k.startswith("KAVEON_")}
        env.update({"KAVEON_HTTP_PORT": str(port), "KAVEON_TLS_CERT_PATH": str(cert_path),
                    "KAVEON_TLS_KEY_PATH": str(key_path), "KAVEON_CATALOG_DATABASE_PATH": str(temp / "catalog.db"),
                    "KAVEON_SECURITY_JSON": json.dumps({"principals": [{"token": token, "principal": "tls-test", "role": "analyst"}]})})
        context = ssl.create_default_context(cafile=str(cert_path))
        base = f"https://127.0.0.1:{port}"
        with (temp / "server.log").open("w") as output:
            process = subprocess.Popen([str(server), str(temp / "missing.toml")], env=env, cwd=temp,
                                       stdout=output, stderr=subprocess.STDOUT,
                                       creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
            try:
                deadline = time.monotonic() + 20
                while True:
                    try:
                        with urllib.request.urlopen(base + "/health", context=context, timeout=1) as response:
                            assert response.status == 200
                            break
                    except (urllib.error.URLError, OSError):
                        if process.poll() is not None or time.monotonic() > deadline:
                            raise RuntimeError((temp / "server.log").read_text())
                        time.sleep(0.1)
                request = urllib.request.Request(base + "/v1/node", headers={"Authorization": "Bearer " + token})
                with urllib.request.urlopen(request, context=context, timeout=3) as response:
                    assert response.status == 200
                try:
                    urllib.request.urlopen(base + "/v1/node", context=context, timeout=3)
                    raise AssertionError("missing token accepted")
                except urllib.error.HTTPError as error:
                    assert error.code == 401
                try:
                    urllib.request.urlopen(base + "/health", timeout=3)
                    raise AssertionError("untrusted certificate accepted")
                except urllib.error.URLError as error:
                    assert isinstance(error.reason, ssl.SSLCertVerificationError), str(error)
                try:
                    urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=3)
                    raise AssertionError("plaintext accepted by TLS listener")
                except (urllib.error.URLError, ConnectionError, http.client.HTTPException):
                    pass
                print(json.dumps({"tls_handshake": "passed", "trusted_certificate": "passed",
                                  "untrusted_certificate_rejected": "passed", "plaintext_rejected": "passed",
                                  "authenticated_request": "passed", "unauthenticated_request_rejected": "passed"}, indent=2))
            finally:
                process.terminate()
                process.wait(timeout=10)


if __name__ == "__main__":
    main()
