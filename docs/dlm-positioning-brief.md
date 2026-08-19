# DLM — the short version

*Email-length brief. For the full argument (Fabric-capability breakdown, architecture, where we don't win), see [dlm-positioning.md](dlm-positioning.md).*

**In one line:** Fabric makes the query fast; Fabric IQ makes it conversational with an LLM in the loop. Kaveon makes it conversational with **no LLM in the loop — and, for the common questions, no database query either.** It answers 10 million rows in ~1.5 seconds from precompiled context, on a tiny box, and only touches the warehouse when the question is genuinely novel — running in production today.

**Proven** — measured live at [kaveon.vercel.app](https://kaveon.vercel.app) over a 10.1M-row dataset:

- "Current usage" (a SUM over 10.1M rows): **15s live → ~1.5s from context** — the scan is eliminated.
- Totals, per-dimension breakdowns, and single-dimension filters: **served from context, zero DB trip.**
- Only genuinely novel slices hit the warehouse — **one** query, honestly labeled "Live query," then cached.
- **0** hosted-LLM calls per question · **0** data egress · 47-case adversarial battery: **0 crashes, 0 injection leaks.**

**How it sits next to Fabric:**

- **Fast query** (Direct Lake / VertiPaq / result cache) — complementary. That's *executing* a known query; we sit above it, on deciding *which* query.
- **Proactive statistics** — same raw signal, different use: we repurpose it for retrieval + answer-validity, not query-plan selection.
- **Fabric IQ / Copilot** — the real comparison. It calls a hosted LLM *per question*; we amortize to a one-time precompute + a lookup. It wins on open-ended reasoning; we win on cost, privacy, determinism, and portability (runs over Postgres, MySQL, StarRocks, Azure SQL — **and** Fabric).

**See it:** open [kaveon.vercel.app](https://kaveon.vercel.app) and ask *"what is current Kaveon usage?"* then *"queries by plan in 2026."* Every answer shows its source (context vs live) and its timing — so the numbers above are verifiable, not asserted.

**Where we don't win:** open-ended reasoning ("why did revenue drop") is Fabric IQ's frontier-LLM turf. We own the 80% "fetch the right data" question — deterministically, privately, and at near-zero marginal cost.
