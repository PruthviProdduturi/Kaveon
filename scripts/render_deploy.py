"""Render API helper — create/deploy kaveon-api (Docker) from the public repo.

Reads RENDER_KEY, NEON_URL, KAVEON_PROXY_SECRET from env. No secrets stored here.
Usage:
  python render_deploy.py create     # create the kaveon-api web service
  python render_deploy.py status     # show service + latest deploy state
"""
import os
import sys
import json
import requests
from urllib.parse import urlparse, unquote, parse_qs

KEY = os.environ["RENDER_KEY"]
API = "https://api.render.com/v1"
H = {"Authorization": f"Bearer {KEY}", "Accept": "application/json", "Content-Type": "application/json"}
REPO = "https://github.com/PruthviProdduturi/Kaveon"
NAME = "kaveon-api"


def neon_parts():
    u = urlparse(os.environ["NEON_URL"])
    q = parse_qs(u.query or "")
    return {
        "host": u.hostname, "port": str(u.port or 5432),
        "db": (u.path or "").lstrip("/"),
        "user": unquote(u.username or ""), "password": unquote(u.password or ""),
        "sslmode": (q.get("sslmode") or ["require"])[0],
    }


def owner_id():
    r = requests.get(API + "/owners", headers=H, timeout=30)
    r.raise_for_status()
    return r.json()[0]["owner"]["id"]


def find_service():
    r = requests.get(API + f"/services?name={NAME}&limit=20", headers=H, timeout=30)
    r.raise_for_status()
    for s in r.json():
        svc = s.get("service", s)
        if svc.get("name") == NAME:
            return svc
    return None


def create():
    n = neon_parts()
    envvars = [
        {"key": "NODE_ENV", "value": "production"},
        {"key": "METADATA_DB_TYPE", "value": "postgresql"},
        {"key": "METADATA_HOST", "value": n["host"]},
        {"key": "METADATA_PORT", "value": n["port"]},
        {"key": "METADATA_DATABASE", "value": n["db"]},
        {"key": "METADATA_USER", "value": n["user"]},
        {"key": "METADATA_PASSWORD", "value": n["password"]},
        {"key": "METADATA_SSLMODE", "value": n["sslmode"]},
        {"key": "KAVEON_PROXY_SECRET", "value": os.environ["KAVEON_PROXY_SECRET"]},
        {"key": "WEB_URL", "value": os.environ.get("WEB_URL", "https://lens.vercel.app")},
    ]
    body = {
        "type": "web_service",
        "name": NAME,
        "ownerId": owner_id(),
        "repo": REPO,
        "branch": "dev",
        "autoDeploy": "yes",
        "serviceDetails": {
            "runtime": "docker",
            "plan": "free",
            "region": "oregon",
            "healthCheckPath": "/api/health",
            "envSpecificDetails": {
                "dockerfilePath": "./api/Dockerfile",
                "dockerContext": ".",
            },
        },
        "envVars": envvars,
    }
    r = requests.post(API + "/services", headers=H, data=json.dumps(body), timeout=60)
    print("status", r.status_code)
    d = r.json()
    if r.ok:
        svc = d.get("service", d)
        print("service id:", svc.get("id"))
        print("url:", svc.get("serviceDetails", {}).get("url") or "(pending)")
    else:
        print(json.dumps(d, indent=2)[:1000])


def status():
    svc = find_service()
    if not svc:
        print("no kaveon-api service")
        return
    print("id:", svc["id"], "| suspended:", svc.get("suspended"))
    print("url:", svc.get("serviceDetails", {}).get("url"))
    r = requests.get(API + f"/services/{svc['id']}/deploys?limit=1", headers=H, timeout=30)
    if r.ok and r.json():
        dep = r.json()[0].get("deploy", r.json()[0])
        print("latest deploy:", dep.get("status"), dep.get("createdAt"))


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else "status"
    {"create": create, "status": status}.get(cmd, status)()
