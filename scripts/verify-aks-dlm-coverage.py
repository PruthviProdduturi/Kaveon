"""Trigger and verify the AKS DLM row-count backfill through the API."""

import argparse
import json
import os
import sys
import urllib.request
from datetime import datetime, timezone
from pathlib import Path


EXPECTED_DATASETS = (
    "Kaveon Events",
    "NYC Taxi by Borough",
    "COVID-19 Global",
    "AI Model Pricing",
    "AI Arena Battles",
    "AI Model Leaderboard",
    "Climate × Energy",
    "Global Temperature",
    "Global Energy",
)


def validate_coverage(payload):
    datasets = payload.get("datasets") if isinstance(payload, dict) else None
    if not isinstance(datasets, list):
        return [], ["response does not contain a datasets list"]
    by_name = {item.get("name"): item for item in datasets if isinstance(item, dict)}
    checks, errors = [], []
    for name in EXPECTED_DATASETS:
        item = by_name.get(name)
        if item is None:
            errors.append(f"missing dataset: {name}")
            continue
        status = item.get("status")
        row_count = item.get("row_count")
        row_count_source = item.get("row_count_source")
        valid_count = isinstance(row_count, int) and not isinstance(row_count, bool) and row_count > 0
        if status != "ready":
            errors.append(f"{name}: expected ready, received {status!r}")
        if not valid_count:
            errors.append(f"{name}: expected positive exact row_count, received {row_count!r}")
        if row_count_source != "kaveon_engine_exact":
            errors.append(
                f"{name}: expected kaveon_engine_exact row_count source, received {row_count_source!r}"
            )
        checks.append({
            "dataset_id": str(item.get("dataset_id", "")),
            "name": name,
            "status": status,
            "row_count": row_count,
            "row_count_source": row_count_source,
            "passed": status == "ready" and valid_count
                      and row_count_source == "kaveon_engine_exact",
        })
    return checks, errors


def request_coverage(api_url, token=None, proxy_secret=None, user_email=None):
    headers = {"Accept": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    elif proxy_secret and user_email:
        headers.update({
            "X-Proxy-Secret": proxy_secret,
            "X-User-Email": user_email,
            "X-User-Role": "Admin",
            "X-User-Roles": "Admin",
        })
    else:
        raise ValueError("set an access token or proxy secret plus user email")
    request = urllib.request.Request(
        api_url.rstrip("/") + "/api/v1/dlm/coverage", headers=headers
    )
    with urllib.request.urlopen(request, timeout=180) as response:
        return json.load(response)


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--api-url", required=True)
    parser.add_argument("--token-env", default="KAVEON_API_ACCESS_TOKEN")
    parser.add_argument("--proxy-secret-env", default="KAVEON_PROXY_SECRET")
    parser.add_argument("--user-email", default=os.environ.get("KAVEON_VERIFY_USER_EMAIL"))
    parser.add_argument("--report", type=Path,
                        default=Path("tmp/aks-dlm-row-count-validation.json"))
    args = parser.parse_args(argv)
    token = os.environ.get(args.token_env)
    proxy_secret = os.environ.get(args.proxy_secret_env)
    try:
        payload = request_coverage(args.api_url, token, proxy_secret, args.user_email)
        checks, errors = validate_coverage(payload)
    except Exception as error:
        checks, errors = [], [f"coverage request failed: {error}"]
    report = {
        "passed": not errors,
        "verified_at": datetime.now(timezone.utc).isoformat(),
        "api_url": args.api_url,
        "expected_dataset_count": len(EXPECTED_DATASETS),
        "checks": checks,
        "errors": errors,
    }
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    if errors:
        print(f"FAIL: {len(errors)} DLM coverage error(s); report: {args.report}", file=sys.stderr)
        return 1
    print(f"PASS: {len(checks)} ready Engine DLM datasets have positive exact row counts; report: {args.report}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
