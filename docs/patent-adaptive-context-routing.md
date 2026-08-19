# Patent — Adaptive Context-Based Query Routing Using Data Staleness Scoring

**Inventor:** Pruthvi Prodduturi
**Status:** draft claim set for filing
**Related:** `whitepaper-adaptive-context-routing.md` (technical disclosure),
`whitepaper-nl-to-sql.md` (deterministic translation layer)

> Drafting notes and rationale are quoted like this and are **not** part of the
> claims. They record why each limitation is worded as it is, and the prior-art
> delta each is meant to hold.

---

## Field

Methods and systems for routing natural-language questions over structured data
between a cached context representation and live database execution, based on a
measured data-staleness score computed per context element.

---

## Independent Claim 1 (method)

A computer-implemented method comprising:

**(a)** generating, without use of a large language model, a global context
representation of a structured dataset by statistically profiling column value
distributions, inferred relationships between tables, and query pattern frequency,
wherein the column value distributions are obtained by reading statistics
maintained by the database management system about the dataset **rather than by
scanning the dataset's rows**;

**(b)** capturing, for each of a plurality of context elements of the context
representation, one or more data-change indicators reported by the database
management system, the data-change indicators comprising at least a count of row
modifications and a time of last statistics computation, each context element
comprising one of a table or a column;

**(c)** assigning a validity score to individual context elements, the validity
score computed from a combination of (i) elapsed time since the context element
was last refreshed, (ii) a magnitude of detected change in the underlying data of
the context element since the context element was generated, **the magnitude of
detected change being determined from a change in the said data-change indicators
without re-querying the underlying data**, and (iii) a decay function weighted by
how frequently the context element has been relied upon for prior answers;

**(d)** receiving a natural-language question from a user;

**(e)** determining, without use of a large language model, a set of context
elements relevant to the natural-language question by matching terms of the
question against names of the tables and columns of the context representation;

**(f)** determining, using the validity scores of the set of context elements
relevant to the question and not the validity of the dataset as a whole, whether
to answer the question from the context representation or to trigger execution of
a live database query;

**(g)** responsive to determining to trigger a live query, generating and
executing the query, updating the captured data-change indicators and the validity
scores of the affected context elements to a refreshed state, caching a result of
the query in association with the question and the set of context elements the
result depends upon, and returning the result; and

**(h)** responsive to determining to answer from the context representation,
returning an answer without query execution.

> **Why (a) adds "reading statistics … rather than scanning."** The original
> claim said "statistically profiling column value distributions" but not how.
> Grounding it in reading DBMS-maintained statistics (a) states a concrete,
> enabling mechanism and (b) draws the novelty line against any approach that
> profiles by scanning or sampling data.
>
> **Why (b) is a separate step.** Capturing the change indicators is the
> enablement pivot for (c)(ii). Making it an explicit limitation forecloses the
> reading that change is detected by re-computing statistics (i.e., by querying),
> which would defeat the invention.
>
> **Why (c)(ii) says "without re-querying the underlying data."** This is the
> load-bearing distinction from a materialized-view/incremental-maintenance system
> and the answer to the enablement question "how do you know the data changed
> without looking at it?" — you read a counter the DBMS already maintains.
>
> **Why (e) is added.** The original independent claim assumed the system could
> identify "the specific context elements relevant to the question" but never
> claimed the step. Claiming it (deterministically, no LLM) keeps the no-LLM story
> internally consistent and adds a limitation competitors must design around.
>
> **Why (f) says "and not the validity of the dataset as a whole."** Per-element
> granularity is the core differentiator from a TTL/whole-cache scheme; stating it
> in the independent claim protects it directly rather than leaving it to a
> dependent.

---

## Dependent Claims

**2.** The method of claim 1, wherein the decay function of (c)(iii) shortens an
effective half-life of factor (c)(i) as the frequency of reliance on the context
element increases, such that context elements relied upon more frequently are
assigned a lower validity score at equal elapsed time and equal detected change.

> Fixes the ambiguity in the original "weighted by how frequently relied upon":
> pins the *direction* (hot decays faster) so the claim is definite.

**3.** The method of claim 1, wherein determining whether to answer from the
context representation or trigger a live query comprises comparing a minimum
validity score among the set of relevant context elements against a configurable
threshold, and selecting the context representation only when the minimum validity
score satisfies the threshold.

> Supplies the decision *mechanism* the original claim's "determining whether"
> lacked.

**4.** The method of claim 3, wherein, when a first subset of the relevant context
elements satisfies the threshold and a second subset does not, the method answers a
first portion of the question from the context representation for the first subset
and triggers a live query only for the second subset.

> The hybrid/partial-answer path — the strongest independent-of-prior-art
> dependent, and hard to design around because it exploits the per-element graph.

**5.** The method of claim 1, wherein the data-change indicators comprise a running
count of row modifications since a last statistics computation reported by the
database management system, and the magnitude of detected change is computed as a
function of a change in said count divided by a row count captured at generation
time.

> Nails factor (b) to a concrete, named signal (`n_mod_since_analyze`-class
> counters) and a concrete formula, strengthening enablement.

**6.** The method of claim 5, wherein a change in the time of last statistics
computation is treated as an independent indicator of data change even when the
running count of row modifications has been reset.

> Covers the autovacuum/ANALYZE reset case so a competitor can't evade (5) by
> relying on counter resets.

**7.** The method of claim 1, wherein answering from the context representation
comprises synthesizing the answer directly from the profiled column value
distributions without executing any query, for questions requesting one of a row
count, an approximate count of distinct values, a null fraction, or a most-common
value.

> Claims the profile-synthesised-answer path (answer *is* the statistic), a
> no-query-ever behavior a result cache cannot provide.

**8.** The method of claim 7, wherein an answer synthesized from a distinct-value
estimate is returned with an indication that the answer is approximate.

> The confidence/uncertainty output, attached where it actually applies.

**9.** The method of claim 1, wherein answering from the context representation
comprises returning a previously cached result of a prior live query, and the
cached result is returned only while the validity score of every context element
the cached result depends upon satisfies a threshold.

> Claims the dependency-valid cache — the precise upgrade over a TTL cache.

**10.** The method of claim 1, wherein the inferred relationships between tables
comprise foreign-key relationships read from the database catalog and,
additionally, relationships inferred by matching a column name and type against a
primary key of another table in the absence of a declared foreign key.

> Covers the FK-inference heuristic for warehouses that drop constraints.

**11.** The method of claim 1, wherein updating the validity scores of the affected
context elements to a refreshed state in response to the live query causes a
subsequent identical question to be answered from the context representation until
a further change is detected in the data-change indicators of those elements.

> Claims the self-healing feedback loop: stale→query→refresh→context.

**12.** The method of claim 1, wherein the query pattern frequency is derived from a
stored history of previously executed queries and the tables each executed query
referenced, and is used to compute the reliance frequency of factor (c)(iii).

> Grounds "query pattern frequency" and ties it to factor (c).

**13.** The method of claim 1, further comprising periodically or on-demand
re-generating the context representation for a dataset in response to a user action
associated with the dataset in a user interface, without executing a query that
scans the dataset's rows.

> Covers the "Build Context" UI action — an on-demand profile trigger next to the
> dataset/data-source, reading statistics only.

---

## Independent Claim 14 (system)

A system comprising one or more processors and memory storing instructions that,
when executed, cause the system to perform the method of claim 1.

## Independent Claim 15 (medium)

A non-transitory computer-readable medium storing instructions that, when executed
by one or more processors, cause the processors to perform the method of claim 1.

> Standard system + CRM independent claims so the invention is protected in all
> three statutory categories.

---

## Dependent Claims — Productization (precomputed answer store & separated planes)

> These claims were added after reduction to practice. They capture two elements
> that shipped in the productized embodiment (the "DLM"): a **precomputed
> per-metric/per-dimension answer store** that answers common questions with no
> query at all, and **physical separation** of the context/answer store from the
> data warehouse. Numbered from 16 to avoid disturbing claims 1–15.

**16.** The method of claim 1, wherein generating the context representation further
comprises precomputing, without a live user query and for each of a plurality of
predefined aggregate metrics of the dataset, (i) a grand aggregate value of the
metric over the dataset and (ii) a set of per-dimension breakdown values grouping
the metric by each of one or more dimension columns, and storing said precomputed
values in an answer store keyed by metric and by dimension; and wherein answering
from the context representation comprises returning one of said precomputed values
without executing a query.

> Claims compute-once/answer-many. This is a distinct no-query path from the
> statistic-synthesised answers of claim 7: it precomputes *defined-metric*
> aggregates and breakdowns at build time, not just DBMS-maintained statistics.

**17.** The method of claim 16, wherein answering a question that specifies an
equality filter on a single dimension comprises selecting, from the stored
per-dimension breakdown for that dimension, a row matching the filter value, and
returning the selected value without executing a query.

> The single-dimension-filter-from-breakdown path: a filtered answer served with no
> scan, by indexing into a precomputed breakdown.

**18.** The method of claim 16, wherein each of the grand aggregate value and the
per-dimension breakdown values is computed independently at generation time, such
that a non-additive metric comprising one of a count of distinct values or an
average is returned exactly and is not derived by aggregating the per-dimension
breakdown values.

> Forecloses the incorrect-roll-up failure mode and claims correctness for
> non-additive metrics — a limitation a naive precompute would violate.

**19.** The method of claim 16, wherein the precomputed values for a dataset are
loaded into an in-memory cache upon first access and subsequent answers for that
dataset are served from the in-memory cache, the in-memory cache being invalidated
upon re-generation of the context representation for that dataset.

> Claims the warmed in-memory answer cache and its invalidation on regeneration.

**20.** The method of claim 1, wherein the context representation, the captured
data-change indicators, and any precomputed answer store are stored in a first
database that is physically separate from a second database storing the underlying
rows of the dataset, such that answering from the context representation reads only
the first database and does not contend with the second database.

> Claims the physical control/context-vs-warehouse separation: the reason a
> common-case answer is served from a small, fast store while the warehouse scales
> independently. Distinct from any single-store cache.

**21.** The method of claim 1, further comprising returning, with each answer, an
indicator of whether the answer was produced from the context representation
without executing a query or from a live database query, together with a measured
time taken to produce the answer.

> Claims the honest source/latency labelling surfaced to the user — a per-answer
> provenance signal the prior-art caches do not expose.

---

## Prior-Art Delta (summary for prosecution)

| Reference class | What it teaches | What claim 1 adds |
|---|---|---|
| TTL result cache | expire cached answers on a clock | invalidation by **measured per-element data change**, not time |
| Materialized view / incremental view maintenance | keep a view fresh as base tables change | no view maintenance; **route** a NL question, answer from statistics with **no query**, per-element |
| Query-plan statistics (`pg_stats`, histograms) | help the optimizer choose plans | repurpose the statistics as a **user-facing context representation** and as a **staleness signal** |
| LLM RAG over embeddings | retrieve context by semantic similarity | context built **without an LLM**; freshness by **counter**, not similarity; routes to live SQL |
| Cache invalidation via triggers/CDC | invalidate on write events | **score** staleness continuously and **per element relevant to a question**, and answer partially (hybrid) |
| Precomputed aggregate / OLAP cube | precompute aggregates for fast reads | precompute is **gated and invalidated by the measured per-element staleness score** and served from a **plane physically separate** from the rows; answers a NL question with **no query**, with per-answer provenance (claims 16–21) |

The defensible core is the **combination**: a per-element validity score whose
change term is read from a DBMS-maintained modification counter (not a re-query),
used to route a natural-language question between a no-query context answer and a
live query, over a context representation built without a large language model.
