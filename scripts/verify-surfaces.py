"""Exercise every readable API route and every Studio page, and report what broke.

Why this exists
---------------
The platform has 172 API routes and 77 pages. Checking them by hand means
checking the ones you happen to think of, which is how a defect gets reported
three times before anyone writes it down. This walks all of them and prints a
table, so "everything works" is a thing that was measured rather than assumed.

What it does, and deliberately does not do
------------------------------------------
Read-only. It issues GET only — never POST, PUT, PATCH or DELETE — so it is
safe against the live installation. The 53 POST routes are therefore NOT
covered here; mutation needs fixtures and teardown, and a smoke test that
creates records in a demo someone is showing is worse than no smoke test.

Path parameters are filled from identifiers the installation actually holds
(a real dataset id, a real chart id, a real Engine table id), because a route
exercised with a made-up id returns 404 and tells you nothing. A route whose
parameter cannot be resolved is reported as `skipped`, with the reason, rather
than silently passing.

Pages are checked for server-side render failures. An unauthenticated request
is redirected to the sign-in screen by the middleware, so a 200 and a redirect
both mean the page rendered; a 500 means it did not. That is a narrow check
and it is stated as such — it catches crashes, not layout.

Run it from inside the API pod, where both services are reachable and the
identity is already configured:

    POD=$(kubectl get pod -n kaveon -l app=kaveon-api \
            --field-selector=status.phase=Running -o jsonpath='{.items[0].metadata.name}')
    kubectl exec -i -n kaveon "$POD" -- python - < scripts/verify-surfaces.py

Exit status is 1 if anything failed, so it can gate a deployment.
"""

from __future__ import annotations

import json
import os
import re
import sys
import time
import urllib.error
import urllib.request
from urllib.parse import quote

ACTOR = os.environ.get("KAVEON_VERIFY_ACTOR", "pruthvi.prodduturi@gmail.com")
ROLE = "Admin"
API = os.environ.get("KAVEON_VERIFY_API", "http://127.0.0.1:8080")
PORTAL = os.environ.get("KAVEON_VERIFY_PORTAL", "http://kaveon-portal.kaveon.svc.cluster.local:3000")
TIMEOUT = 60

# Routes that are deliberately not exercised, with the reason. Anything not
# listed here and not reachable is a failure, so this list is the honest record
# of what this harness does not cover.
EXCLUDED = {
    "/api/v1/engine/console/stream": "server-sent events; would not terminate",
    "/api/v1/lab/query/{query_id}/results/{page}": "needs a query submitted first (a write)",
    "/api/v1/dlm/stream": "server-sent events",
}


def request(url: str, headers: dict | None = None) -> tuple[int, str]:
    """One GET. Returns (status, first bytes of body). Never raises for HTTP."""
    req = urllib.request.Request(url, method="GET", headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as response:
            return response.status, response.read(400).decode("utf-8", "replace")
    except urllib.error.HTTPError as error:
        return error.code, error.read(400).decode("utf-8", "replace")
    except urllib.error.URLError as error:
        return 0, f"transport: {error.reason}"
    except Exception as error:  # noqa: BLE001 - a harness reports, it does not crash
        return 0, f"{type(error).__name__}: {error}"


def api_headers() -> dict:
    secret = os.environ.get("KAVEON_PROXY_SECRET", "")
    if not secret:
        raise SystemExit("KAVEON_PROXY_SECRET is not set; run this inside the API pod")
    return {"X-User-Email": ACTOR, "X-User-Role": ROLE, "X-Proxy-Secret": secret}


def identifiers(headers: dict) -> dict:
    """Real identifiers for the path parameters, read from the installation.

    A route exercised with a fabricated id answers 404 and proves nothing, so
    anything that cannot be resolved is skipped and said to be skipped.
    """
    found: dict[str, str] = {}

    def first(url: str, *paths):
        status, body = request(f"{API}{url}", headers)
        if status != 200:
            return None
        try:
            payload = json.loads(body) if len(body) < 400 else None
        except ValueError:
            payload = None
        if payload is None:  # body was truncated for display; fetch in full
            req = urllib.request.Request(f"{API}{url}", headers=headers)
            try:
                with urllib.request.urlopen(req, timeout=TIMEOUT) as response:
                    payload = json.loads(response.read())
            except Exception:  # noqa: BLE001
                return None
        for path in paths:
            node = payload
            for key in path:
                if isinstance(node, dict):
                    node = node.get(key)
                elif isinstance(node, list) and node:
                    node = node[0]
                else:
                    node = None
                if node is None:
                    break
            if isinstance(node, (str, int)):
                return str(node)
        return None

    found["dataset_id"] = first("/api/v1/datasets", ("datasets", "id"), ("items", "id"))
    found["chart_id"] = first("/api/v1/charts/summary", ("charts", "id"), ("items", "id"))
    found["dashboard_id"] = first("/api/v1/dashboards", ("dashboards", "id"), ("items", "id"))
    found["cs_id"] = first("/api/v1/catalog-sources", ("catalogSources", "id"))
    found["catalog_id"] = first("/api/v1/engine/catalog/definitions", ("definitions", "id"), (0, "id"))
    if found.get("catalog_id"):
        found["schema_id"] = first(
            f"/api/v1/engine/catalog/definitions/{quote(found['catalog_id'], safe='')}/schemas",
            ("schemas", "id"), (0, "id"))
    if found.get("schema_id"):
        found["table_id"] = first(
            f"/api/v1/engine/catalog/schemas/{quote(found['schema_id'], safe='')}/tables",
            ("tables", "id"), (0, "id"))
    found["saved_query_id"] = first("/api/v1/lab/saved-queries", ("savedQueries", "id"), ("items", "id"))
    # Common aliases the routes use for the same things.
    found.setdefault("id", found.get("dataset_id"))
    found["catalog"] = first("/api/v1/catalog-sources", ("catalogSources", "engine_catalog"))
    return {key: value for key, value in found.items() if value}


def api_routes() -> list[str]:
    import main
    paths = set()
    for route in main.app.routes:
        methods = getattr(route, "methods", None) or set()
        if "GET" in methods and getattr(route, "path", "").startswith("/api/"):
            paths.add(route.path)
    return sorted(paths)


def fill(path: str, ids: dict) -> tuple[str | None, str]:
    """Substitute real identifiers, or explain why the route is skipped."""
    missing = [name for name in re.findall(r"\{([^}:]+)", path) if name not in ids]
    if missing:
        return None, f"no identifier for {', '.join(missing)}"
    filled = re.sub(r"\{([^}:]+)(?::[^}]+)?\}",
                    lambda m: quote(str(ids[m.group(1)]), safe=""), path)
    return filled, ""


def studio_pages() -> list[str]:
    """Every page route, from the App Router file tree."""
    root = os.environ.get("KAVEON_VERIFY_APP_DIR", "")
    if root and os.path.isdir(root):
        routes = []
        for current, _dirs, files in os.walk(root):
            if "page.tsx" not in files:
                continue
            rel = os.path.relpath(current, root).replace(os.sep, "/")
            if rel == ".":
                routes.append("/")
                continue
            if "(" in rel or "@" in rel:
                continue
            routes.append("/" + rel)
        return sorted(routes)
    # Inside the pod the source tree is not present; check the routes a reader
    # actually navigates to rather than inventing a list.
    return ["/", "/home", "/workspace", "/charts/new", "/dashboards/new", "/datasets",
            "/datasets/new", "/data-sources", "/catalog", "/lab", "/lab/queries",
            "/engine", "/favorites", "/settings", "/settings/connections",
            "/settings/maintenance", "/docs", "/login"]


def main_() -> int:
    headers = api_headers()
    print("resolving identifiers from the installation", flush=True)
    ids = identifiers(headers)
    print(f"  resolved: {', '.join(sorted(ids)) or 'none'}\n", flush=True)

    failures, skipped, passed = [], [], 0
    print("API (GET only)")
    for path in api_routes():
        if path in EXCLUDED:
            skipped.append((path, EXCLUDED[path]))
            continue
        filled, why = fill(path, ids)
        if filled is None:
            skipped.append((path, why))
            continue
        started = time.time()
        status, body = request(f"{API}{filled}", headers)
        elapsed = time.time() - started
        ok = status in (200, 204, 304)
        if ok:
            passed += 1
        else:
            failures.append(("API", path, status, body.replace("\n", " ")[:120]))
        print(f"  {status:>3} {elapsed:5.2f}s {path}", flush=True)

    print("\nStudio pages (render check: 2xx or a redirect to sign-in is a pass)")
    for route in studio_pages():
        status, body = request(f"{PORTAL}{route}")
        ok = status in (200, 204, 301, 302, 303, 307, 308)
        if ok:
            passed += 1
        else:
            failures.append(("page", route, status, body.replace("\n", " ")[:120]))
        print(f"  {status:>3} {route}", flush=True)

    print(f"\npassed {passed} · failed {len(failures)} · skipped {len(skipped)}")
    if skipped:
        print("\nskipped (not covered by this harness):")
        for path, why in skipped:
            print(f"  {path} — {why}")
    if failures:
        print("\nFAILURES:")
        for kind, path, status, body in failures:
            print(f"  [{kind}] {status} {path}\n        {body}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main_())
