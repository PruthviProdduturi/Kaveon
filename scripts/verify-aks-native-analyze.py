"""Credential-safe two-phase AKS verification for durable native ANALYZE."""
import argparse, json, os, sys, urllib.request
from datetime import datetime, timezone
from pathlib import Path

TABLE = "OpenSource.ai_benchmarks.leaderboard"
ROWS = 34

def request(url, method, token, proxy_secret, user_email):
    headers = {"Accept": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    elif proxy_secret and user_email:
        headers.update({"X-Proxy-Secret": proxy_secret, "X-User-Email": user_email,
                        "X-User-Role": "Admin", "X-User-Roles": "Admin"})
    else:
        raise ValueError("set an access token or proxy secret plus user email")
    req = urllib.request.Request(url, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=180) as response:
        return json.load(response)

def diagnostic(payload):
    records = payload.get("statistics", []) if isinstance(payload, dict) else []
    return next((r for r in records if r.get("table") == TABLE), None)

def main(argv=None):
    parser = argparse.ArgumentParser(description="Run before and after an externally controlled coordinator restart")
    parser.add_argument("--api-url", required=True)
    parser.add_argument("--phase", choices=("analyze", "restart"), required=True)
    parser.add_argument("--token-env", default="KAVEON_API_ACCESS_TOKEN")
    parser.add_argument("--proxy-secret-env", default="KAVEON_PROXY_SECRET")
    parser.add_argument("--user-email", default=os.getenv("KAVEON_VERIFY_USER_EMAIL"))
    parser.add_argument("--report", type=Path, default=Path("tmp/aks-native-analyze-validation.json"))
    args = parser.parse_args(argv)
    base = args.api_url.rstrip("/") + "/api/v1/engine/console/statistics"
    token, secret = os.getenv(args.token_env), os.getenv(args.proxy_secret_env)
    errors, query = [], None
    try:
        if args.phase == "analyze":
            qualification = request(base + "/qualify", "POST", token, secret, args.user_email)
            if qualification.get("capability", {}).get("native_analyze") is not True:
                errors.append("Engine native ANALYZE capability was not verified")
            query = qualification.get("query") or {}
            if query.get("state") != "FINISHED" or not query.get("id"):
                errors.append("ANALYZE did not return a finished query ID")
            data = query.get("data") or []
            if not data or data[0][-1] != ROWS:
                errors.append(f"ANALYZE did not report exact {ROWS} rows")
        stats = request(base, "GET", token, secret, args.user_email)
        item = diagnostic(stats)
        if not item:
            errors.append(f"missing durable statistic for {TABLE}")
        elif item.get("row_count") != ROWS or item.get("current") is not True:
            errors.append(f"durable statistic is not current with {ROWS} rows")
    except Exception as error:
        item = None
        errors.append(f"verification request failed: {error}")
    report = {"passed": not errors, "phase": args.phase,
              "verified_at": datetime.now(timezone.utc).isoformat(), "table": TABLE,
              "expected_rows": ROWS,
              "query": ({"id": query.get("id"), "state": query.get("state")} if query else None),
              "statistic": item, "planner_check": "deferred-no-qualified-safe-join-pair", "errors": errors}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    if errors:
        print(f"FAIL: {len(errors)} native ANALYZE error(s); report: {args.report}", file=sys.stderr)
        return 1
    print(f"PASS: {args.phase} native ANALYZE durability for {TABLE}; report: {args.report}")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
