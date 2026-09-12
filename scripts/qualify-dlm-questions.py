"""Run the DLM question corpus through the Studio front door and report every
deviation from the contract in docs/qualification/dlm-question-corpus.json.

    kubectl --context kaveon-test-aks -n kaveon port-forward svc/kaveon-portal 13015:3000
    python scripts/qualify-dlm-questions.py --portal http://127.0.0.1:13015 [--only F01,F02] [--execute-live]

Each question is asked exactly as a person would: follow-ups carry the frame of
the question they follow; a resumed clarification re-posts the original
question with the chosen slot pinned. `--execute-live` also runs the SQL the
DLM produced (through /sql/engine or /sql/execute) and records rows and
latency, which is what a user experiences in Chat.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
CORPUS = ROOT / "docs" / "qualification" / "dlm-question-corpus.json"


def check(expect: dict[str, Any], answer: dict[str, Any], live: dict[str, Any] | None) -> list[str]:
    """Every way the answer departs from the contract, in words."""
    problems: list[str] = []
    reason = answer.get("reason")
    if "ok" in expect and bool(answer.get("ok")) != expect["ok"]:
        problems.append(f"ok={answer.get('ok')} (reason={reason}) expected ok={expect['ok']}")
    if "reason" in expect and reason != expect["reason"]:
        problems.append(f"reason={reason} expected {expect['reason']}")
    if "reason_in" in expect and reason not in expect["reason_in"]:
        problems.append(f"reason={reason} expected one of {expect['reason_in']}")
    if "dataset" in expect and str(answer.get("dataset_id")) != expect["dataset"]:
        problems.append(f"dataset={answer.get('dataset_id')} expected {expect['dataset']}")
    if not answer.get("ok"):
        return problems
    route = "context" if answer.get("from_context") else "live"
    if "route" in expect and route != expect["route"]:
        problems.append(f"route={route} expected {expect['route']}")
    frame = answer.get("frame") or {}
    if "metric" in expect and frame.get("metric") != expect["metric"]:
        problems.append(f"metric={frame.get('metric')} expected {expect['metric']}")
    if "group" in expect and (frame.get("group_col") or "") != expect["group"]:
        problems.append(f"group={frame.get('group_col')} expected {expect['group']}")
    if "year" in expect and frame.get("year") != expect["year"]:
        problems.append(f"year={frame.get('year')} expected {expect['year']}")
    if "filters" in expect:
        got = sorted((f.get("column"), str(f.get("value"))) for f in (frame.get("filters") or []))
        want = sorted((c, str(v)) for c, v in expect["filters"])
        if got != want:
            problems.append(f"filters={got} expected {want}")
    if "sort" in expect and (("asc" if frame.get("sort_asc") else "desc") != expect["sort"]):
        problems.append(f"sort={'asc' if frame.get('sort_asc') else 'desc'} expected {expect['sort']}")
    if "sql_contains" in expect:
        for fragment in expect["sql_contains"]:
            if fragment not in (answer.get("sql") or ""):
                problems.append(f"sql lacks {fragment!r}")
    if "note_contains" in expect and expect["note_contains"] not in (answer.get("note") or ""):
        problems.append(f"note={answer.get('note')!r} lacks {expect['note_contains']!r}")
    rows = answer.get("rows") if answer.get("from_context") else (live or {}).get("rows")
    if rows is not None:
        if "rows" in expect and len(rows) != expect["rows"]:
            problems.append(f"rows={len(rows)} expected {expect['rows']}")
        if "first_row" in expect and (not rows or [str(v) for v in rows[0][:len(expect["first_row"])]] != [str(v) for v in expect["first_row"]]):
            problems.append(f"first_row={rows[0] if rows else None} expected {expect['first_row']}")
        if "first_value" in expect and (not rows or str(rows[0][-1]) != str(expect["first_value"])):
            problems.append(f"first_value={rows[0][-1] if rows else None} expected {expect['first_value']}")
    elif any(k in expect for k in ("rows", "first_row", "first_value")):
        problems.append("no rows to check (live SQL not executed)")
    return problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--portal", default="http://127.0.0.1:13015")
    parser.add_argument("--corpus", type=Path, default=CORPUS)
    parser.add_argument("--only", help="comma-separated question ids")
    parser.add_argument("--execute-live", action="store_true")
    parser.add_argument("--report", type=Path, default=ROOT / "tmp" / "dlm-question-report.json")
    args = parser.parse_args()
    corpus = json.loads(args.corpus.read_text(encoding="utf-8"))
    questions = corpus["questions"]
    if args.only:
        wanted = set(args.only.split(","))
        questions = [q for q in questions if q["id"] in wanted]

    from playwright.sync_api import sync_playwright
    with sync_playwright() as playwright:
        request = playwright.request.new_context(base_url=args.portal.rstrip("/"), timeout=600_000)
        try:
            config = request.get("/api/auth/entra-config").json()
            az = "az.cmd" if os.name == "nt" else "az"
            auth = subprocess.run([az, "account", "get-access-token", "--tenant", config["tenantId"],
                                   "--scope", config["scope"], "-o", "json"], capture_output=True, text=True, check=True)
            token = json.loads(auth.stdout)["accessToken"]
            csrf = request.get("/api/auth/csrf").json()["csrfToken"]
            response = request.post("/api/auth/callback/entra-public",
                                    form={"csrfToken": csrf, "token": token, "callbackUrl": f"{args.portal.rstrip('/')}/"},
                                    headers={"X-Auth-Return-Redirect": "1"})
            if not response.ok:
                raise RuntimeError("Portal sign-in failed")

            def api(method: str, path: str, body: dict[str, Any] | None = None) -> Any:
                result = request.fetch(f"/api/kaveon/api/v1/{path}", method=method, data=body)
                if not result.ok:
                    return {"_http": result.status, "_text": result.text()[:300]}
                return result.json()

            answers: dict[str, dict[str, Any]] = {}
            results = []
            for q in questions:
                body: dict[str, Any] = {"question": q["q"]}
                if q.get("follow_up_of"):
                    prev = answers.get(q["follow_up_of"]) or {}
                    body["frame"] = prev.get("frame")
                if q.get("resume_of"):
                    prev = answers.get(q["resume_of"]) or {}
                    resume = prev.get("resume") or {}
                    body["question"] = resume.get("question") or q["q"]
                    body["choices"] = dict(resume.get("choices") or {}, **{q["choose"]["kind"]: q["choose"]["id"]})
                t0 = time.time()
                answer = api("POST", "dlm/ask", body)
                seconds = round(time.time() - t0, 2)
                answers[q["id"]] = answer if isinstance(answer, dict) else {}
                live = None
                if args.execute_live and answer.get("ok") and answer.get("sql") and not answer.get("from_context"):
                    t1 = time.time()
                    if answer.get("engine"):
                        executed = api("POST", "sql/engine", {"sql_text": answer["sql"], "database": answer.get("database"),
                                                              "dataset_id": int(answer["dataset_id"]), "source": "qualification"})
                    else:
                        executed = api("POST", "sql/execute", {"sql_text": answer["sql"], "database": answer.get("database") or "kaveon", "source": "qualification"})
                    if "_http" in executed:
                        live = {"seconds": round(time.time() - t1, 2), "error": f"HTTP {executed['_http']} {executed['_text']}"}
                    else:
                        live = {"seconds": round(time.time() - t1, 2), "rows": executed.get("rows") or executed.get("data") or []}
                problems = check(q["expect"], answer, live)
                if "_http" in answer:
                    problems.insert(0, f"HTTP {answer['_http']} {answer['_text']}")
                if live and live.get("error"):
                    problems.append(f"live: {live['error']}")
                results.append({
                    "id": q["id"], "group": q["group"], "question": body["question"], "seconds": seconds,
                    "ok": answer.get("ok"), "reason": answer.get("reason"), "dataset": answer.get("dataset_id"),
                    "route": ("context" if answer.get("from_context") else "live") if answer.get("ok") else None,
                    "frame": answer.get("frame"), "sql": answer.get("sql"),
                    "clarification": answer.get("clarification"), "note": answer.get("note"),
                    "rows": (answer.get("rows") or [])[:3] if answer.get("from_context") else ((live or {}).get("rows") or [])[:3],
                    "live_seconds": (live or {}).get("seconds"), "problems": problems,
                })
                mark = "PASS" if not problems else "FAIL"
                print(f"{mark} {q['id']:4} {seconds:5.2f}s {(results[-1]['route'] or answer.get('reason') or '-'):11} {q['q'][:60]:60} {'; '.join(problems)[:140]}", flush=True)
        finally:
            request.dispose()

    failed = [r for r in results if r["problems"]]
    summary = {"asked": len(results), "passed": len(results) - len(failed), "failed": len(failed),
               "by_group": {}, "results": results}
    for r in results:
        g = summary["by_group"].setdefault(r["group"], {"passed": 0, "failed": 0})
        g["failed" if r["problems"] else "passed"] += 1
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(f"\n{summary['passed']}/{summary['asked']} passed; report {args.report}")
    return 0 if not failed else 1


if __name__ == "__main__":
    sys.exit(main())
