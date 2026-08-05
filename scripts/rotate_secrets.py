"""One-off: rotate the Neon DB password + KAVEON_PROXY_SECRET and propagate them.

Rotates:
  • Neon owner password  — via ALTER ROLE (owner can change its own password),
    then updates local .env METADATA_PASSWORD + Render service env.
  • KAVEON_PROXY_SECRET     — fresh value on local .env + Render env + Vercel env.

Reads current DB parts from .env; reads RENDER_KEY / VERCEL_TOKEN from env.
No secrets are printed or stored in this file. After running, redeploy Render +
Vercel and restart the local API so the new values take effect.

Usage:  RENDER_KEY=... VERCEL_TOKEN=... python scripts/rotate_secrets.py
"""
import os
import re
import secrets

import requests
import psycopg2

ENV_PATH = os.path.join(os.path.dirname(__file__), "..", ".env")
RENDER_SID = "srv-d9npv8u1egvs738jeh10"
VERCEL_PROJECT = "prj_xAld9qV2LBaK4dRRZl7tNgOXtGV9"
RENDER_KEY = os.environ["RENDER_KEY"]
VERCEL_TOKEN = os.environ["VERCEL_TOKEN"]


def read_env():
    d = {}
    for line in open(ENV_PATH, encoding="utf-8"):
        s = line.strip()
        if s and not s.startswith("#") and "=" in s:
            k, v = s.split("=", 1)
            d[k.strip()] = v.strip().strip('"')
    return d


def set_env_lines(updates: dict):
    """Rewrite .env, replacing the given KEY=value lines in place."""
    lines = open(ENV_PATH, encoding="utf-8").read().splitlines()
    seen = set()
    for i, line in enumerate(lines):
        m = re.match(r"\s*([A-Z_][A-Z0-9_]*)\s*=", line)
        if m and m.group(1) in updates:
            lines[i] = f"{m.group(1)}={updates[m.group(1)]}"
            seen.add(m.group(1))
    for k, v in updates.items():
        if k not in seen:
            lines.append(f"{k}={v}")
    open(ENV_PATH, "w", encoding="utf-8", newline="\n").write("\n".join(lines) + "\n")


def render_set(key, value):
    r = requests.put(
        f"https://api.render.com/v1/services/{RENDER_SID}/env-vars/{key}",
        headers={"Authorization": f"Bearer {RENDER_KEY}", "Content-Type": "application/json"},
        json={"value": value}, timeout=30)
    return r.status_code, r.text[:120]


def vercel_set(key, value):
    h = {"Authorization": f"Bearer {VERCEL_TOKEN}", "Content-Type": "application/json"}
    envs = requests.get(f"https://api.vercel.com/v9/projects/{VERCEL_PROJECT}/env?limit=100",
                        headers=h, timeout=30).json().get("envs", [])
    eid = next((e["id"] for e in envs if e.get("key") == key), None)
    if not eid:
        return "no-id", ""
    r = requests.patch(f"https://api.vercel.com/v9/projects/{VERCEL_PROJECT}/env/{eid}",
                       headers=h, json={"value": value}, timeout=30)
    return r.status_code, r.text[:120]


def main():
    env = read_env()
    old_pw = env["METADATA_PASSWORD"]
    user, host, port, db, ssl = (env["METADATA_USER"], env["METADATA_HOST"],
                                 env.get("METADATA_PORT", "5432"), env["METADATA_DATABASE"],
                                 env.get("METADATA_SSLMODE", "require"))
    new_pw = secrets.token_hex(24)          # 48 hex chars — URL-safe, no encoding needed
    new_proxy = secrets.token_urlsafe(36)

    old_url = f"postgresql://{user}:{old_pw}@{host}:{port}/{db}?sslmode={ssl}"
    new_url = f"postgresql://{user}:{new_pw}@{host}:{port}/{db}?sslmode={ssl}"

    # 1) Rotate Neon password (owner changing its own password).
    c = psycopg2.connect(old_url, connect_timeout=30); c.autocommit = True
    c.cursor().execute(f'ALTER ROLE "{user}" WITH PASSWORD %s', (new_pw,))
    c.close()
    # verify the new password works
    psycopg2.connect(new_url, connect_timeout=30).close()
    print("neon: password rotated + verified")

    # 2) Propagate to Render service env.
    print("render METADATA_PASSWORD:", render_set("METADATA_PASSWORD", new_pw))
    print("render KAVEON_PROXY_SECRET:", render_set("KAVEON_PROXY_SECRET", new_proxy))

    # 3) Propagate proxy secret to Vercel env.
    print("vercel KAVEON_PROXY_SECRET:", vercel_set("KAVEON_PROXY_SECRET", new_proxy))

    # 4) Update local .env last (so failures above don't strand the running API).
    set_env_lines({"METADATA_PASSWORD": new_pw, "KAVEON_PROXY_SECRET": new_proxy})
    print("local .env: updated METADATA_PASSWORD + KAVEON_PROXY_SECRET")
    print("DONE — now redeploy Render + Vercel and restart the local API.")


if __name__ == "__main__":
    main()
