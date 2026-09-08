#!/usr/bin/env python3
"""Measure DLM recall against a live compiled context.

Runs a tagged corpus of natural-language questions through /api/v1/dlm/ask and
reports what happened to each one. The point is not a pass rate -- it is to
separate failures that curation can fix (a missing alias, an unmatched metric
name) from failures that need a different mechanism (multi-turn state, or a
model to normalise phrasing). Those have very different costs, and the tags on
each question say which capability it is probing.

Usage:
    python scripts/dlm_recall.py                       # localhost compose stack
    python scripts/dlm_recall.py --api https://host    # another deployment
    python scripts/dlm_recall.py --corpus mine.json    # your own questions
    python scripts/dlm_recall.py --tag follow-up       # only one capability

Auth: set KAVEON_PROXY_SECRET and KAVEON_DEV_USER_EMAIL to call the API
directly. Against the Compose stack in local mode neither is required.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request
from collections import Counter, defaultdict
from typing import Any, Dict, List, Optional

# Each question carries the capability it probes. A failure is only interesting
# once you know which of these it belongs to.
#
#   basic       metric + dimension, plainly worded
#   alias       needs a synonym or value alias to resolve
#   time        needs a date range or grain
#   topn        needs ranking and a limit
#   filter      needs one or more entity filters
#   distinct    needs a non-additive count
#   paraphrase  same intent, wording that does not overlap the schema
#   follow-up   only resolvable with the previous turn's frame
#   scope       genuinely unanswerable; the DLM should decline, not guess
CORPUS: List[Dict[str, str]] = [
    {"q": "revenue by region",                              "tag": "basic"},
    {"q": "total revenue",                                  "tag": "basic"},
    {"q": "orders by plan",                                 "tag": "basic"},
    {"q": "active users by region",                         "tag": "basic"},
    {"q": "show me revenue for each region",                "tag": "basic"},
    {"q": "sales by area",                                  "tag": "alias"},
    {"q": "revenue by segment",                             "tag": "alias"},
    {"q": "how many queries by tier",                       "tag": "alias"},
    {"q": "revenue in the USA",                             "tag": "alias"},
    {"q": "usage for smb customers",                        "tag": "alias"},
    {"q": "revenue last quarter",                           "tag": "time"},
    {"q": "orders in 2026",                                 "tag": "time"},
    {"q": "revenue by month",                               "tag": "time"},
    {"q": "active users over the last 30 days",             "tag": "time"},
    {"q": "top 10 regions by revenue",                      "tag": "topn"},
    {"q": "which plan has the most orders",                 "tag": "topn"},
    {"q": "worst performing region",                        "tag": "topn"},
    {"q": "revenue for Enterprise",                         "tag": "filter"},
    {"q": "active users in Europe on the Team plan",        "tag": "filter"},
    {"q": "orders for India",                               "tag": "filter"},
    {"q": "how many distinct users",                        "tag": "distinct"},
    {"q": "unique customers by region",                     "tag": "distinct"},
    {"q": "how did the west coast do last quarter",         "tag": "paraphrase"},
    {"q": "are we growing",                                 "tag": "paraphrase"},
    {"q": "what's our biggest market",                      "tag": "paraphrase"},
    {"q": "break that down by plan",                        "tag": "follow-up"},
    {"q": "what about Europe",                              "tag": "follow-up"},
    {"q": "and last year",                                  "tag": "follow-up"},
    {"q": "show it as a line chart",                        "tag": "follow-up"},
    {"q": "what is the weather in Seattle",                 "tag": "scope"},
    {"q": "delete all orders",                              "tag": "scope"},
]


def post(api: str, path: str, body: Dict[str, Any], timeout: float) -> Dict[str, Any]:
    req = urllib.request.Request(
        api.rstrip("/") + path,
        data=json.dumps(body).encode(),
        method="POST",
        headers={"content-type": "application/json", **identity_headers()},
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode())


def identity_headers() -> Dict[str, str]:
    headers: Dict[str, str] = {}
    secret = os.environ.get("KAVEON_PROXY_SECRET")
    email = os.environ.get("KAVEON_DEV_USER_EMAIL")
    if secret:
        headers["x-proxy-secret"] = secret
    if email:
        headers["x-user-email"] = email
        headers["x-user-role"] = os.environ.get("KAVEON_DEV_USER_ROLE", "Admin")
    return headers


def classify(result: Dict[str, Any]) -> str:
    """Collapse an ask() response into one outcome bucket."""
    if not result.get("ok"):
        return {"no_dataset": "NO ROUTE"}.get(result.get("reason"), "FAILED")
    if result.get("unresolved_entity"):
        return "RISKY"          # answered, but silently dropped an entity filter
    route = result.get("route")
    if route == "context":
        return "SKETCH" if result.get("approx") else "CONTEXT"
    if route == "live":
        return "LIVE"
    return "ANSWERED"


def run(api: str, corpus: List[Dict[str, str]], timeout: float) -> int:
    rows = []
    for case in corpus:
        started = time.monotonic()
        try:
            result = post(api, "/api/v1/dlm/ask", {"question": case["q"], "limit": 50}, timeout)
        except urllib.error.HTTPError as exc:
            body = exc.read().decode()[:120]
            result = {"ok": False, "reason": f"http_{exc.code}", "_detail": body}
        except Exception as exc:                                  # noqa: BLE001
            print(f"cannot reach {api}: {exc}", file=sys.stderr)
            return 2
        rows.append({
            "q": case["q"],
            "tag": case["tag"],
            "outcome": classify(result),
            "reason": result.get("reason") or "",
            "dataset": result.get("dataset_id") or "",
            "note": (result.get("note") or "")[:44],
            "ms": int((time.monotonic() - started) * 1000),
        })

    width = max(len(r["q"]) for r in rows) + 2
    print(f"\n{'question':<{width}}{'tag':<12}{'outcome':<10}{'ms':>6}  detail")
    print("-" * (width + 44))
    for r in rows:
        detail = r["reason"] or r["note"]
        print(f"{r['q']:<{width}}{r['tag']:<12}{r['outcome']:<10}{r['ms']:>6}  {detail}")

    answered = {"CONTEXT", "SKETCH", "LIVE", "ANSWERED"}
    by_tag: Dict[str, Counter] = defaultdict(Counter)
    for r in rows:
        by_tag[r["tag"]][r["outcome"] in answered] += 1

    print(f"\n{'capability':<14}{'answered':>10}{'failed':>8}   verdict")
    print("-" * 62)
    for tag in sorted(by_tag):
        ok, bad = by_tag[tag][True], by_tag[tag][False]
        if tag == "scope":
            verdict = "declining is correct" if bad else "ANSWERED OUT OF SCOPE"
        elif bad == 0:
            verdict = "covered"
        elif tag in ("alias", "filter"):
            verdict = "curation gap -- add aliases"
        elif tag == "follow-up":
            verdict = "needs conversation state"
        elif tag == "paraphrase":
            verdict = "needs phrasing normalisation"
        else:
            verdict = "needs investigation"
        print(f"{tag:<14}{ok:>10}{bad:>8}   {verdict}")

    total_ok = sum(1 for r in rows if r["outcome"] in answered)
    risky = sum(1 for r in rows if r["outcome"] == "RISKY")
    print(f"\n{total_ok}/{len(rows)} answered · {risky} answered with an unresolved entity")
    if risky:
        print("An unresolved entity means a filter was silently dropped -- treat as a defect,")
        print("not a near miss: the number returned answers a different question.")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--api", default=os.environ.get("KAVEON_API", "http://localhost:8080"))
    parser.add_argument("--corpus", type=argparse.FileType("r"), help="JSON list of {q, tag}")
    parser.add_argument("--tag", help="only run questions with this tag")
    parser.add_argument("--timeout", type=float, default=120.0)
    args = parser.parse_args()

    corpus = json.load(args.corpus) if args.corpus else CORPUS
    if args.tag:
        corpus = [c for c in corpus if c["tag"] == args.tag]
    if not corpus:
        print("no questions selected", file=sys.stderr)
        return 2
    return run(args.api, corpus, args.timeout)


if __name__ == "__main__":
    raise SystemExit(main())
