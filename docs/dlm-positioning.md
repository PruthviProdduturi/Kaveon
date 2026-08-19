# DLM — One-Pager (proven, running in prod)

**DLM = Data Language Model.** A per-dataset compiled context artifact that answers natural-language questions **with no hosted LLM — and, for the common cases, no database scan at all.** Compile once; answer from context until the data changes.

> **Opening line for the room:**
> *"Fabric makes the query fast; Fabric IQ makes it conversational with an LLM in the loop. We make it conversational with **no LLM in the loop — and for the common questions, no database query either.** We answer **10 million rows in ~1.5 seconds from precompiled context**, on a tiny box, and only touch the warehouse when the question is genuinely novel."*

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

**Compute-once, answer-many:** at generate time we precompute **every metric's grand total + each per-dimension breakdown** (a handful of scans). After that, totals, breakdowns, and single-dimension filters all serve from context — the DB is touched only when the data changes.

**Non-additive metrics are safe:** `COUNT DISTINCT` / `AVG` are computed *independently per shape* — never derived by summing a breakdown — so "Active Users for Enterprise" is exact, not an illegal roll-up.

**Data planes are physically separated (shipped):** a tiny **control + context DB** serves the answers; a separate **warehouse** holds the rows. That's why "runs on a $20/mo box" is now *literal* — context answers never contend with a 10M-row scan.

**100% transparent:** every answer is badged **"⚡ From context · no DB scan"** or **"Live query · Xs"**, with the real timing — no faking either way.

---

## 60-second live demo (what to click)

1. *"What is current Kaveon usage?"* → **⚡ From context · ~1.5s · no DB scan** — 53M queries across 10.1M rows.
2. *"queries by plan"*, *"active users by region"*, *"top 10 orgs by dashboard views"* → instant breakdowns, still from context.
3. *"queries by plan **in 2026**"* (a slice we didn't precompute) → **Live query · Xs**, one warehouse trip, then cached. Honest about the cost.
4. Open the dataset's **Context panel**: *"last generated · took Xs · N precomputed answers"* — the one-time cost that buys everything above (and it's transparent about *why* it took that long).

---

## The three Fabric capabilities this gets confused with

| Fabric capability | What it does | Relation to DLM |
|---|---|---|
| **Fast query** (Direct Lake, VertiPaq, result cache, MVs) | makes *executing a known query* fast | **Complementary, different layer.** It never touches "given this English, *which* query." Our live-data fallback is fast *because* of it. |
| **Proactive / automatic statistics** | histograms + distinct counts for the **query optimizer** | **Same raw material, different product** — we consume it for *retrieval + answer-validity*, not plan selection. |
| **Fabric IQ / Copilot / Data Agents** | NL → DAX/SQL via a **hosted LLM** | **The real comparison** (below). |

**"Isn't this just proactive stats?"** — pre-empt it: *"Yes, we consume exactly that signal — but Fabric maintains it to pick a query plan; we repurpose the same cheap catalog to (1) resolve NL terms to columns/values (`anthropic` → `provider='Anthropic'`, zero scan) and (2) decide whether a cached answer is still valid. A second use for a signal Fabric already pays to maintain — a strength, not a gap."*

---

## Where we win vs. Fabric IQ — and where we don't (be honest)

**Don't** claim to beat Fabric IQ on open-ended *reasoning* ("why did revenue drop"). It's a frontier LLM + a big team; it wins there. We win on economics and constraints:

- **No hosted LLM / no data egress** — answered on-box. Privacy, compliance, air-gap, cost, latency.
- **Compute-once, answer-many** — Fabric IQ makes an LLM call *per question*; we amortize to a precompute + a lookup. Proven: 10M rows in ~1.5s, **no per-query scan or LLM**.
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

**Bottom line:** we sit **above** fast query and **beside** Fabric IQ — the deterministic, private, **zero-scan-for-the-common-case** path for the 80% "fetch the right data" question. It's not a slide; it's answering 10 million rows in ~1.5 seconds in prod today.
