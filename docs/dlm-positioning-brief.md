# DLM — the short version

*One-screen brief — full version: [dlm-positioning.md](dlm-positioning.md).*

**In one line:** Fabric makes the query fast; Fabric IQ makes it conversational with an LLM in the loop. Kaveon does it with **no LLM — and, for common questions, no database query either**: 10M rows answered in ~1.5s from precompiled context, hitting the warehouse only when the question is genuinely novel. Live in production today.

**Proven** — measured live at [kaveon.vercel.app](https://kaveon.vercel.app) over a 10.1M-row dataset:

- "Current usage" (a SUM over 10.1M rows): **15s live → ~1.5s from context** — the scan is eliminated.
- Totals, per-dimension breakdowns, and single-dimension filters: **served from context, zero DB trip.**
- Only genuinely novel slices hit the warehouse — **one** query, honestly labeled "Live query," then cached.
- **0** hosted-LLM calls per question · **0** data egress · 47-case adversarial battery: **0 crashes, 0 injection leaks.**

**How it sits next to Fabric:**

- **Fast query** (Direct Lake / VertiPaq / result cache) — complementary. That's *executing* a known query; Kaveon sits above it, on deciding *which* query.
- **Proactive statistics** — same raw signal, different use: Kaveon repurposes it for retrieval + answer-validity, not query-plan selection.
- **Fabric IQ / Copilot** — the real comparison. It calls a hosted LLM *per question*; Kaveon amortizes to a one-time precompute + a lookup. Fabric IQ wins on open-ended reasoning; Kaveon wins on cost, privacy, determinism, and portability (runs over Postgres, MySQL, StarRocks, Azure SQL — **and** Fabric).

**See it:** open [kaveon.vercel.app](https://kaveon.vercel.app) and ask *"what is current Kaveon usage?"* then *"queries by plan in 2026."* Every answer shows its source (context vs live) and its timing — so the numbers above are verifiable, not asserted.

**Where it doesn't win:** open-ended reasoning ("why did revenue drop") is Fabric IQ's frontier-LLM turf. Kaveon owns the 80% "fetch the right data" question — deterministically, privately, and at near-zero marginal cost.
