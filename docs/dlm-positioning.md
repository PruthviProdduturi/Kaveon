# DLM vs. Fabric — One-Pager

**DLM = Data Language Model.** A per-dataset, compiled context artifact that resolves natural-language questions to the right columns and filters **with no hosted LLM in the loop and no data scan** — then answers from that context until the data changes.

> **Opening line for the room:**
> *"Fabric makes the query fast, and Fabric IQ makes it conversational with an LLM in the loop. We make it conversational with **no LLM in the loop** — compile the dataset's context once, resolve and answer deterministically on-box, and only touch live data when our staleness score says it moved. It's the private, portable, zero-marginal-cost complement to fast query, not a competitor to it."*

---

## The three Fabric capabilities this gets confused with — and where each sits

| Fabric capability | What it actually does | Overlap with DLM |
|---|---|---|
| **Fast query** — Direct Lake, VertiPaq, result-set cache, MVs | makes *executing a known query* fast | **None.** It answers "given this SQL/DAX, return rows." It never touches "given this English, *which* query." Different layer — **complementary**: our live-data fallback is fast *because* of it. |
| **Proactive / automatic statistics** | histograms + distinct counts fed to the **query optimizer** for plan selection | **Same raw material, different product.** We consume that signal for *retrieval* and *answer-validity*, not plan selection. |
| **Fabric IQ / Copilot / Data Agents** | NL → DAX/SQL via a **hosted LLM** over the semantic model | **The real comparison.** Different economics and constraints (below). |

---

## "Isn't this just proactive stats?" — the sharpest jab, pre-empted

Yes — and we consume exactly that signal on purpose. Fabric maintains those statistics to **pick a query plan**. We repurpose the same cheap catalog — histograms, distinct counts, mod-since-analyze — for two things the optimizer never does with them:

1. **Resolve a natural-language term to the right column + filter value** (`"anthropic"` → `provider = 'Anthropic'`), from `most_common_vals` — zero scan.
2. **Score whether a cached *answer* is still valid**, or the data moved underneath it.

> Same proactive stats. Aimed at **retrieval + answer-validity** instead of plan selection. That's a *second use for a signal Fabric already pays to maintain* — a strength, not a gap.

---

## Two different "precompute" claims — state them separately (don't conflate)

Conflating these is the one way to get dismantled. There are **two** precomputations, and they answer different things:

1. **Precompute the retrieval substrate (the DLM).** Compile each dataset's structure + value inventory + usage once, so **any** question — including unseen ones — resolves to columns/filters and assembles to SQL **without an LLM**.
2. **Cache validated answers (`context_answer_cache`).** For **repeated** questions, skip execution entirely and serve the stored answer — invalidated by the **staleness score**, not a TTL, so it self-heals when data changes.

> #1 handles novel questions cheaply. #2 makes repeats free. "We *always* answer from context" over-promises — you can't precompute answers to arbitrary unseen questions. Say both, separately, and the position is bulletproof.

---

## Where we win vs. Fabric IQ — and where we don't (be honest)

**Do not claim** to beat Fabric IQ on open-ended *reasoning* ("why did revenue drop, what's driving it"). It's a frontier LLM + a large team; it wins there. We fight where the economics and constraints differ:

- **No hosted LLM / no data egress.** Fabric IQ calls Azure OpenAI per question; we answer on-box. → privacy, compliance, air-gap, cost, latency.
- **Compute-once, answer-many.** Fabric IQ regenerates every question (an LLM call each time). We compile once and answer from context until data changes. → different unit economics: theirs is per-query marginal cost, ours is amortized.
- **Portable & open.** DLM runs over Postgres, MySQL, StarRocks, Azure SQL, **and** Fabric. Fabric IQ is Fabric-only.
- **Deterministic & auditable.** Same question → same SQL, with an explainable trace ("resolved 'anthropic' → `provider='Anthropic'` via value index"). LLM NL→SQL is non-deterministic and hard to govern.

---

## The pipeline (one glance)

```
[Global compressor]            ← the invention (trained once; v2 = discrete RQ-VAE codes)
        │ applied per dataset ("Generate DLM" = encode, not train)
        ▼
[Per-dataset DLM]  = compiled(structure + value inventory + stats + usage)
        │ summaries feed upward
        ▼
[DLM-over-DLMs router]  = question → which dataset(s)
        │
        ▼
[Deterministic assembler]  → SQL      (no LLM)
        │
        ▼
[Answer cache]  = staleness-validated; serve-from-context until data moves
```

**Bottom line:** we sit **above** fast query and **beside** Fabric IQ — the deterministic, private, zero-marginal-cost path for the 80% "fetch the right data" case, escalating to live data only when our staleness score says the world changed.
