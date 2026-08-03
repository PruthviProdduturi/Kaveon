"""Vercel API helper for Lens — inspect Forge's project and create/deploy Lens.

Reads VERCEL_TOKEN from env. No secrets are stored in this file.
Usage:
  python vercel_deploy.py inspect          # show Forge + any lens project config
  python vercel_deploy.py create           # create the 'lens' project (git-linked)
  python vercel_deploy.py setenv KEY VALUE [target]   # add an env var (default: production,preview,development)
  python vercel_deploy.py deploy           # trigger a production deployment from GitHub
"""
import os
import sys
import json
import requests

TOKEN = os.environ["VERCEL_TOKEN"]
API = "https://api.vercel.com"
H = {"Authorization": f"Bearer {TOKEN}", "Content-Type": "application/json"}
REPO = "PruthviProdduturi/Lens"
PROJECT = "lens"


def _get(path, **kw):
    return requests.get(API + path, headers=H, timeout=30, **kw)


def _post(path, body, **kw):
    return requests.post(API + path, headers=H, data=json.dumps(body), timeout=60, **kw)


def inspect():
    r = _get("/v9/projects?limit=100")
    r.raise_for_status()
    for p in r.json().get("projects", []):
        if p["name"] in ("forge", "lens", "forge-portal"):
            print(f"--- {p['name']} ---")
            for k in ("id", "framework", "rootDirectory", "buildCommand",
                      "installCommand", "outputDirectory", "devCommand", "nodeVersion"):
                print(f"  {k}: {p.get(k)}")
            link = p.get("link") or {}
            print(f"  git: {link.get('type')}:{link.get('org')}/{link.get('repo')} branch={link.get('productionBranch')}")


def create():
    body = {
        "name": PROJECT,
        "framework": "nextjs",
        "rootDirectory": "apps/lens-web",
        "gitRepository": {"type": "github", "repo": REPO},
    }
    r = _post("/v11/projects", body)
    print(r.status_code)
    d = r.json()
    if r.ok:
        print("created project id:", d.get("id"), "name:", d.get("name"))
    else:
        print(json.dumps(d, indent=2)[:800])


def setenv(key, value, target="production,preview,development"):
    body = {"key": key, "value": value, "type": "encrypted",
            "target": target.split(","), "gitBranch": None}
    r = _post(f"/v10/projects/{PROJECT}/env?upsert=true", body)
    print(key, "->", r.status_code, ("" if r.ok else r.text[:200]))


def deploy():
    # Trigger a deployment from the linked GitHub repo (production).
    body = {
        "name": PROJECT,
        "project": PROJECT,
        "target": "production",
        "gitSource": {"type": "github", "repo": REPO, "ref": "dev"},
    }
    r = _post("/v13/deployments", body)
    print(r.status_code)
    d = r.json()
    if r.ok:
        print("deployment:", d.get("url"), "state:", d.get("readyState") or d.get("status"))
    else:
        print(json.dumps(d, indent=2)[:800])


import hashlib

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
IGNORE_DIRS = {".git", "node_modules", ".next", ".turbo", "venv", "env",
               "__pycache__", ".vercel", "dist", "build", "out", ".pnpm-store",
               ".claude", "coverage"}
IGNORE_FILES = {"CLAUDE.md"}


def _iter_files(root):
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in IGNORE_DIRS]
        for fn in filenames:
            if fn in IGNORE_FILES or fn.startswith(".env") or fn.endswith((".pyc", ".log")):
                continue
            full = os.path.join(dirpath, fn)
            rel = os.path.relpath(full, root).replace("\\", "/")
            yield full, rel


def disable_protection():
    """Disable Vercel Deployment Protection so anyone can view (no Vercel login)."""
    r = requests.patch(API + f"/v9/projects/{PROJECT}", headers=H,
                       data=json.dumps({"ssoProtection": None, "passwordProtection": None}),
                       timeout=30)
    print("protection:", r.status_code, "(disabled)" if r.ok else r.text[:300])


def add_domain(name):
    """Attach a *.vercel.app domain to the project if available."""
    r = _post(f"/v10/projects/{PROJECT}/domains", {"name": name})
    ok = r.ok
    print(name, "->", r.status_code, "ATTACHED" if ok else r.json().get("error", {}).get("code", r.text[:120]))
    return ok


def pick_domain():
    for cand in ("lens-analytics.vercel.app", "kaveon-lens.vercel.app",
                 "lens-analyze.vercel.app", "getlens.vercel.app",
                 "lens-bi.vercel.app", "lens-app.vercel.app", "uselens.vercel.app"):
        if add_domain(cand):
            print("PICKED:", cand)
            return cand
    print("none available")
    return None


def create_bare():
    """Create the project with no git link (token-only path)."""
    body = {"name": PROJECT, "framework": "nextjs", "rootDirectory": "apps/lens-web"}
    r = _post("/v11/projects", body)
    if r.status_code == 409:
        print("project already exists")
        return
    print("create:", r.status_code, "" if r.ok else r.text[:300])


def deploy_files():
    """Upload the repo and create a production deployment (no GitHub app needed)."""
    files_meta = []
    uploaded = 0
    for full, rel in _iter_files(REPO_ROOT):
        data = open(full, "rb").read()
        sha = hashlib.sha1(data).hexdigest()
        up = requests.post(
            API + "/v2/files",
            headers={"Authorization": f"Bearer {TOKEN}", "x-vercel-digest": sha,
                     "Content-Type": "application/octet-stream"},
            data=data, timeout=120,
        )
        if up.status_code not in (200, 201):
            print("upload FAIL", rel, up.status_code, up.text[:150]); return
        files_meta.append({"file": rel, "sha": sha, "size": len(data)})
        uploaded += 1
    print(f"uploaded {uploaded} files")
    body = {
        "name": PROJECT,
        "files": files_meta,
        "target": "production",
        "projectSettings": {"framework": "nextjs", "rootDirectory": "apps/lens-web"},
    }
    r = _post("/v13/deployments?skipAutoDetectionConfirmation=1", body)
    print("deploy:", r.status_code)
    d = r.json()
    if r.ok:
        print("url: https://" + d.get("url", "?"), "| state:", d.get("readyState") or d.get("status"))
        print("id:", d.get("id"))
    else:
        print(json.dumps(d, indent=2)[:800])


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else "inspect"
    if cmd == "inspect":
        inspect()
    elif cmd == "create":
        create()
    elif cmd == "create_bare":
        create_bare()
    elif cmd == "setenv":
        setenv(*sys.argv[2:])
    elif cmd == "deploy":
        deploy()
    elif cmd == "deploy_files":
        deploy_files()
    elif cmd == "disable_protection":
        disable_protection()
    elif cmd == "add_domain":
        add_domain(sys.argv[2])
    elif cmd == "pick_domain":
        pick_domain()
