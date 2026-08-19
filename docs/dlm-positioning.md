# DLM — One-Pager (proven, running in prod)

**DLM = Data Language Model.** A per-dataset compiled context artifact that answers natural-language questions **with no hosted LLM — and, for the common cases, no database scan at all.** Compile once; answer from context until the data changes.

*Need the email-length version to paste into a note? See [dlm-positioning-brief.md](dlm-positioning-brief.md).*

> **In one line:** Fabric makes the query fast; Fabric IQ makes it conversational with an LLM in the loop. Kaveon makes it conversational with **no LLM in the loop — and, for the common questions, no database query either.** It answers **10 million rows in ~1.5 seconds from precompiled context**, on a tiny box, and only touches the warehouse when the question is genuinely novel — and it's running in production today.

---

## Proven — this is a running demo, not a pitch

Live at kaveon.vercel.app over a **10.1M-row** synthetic usage dataset.

| Claim | Result (measured) |
|---|---|
| "What is current Kaveon usage?" (SUM over **10.1M rows**) | **15s live → ~1.5s from context** — the 10M-row scan is **eliminated** |
| Where the answer comes from | a **precomputed lookup** (sub-ms); the residual ~1.5s is routing/resolution, now isolated on its own DB |
| Single-dimension filters ("queries for Enterprise") | served from the by-plan breakdown — **still no DB trip** |
| Novel slice ("queries by plan **in 2026**") | falls to **one** warehouse query, honestly labeled "Live query", then cached |
| Robustness (47-case adversarial battery) | **0 crashes, 0 SQL errors, 0 injection leaks** |
| Hosted LLM calls per question | **0** · data egress: **0** |

**Compute-once, answer-many:** at generate time Kaveon precomputes **every metric's grand total + each per-dimension breakdown** (a handful of scans). After that, totals, breakdowns, and single-dimension filters all serve from context — the DB is touched only when the data changes.

**Non-additive metrics are safe:** `COUNT DISTINCT` / `AVG` are computed *independently per shape* — never derived by summing a breakdown — so "Active Users for Enterprise" is exact, not an illegal roll-up.

**Data planes are physically separated (shipped):** a tiny **control + context DB** serves the answers; a separate **warehouse** holds the rows. That's why "runs on a $20/mo box" is now *literal* — context answers never contend with a 10M-row scan.

**100% transparent:** every answer is badged **"⚡ From context · no DB scan"** or **"Live query · Xs"**, with the real timing — no faking either way.

---

## See it for yourself — [kaveon.vercel.app](https://kaveon.vercel.app)

On the homepage (over the 10.1M-row demo dataset), these questions each return an answer badged with its source and timing — so the numbers above are verifiable, not asserted:

1. **"What is current Kaveon usage?"** → **⚡ From context · ~1.5s · no DB scan** — 53M queries across 10.1M rows.
2. **"queries by plan"**, **"active users by region"**, **"top 10 orgs by dashboard views"** → instant breakdowns, still from context.
3. **"queries by plan in 2026"** (a slice that was not precomputed) → **Live query · Xs** — one warehouse trip, honestly labeled, then cached.
4. Open the dataset's **Context panel** to see when context was last generated, how long it took, and how many answers were precomputed — the one-time cost that buys everything above.

---

## The three Fabric capabilities this gets confused with

| Fabric capability | What it does | Relation to DLM |
|---|---|---|
| **Fast query** (Direct Lake, VertiPaq, result cache, MVs) | makes *executing a known query* fast | **Complementary, different layer.** It never touches "given this English, *which* query." Kaveon's live-data fallback is fast *because* of it. |
| **Proactive / automatic statistics** | histograms + distinct counts for the **query optimizer** | **Same raw material, different product** — Kaveon consumes it for *retrieval + answer-validity*, not plan selection. |
| **Fabric IQ / Copilot / Data Agents** | NL → DAX/SQL via a **hosted LLM** | **The real comparison** (below). |

**Isn't this just proactive stats?** No. Fabric maintains that signal to pick a query plan; Kaveon repurposes the same cheap catalog to (1) resolve NL terms to columns/values (`anthropic` → `provider='Anthropic'`, zero scan) and (2) decide whether a cached answer is still valid — a second use for a signal Fabric already pays to maintain, a strength rather than a gap.

---

## Where Kaveon wins vs. Fabric IQ — and where it doesn't

Kaveon does not beat Fabric IQ on open-ended *reasoning* ("why did revenue drop") — that is a frontier LLM plus a large team, and Fabric IQ wins there. Kaveon wins on economics and constraints:

- **No hosted LLM / no data egress** — answered on-box. Privacy, compliance, air-gap, cost, latency.
- **Compute-once, answer-many** — Fabric IQ makes an LLM call *per question*; Kaveon amortizes to a precompute + a lookup. Proven: 10M rows in ~1.5s, **no per-query scan or LLM**.
- **Portable & open** — runs over Postgres, MySQL, StarRocks, Azure SQL, **and** Fabric. Fabric IQ is Fabric-only.
- **Deterministic & auditable** — same question → same answer, with an explainable trace. Injection-safe by construction (SQL is assembled only from escaped index values + defined expressions + quoted identifiers).

---

## Architecture (as shipped)

```
[Compiler: "Generate DLM" = encode, not train]        ← one-time, transparent (dataset Context panel shows duration)
        │ scans the warehouse a handful of times
        ▼
[Per-dataset DLM in the tiny CONTEXT DB]
   • value index      value → column/filter        ("anthropic" → provider='Anthropic')
   • precomputed answers  totals + per-dim breakdowns   ← the compute-once store
   • router           question → which dataset(s)
        │
        ├── matches a precomputed shape ──▶ ANSWER FROM CONTEXT   (dict hit, no DB scan)   ⚡
        │
        └── novel slice/combo ──▶ deterministic assembler → ONE warehouse query → cache   (Live query, honest)
```

**Bottom line:** Kaveon sits **above** fast query and **beside** Fabric IQ — the deterministic, private, **zero-scan-for-the-common-case** path for the 80% "fetch the right data" question. It is not a slide; it is answering 10 million rows in ~1.5 seconds in prod today.
