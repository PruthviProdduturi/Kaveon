# Decision log

### The brand accent is an identity colour, not a label-bearing fill

- **Decision (2026-10-15).** `--accent` keeps its value and its role. Two new
  tokens carry white text instead: `--accent-strong` and `--accent-strong-deep`
  for a brand fill, `--danger-strong` for a destructive one, with `--on-accent`
  naming the label. All four are declared on bare `:root` and deliberately have
  no dark-theme override, because the contrast between a label and the fill it
  sits on does not depend on what is behind the fill.
- **Why it was a decision and not a fix.** Every primary button in the product
  put white on the identity accent, and no pairing of the existing tokens
  passes: white on `--accent` measures 2.86:1 and on `--accent-dark` 4.22:1,
  against the 4.5:1 a label needs. The gradient those buttons use runs between
  exactly those two values, so the worst text in the interface was on the
  control a user is most expected to press. Darkening `--accent` itself would
  have fixed them and changed every border, focus ring and tint built from the
  same token; putting dark ink on the accent instead, as the catalog does,
  changes what a primary button looks like. Adding a fill token changes
  neither.
- **How the values were chosen.** They are the same blue, not a different one:
  hue 208 and saturation 0.68 held fixed from `#4A9EE8`, value taken down until
  white clears. `#3571a6` carries white at 5.17:1 and `#2d608c` at 6.64:1, so
  the gradient passes at its lighter end, which is the end that binds. Light
  `--danger` (`#dc2626`) already carried white at 4.83:1; dark's (`#f87171`)
  managed 2.77:1, because that red was chosen as text on a dark ground and a
  confirm button used it as a surface -- `#c81e1e` carries white at 5.74:1 in
  both themes.
- **Evidence.** 15 surfaces changed: 11 gradient sites and one flat badge in
  `studio/styles/globals.css`, `.btn-primary` in `studio/app/globals.css`, one
  dark-theme override in `studio/styles/dashboard.css`, and `ConfirmModal`'s
  confirm button. Ratios were measured against the real grounds, not `#ffffff`
  -- the earlier mistake that put `--text-muted` at 4.52:1 on paper and 4.22:1
  on `--bg-primary`. A grep for white on an ink token across the three imported
  sheets now returns nothing; all three parse at brace depth 0; `tsc` is clean.
- **Consequence.** `--accent` is now for identity and non-text use -- borders,
  rings, tints, icon glyphs on neutral ground. Anything that places text on a
  brand or destructive fill must use the `-strong` pair and `--on-accent`, and
  a new fill colour needs its ratio measured and recorded beside it. Still open
  and still a brand decision: `--accent` cannot carry link text either (2.67:1
  on `--bg-primary`), which is why `.link-button` is a bordered control rather
  than a coloured link.


### Claude took the open Engine defects on architect instruction

- **Decision (2026-10-15).** The architect instructed Claude to fix the Engine
  defects that had been handed to Codex and were still open: D-42, D-51, D-39,
  D-25 and the Engine half of D-8 and D-3. `CLAUDE.md` assigns essentially all
  Engine crates to Codex, so this crosses that boundary and is recorded here
  because the contract requires a crossing to be recorded in the commit or the
  decision log rather than assumed.
- **Evidence.** D-42 was the reason it could not wait: `u8 as char` in
  `normalize_bracket_identifiers` is a Latin-1 conversion, so every non-ASCII
  character in every statement was corrupted, and because product records are
  written as SQL literals it was corrupting stored data once per write and
  compounding on each rewrite. It had been handed over twice with measurements
  and was still live. Commits: `00417e6e` (D-42), `e698da5f` (D-3, D-8),
  `cec6a098` (D-51, D-39, D-25).
- **Consequence.** The Engine crates now carry changes neither engineer owns
  alone, so the next Engine change must read those commits rather than assume
  the crate is untouched. Only the files each change needed were committed; the
  nineteen files holding Codex's uncommitted work were left exactly as found,
  and in `crates/sql/src/parser.rs` only Claude's own hunks were staged so an
  unrelated rustfmt wrap of Codex's stayed in the working tree. Two things are
  deliberately NOT done: repairing the data D-42 already corrupted, which needs
  a pass that counts the Latin-1 layers per value and must run after the parser
  fix is deployed or it would be re-corrupted; and collapsing a cube build's
  statement-per-cuboid, which needs `GROUPING SETS` in the sql and exec crates.

### Cube pairs are planned from observed counts, not declared caps

- **Decision (2026-10-15).** Admit a two-axis cuboid on the product of the
  counts the build actually observed, not the product of the dimensions'
  declared caps.
- **Evidence.** Under the cap rule a pair qualified only when
  `cap_left x cap_right` fit `CUBE_PAIR_MAX_CELLS` (1,000,000), and a dimension
  with no declared cap takes `DEFAULT_DIMENSION_CAP` of 10,000 — so every pair
  was planned at 100,000,000 cells and none ever qualified. On the
  1,017,865,653-row `events.daily_user_activity` this meant single-axis
  groupings answered in 0.55-0.78 s while the first execution of each distinct
  pair cost 116-123 s, with only repeats fast from the result cache.
  `platform x license` is twelve cells and was never built.
- **Consequence.** That table's build goes from twelve statements to sixty-six:
  one scan per cuboid at build time against about two minutes on the first view
  of every pair. It breaks even in the worst case and wins whenever a pair is
  viewed outside a result-cache window. The remaining lever for a cheaper build
  is multi-grouping aggregation, not a cap. An axis excluded for its cap still
  pairs with nothing, and a rebuild now keeps the groupings the previous cube
  held — without that, the first background refresh would have deleted every
  observed-only pair, because a worker-built cube always rebuilds.

### The product snapshot cache is keyed on identity, never on time

- **Decision (2026-10-15).** Cache the validated product catalog snapshot, and
  make a hit require the identity the head attests rather than a freshness
  window.
- **Evidence.** `read_current()` ran on every product call — read, list, BEGIN
  and COMMIT alike — and each time decoded the whole manifest, then read,
  SHA-256-verified and JSON-decoded every typed table in the catalog rather
  than the one being touched, then rebuilt all typed-row indexes to validate.
  A single chart save paid that pass about five times. Measured in release on a
  2,000-row typed table: `read_current` 2.86 ms to 0.63 ms, a one-mutation
  commit 5.19 ms to 2.69 ms.
- **Consequence.** Every call still reads `head.json`, so a writer always sees
  the live head and its CAS version; only the pure function from immutable
  bytes to a validated snapshot is cached, and a hit needs both the generation
  and snapshot id the head just named and a matching manifest digest. Snapshot
  objects are create-only, so an identity names one byte string forever.
  Nothing expires. If storage ever loses or alters a snapshot object the head
  still attests, the old code reported it missing and this serves the content
  whose digest the head names — the content-addressed answer, documented in the
  tests. The O(S^2) staging is unchanged and remains open: a pure batch is
  impossible while each change is derived from the running preview.

### Mixed-key final-merge partitioning remains rejected

- **Decision (2026-10-08).** Keep the final aggregate's established
  broadcast/selection path for mixed unsigned and text keys. Do not enable
  `BroadcastPartitioned` merely because a text key is present.
- **Evidence.** On the fixed 100M-row ClickBench `hits` control, enabling the
  path for q33 (`WatchID, ClientIP`) completed correctly in 20.26 s versus
  the clean ~19–20 s baseline; q35 completed in 13.55 s versus the clean
  ~12.6–13.5 s baseline. Both were slower and showed no compensating
  reduction in the qualified end-to-end result.
- **Consequence.** The unsigned-key guard remains in
  `engine/crates/server/src/fragment_exec.rs`. The experiment is not part of
  the shipped runtime; future work must reduce exchange decode and spill
  cost without adding a full-batch repartition copy to this path.

The non-obvious calls behind the Engine and the platform, dated, three
lines each — what was decided, why, and what it commits us to — with the
commit on `dev` or the HANDSHAKE Log row that records it. A call that the
repository does not record is marked **unverified** rather than dressed up.
The first entries cover 2026-09-16 to 2026-09-19, the days the learning
engine, the governance surface and the benchmark program landed; add an
entry when a choice is made that a reader of the code could not infer.
Reference pages: [The learning engine](../engine/learning-engine.md),
[Governance](../engine/governance.md),
[Storage and catalogs](../engine/storage-and-catalogs.md),
[the 9/10 program](nine-of-ten-program.md),
[the benchmark program](../qualification/benchmark-program.md).

## 2026-09-16

### Benchmarks bypass the result cache

- **Decision.** Every benchmark and qualification submitter sends
  `settings.result_cache = false` on every statement; a record whose query
  records show `execution.mode = "cache"` is invalid.
- **Reason.** The coordinator's result cache (on by default at 256 MiB,
  keyed by the normalised statement, catalog snapshot, pinned Delta
  versions and time zone) would serve a repeat of a benchmark statement in
  0 ms; a number that measures a lookup is not a number about the Engine.
- **Consequence.** `scripts/scale-suite.py`, `differential-cases.py`,
  `benchmark-rounds.py` (through the suite), `benchmark-throughput.py` and
  `engine/qualification/*.py` carry the setting; a run that forgets it is
  detectable from its records. Record: `b421c4b1` (the cache as a
  per-request setting, the scripts changed in the same commit);
  `docs/qualification/benchmark-program.md`, "No result cache"; HANDSHAKE
  Log 2026-09-17 (result cache row, "Boundary crossing you should know
  about").

## 2026-09-17

### Trino is the internal yardstick only, never on product surfaces

- **Decision.** Trino is measured beside Kaveon on the qualification
  cluster and named in the qualification records and the research pages;
  it does not appear in the product page's figures, and no Kaveon
  deployment includes it.
- **Reason.** A comparison an outsider cannot reproduce on their own
  hardware is marketing, not evidence; the campaign records (five rounds,
  medians, digests, cluster shape) are the evidence, and the product page
  carries only what a reader can rerun from the repository.
- **Consequence.** `scripts/benchmark-chart-data.py` emits Kaveon's rows
  and nothing about any other engine ("No other engine appears in the
  output"); the Trino chart is a benchmark-window fixture, never part of a
  deployment; Trino remains a named connector *target* and a research
  subject in Studio's docs section. Record: `47b7d631`; the rule in
  `docs/engineering/nine-of-ten-program.md` (`8f3be246`, "Trino is the
  internal yardstick only") and `benchmark-program.md`, "No production
  Trino" (`e2988995`).

### TPC-H is generated as Delta, with the specification's column names

- **Decision.** The SF100 tables are written by Trino's `tpch` connector
  as Delta tables (`benchmarks/tpch/v2/sf100/<table>/` on the `opensource`
  account) with `l_orderkey`, `o_custkey` and the rest of the standard
  names, and both engines register the same tables.
- **Reason.** The first generation wrote plain Parquet directories, which
  the Engine's cloud Parquet reader refused at the time (one object per
  location); Delta was the multi-file table both engines read from object
  storage. Trino's connector names columns without the table prefix
  (`orderkey`), and the 22 queries name `l_orderkey`: the standard names
  let the query text run verbatim on both engines instead of two suites.
- **Consequence.** Trino registers the tables by their logs
  (`system.register_table`) in every benchmark window and loses them on
  restart (file metastore); Kaveon registers them as Delta from the
  manifest; the directory-Parquet reader that landed the same day
  (below) does not change the record. Note: `benchmark-program.md` and
  `aks-suspend-resume.md` still print the path as
  `opensource/benchmarks/tpch/delta/sf100/`, while the Job and the
  manifest (`docs/qualification/tpch/tables.json`) use
  `benchmarks/tpch/v2/sf100/`; the manifest is what was registered.
  Record: `802ccd1a` (Delta), `fc63d6dc` and `7935ad0e` (column names,
  2026-09-18), `b9e9fcc9` (docs); HANDSHAKE Log 2026-09-17, directory
  Parquet tables row ("which is why the SF100 tables were regenerated as
  Delta").

### A directory of Parquet files is a table, under the Hive rule

- **Decision.** A catalog location that holds no object is listed once per
  scan and read as a table: names beginning with `_` or `.` at any level
  are hidden, zero-byte objects skipped, `.parquet` or extension-less
  objects are data, any other extension is an error naming the object, and
  every directory between the root and a file is a `key=value` partition
  column.
- **Reason.** This is the layout Hive, Spark and Trino write (the SF100
  Parquet generation was 82 files under `<table>/`); a table that is only
  a table when it is one object cannot be registered from a lake anyone
  else wrote. The visibility rule is Hive's and Spark's so that a
  `_SUCCESS` marker or a `.crc` never becomes data and a stray file never
  silently vanishes.
- **Consequence.** Files are spread over scan partitions by size under a
  deterministic assignment; the coordinator pins the listing per query
  (`SourcePins`) while the executable fragment still names only the
  location — a file landing between two tasks' listings is an open window
  that carrying the listing in the task will close; statistics key such a
  table by the listing digest. Record: `ef3fcef6` (object storage),
  `01abd848` (local), `846cb6ee` (partition columns, 2026-09-18);
  `engine/crates/storage/src/parquet_directory.rs` module doc; HANDSHAKE
  Log 2026-09-17, directory Parquet tables row.

### The hybrid final merge never replays its input, and spills on input-side pressure

- **Decision.** The final aggregate merges partial rows into one table
  until the budget refuses a batch, spills the table's groups as encoded
  partial rows into salted sub-partitions, and continues from the row the
  refusal fell on; nothing is read twice, and the replay path
  (`partitioned_final_aggregate`, `ReplayableFinal`) is removed rather
  than kept as a fallback. A source thread the budget refuses asks the
  merge threads for memory and a merge with groups spills for it.
- **Reason.** On AKS the in-memory merge on three threads was refused near
  the end, thrown away, and the input replayed serially through the
  sixteen-partition disk path — 38 s wall for 32 s of one thread's CPU,
  1,056 runs, 400 compactions; the groups already merged were a valid
  partial and did not need to be recomputed. Then the same shape failed
  its final tasks when the refusal was a decode thread's next batch rather
  than the merge's own (`prefetched-exchanges cannot reserve … 3.08 of
  3.22 GB already reserved`): the tables held the budget legitimately and
  nobody could give it up.
- **Consequence.** Memory stays exact (a spill reserve and a prepaid
  balance held from the first batch, the doubling guard released at
  finish); a sub-partition that does not fit fails closed, the same bound
  as before without writing the whole input first; the `Pressure`
  protocol is part of the operator contract (`FinalMergeSpill.on_pressure`
  counts the tables spilled for a source). Local figures only, not
  benchmark claims; the cluster number is still to be taken. Record:
  `8d297e48` (hybrid merge; worktree `3411cf7`), `97e7bce8` (input-side
  pressure, 2026-09-18; worktree `615eb09`), `456d5bdf` (docs);
  `engine/crates/exec/src/final_merge.rs` module doc; HANDSHAKE Log
  2026-09-17 (hybrid merge row) and 2026-09-18 (input-side pressure row).

### The test cluster does not run over the weekend

- **Decision.** The westus2 qualification cluster is taken down between
  benchmark windows and brought back for the next one; the runbook of
  2026-09-17 makes `az aks stop` / `az aks start` the procedure (managed
  disks, the catalog PVC, the PostgreSQL PVC, the storage account and the
  ACR images stay), with delete-and-recreate from the checked-in Bicep
  (`infra/bicep/environments/aks-test.bicep`) and the Helm chart kept as
  the recovery path.
- **Reason.** Three D4s_v3 workers, a coordinator and a Trino window idle
  for two days cost more than the resume; worker `emptyDir` state
  (exchange spools, spills) is disposable by design, and everything that
  is not lives on disks a stop does not touch.
- **Consequence.** Every Monday starts with the verification statements in
  `docs/engineering/aks-suspend-resume.md` before any run; a StatefulSet
  that lost a `kubectl set image` roll is recovered from the digest in the
  last run record; nothing measured on a Friday is comparable to a Monday
  without a cold round. **2026-09-18, 22:10 PT (architect's call, this
  session):** the AKS cluster and the ACR were deleted — not stopped — to
  prove the rebuild; the storage account (ClickBench, TPC-H v2 Delta, the
  lake), the three managed identities, the VNet, the Key Vault and a
  PostgreSQL disk snapshot (`kaveon-postgres-pre-aks-delete-20260918`)
  were kept. The Monday rebuild is `infra/bicep/environments/aks-test.bicep`
  → `scripts/aks-test-secrets.py` → the three Helm charts → images from
  git or the GHCR digest → the three catalog registrations → a smoke
  query, and its first run is the test of that path. The stop/start
  procedure in `aks-suspend-resume.md` remains the ordinary weekend rule.

## 2026-09-18

### Stale statistics cost, never answer

- **Decision.** Planning loads a table's statistics record for every join
  side, filtered scan and aggregate beside the source's current version;
  whatever its version the record estimates cardinality and decides the
  build side and a broadcast, and only a record whose identity *and* row
  count equal the pinned source's answers a statement without a scan.
- **Reason.** A stale estimate makes a plan a little worse; a stale answer
  makes it wrong. The two uses have different failure modes and so
  different gates, and one comparison — the record's `identity_sha256`
  against the version the statement is pinned to — separates them
  without a clock, a TTL or a guess.
- **Consequence.** A newer source version seen at planning refreshes the
  record in the background (`KAVEON_STATISTICS_AUTO_REFRESH`) and the old
  record keeps costing until it lands; every statistics-answered
  statement is held to a scan by `context_answers_equal_the_scanned_answers_and_refuse_a_changed_source`
  and `stale_statistics_cost_but_never_answer`; a `context` record carries
  both versions so the equality is visible on the record. Record:
  `788b9e12` (the rule in the commit body), `1860b238` (the costing),
  `72bcf274` (the versioned store); HANDSHAKE Log 2026-09-18, "Table
  statistics as a catalog object".

### Statistics live in the native catalog; no external metastore

- **Decision.** A table's statistics are one object in the SQLite catalog
  beside the definition (`table_statistics`, deleted with the table,
  catalog migration 2); the product catalog's `statistics/<op>.json`
  document is no longer written or read, and no Hive Metastore, Glue,
  Unity or Iceberg REST service holds anything the Engine plans from.
- **Reason.** Two stores for one fact is one store too many: `ANALYZE`
  needed the product store to exist, the planner read a different
  document from the one `SHOW STATS FOR` presented, and neither was
  versioned by the source. The catalog already had transactions,
  revisions, audit and stable ids; what it lacked was the object.
  External metastore adapters are capability contracts in
  `kaveon_core::CatalogAdapter`, not implementations, and a planning path
  that waited on one would have no table to plan today.
- **Consequence.** `ANALYZE` works on any coordinator (`ANALYZE_DISABLED`
  / `STATISTICS_DISABLED` are gone); statistics do not enter the catalog
  snapshot identity, so an `ANALYZE` does not clear the result cache;
  SQLite is a single-coordinator store — a multi-coordinator catalog
  remains the stated target, not a claim; the adapter enum stays
  declarative. Record: `788b9e12` ("one statistics store"), `72bcf274`;
  HANDSHAKE Log 2026-09-18 ("One store, not two") and the standing Native
  catalog contract ("Hive Metastore, AWS Glue, Unity Catalog, and Iceberg
  REST are capability contracts only; adapters are not implemented").

### A spooled exchange input reserves the batch it holds, not the spool

- **Decision.** `DiskExchangeInput` reserves what the rows of the batch it
  last returned occupy (`local_parallel::occupied_bytes`), releases it
  when the next batch replaces it, and keeps a batch whose reservation was
  refused to offer it again on the next call with the reservation tried
  first.
- **Reason.** Reserving the payload's size charged three producers' spools
  at once against a task that never held them (the final stage of a
  100 M-group aggregate was refused for memory it did not use); and a
  batch's Arrow memory size counted the one IPC body once per buffer —
  four times over for the two-column grouped-state batch, so the 195.8 MB
  refused on q33 was a 49 MB batch. A refused batch the IPC reader had
  moved past was lost with the error.
- **Consequence.** Exactly one reservation per batch in flight, on
  whichever thread holds it (`ThreadSource` / `ReservedBatch` hand it to
  the pump); a caller that can make room retries without a row lost or
  read twice; the memory-guard arithmetic on the exchange side is the
  rows', not the buffers'. Record: `1b25e9fc`, `828a48a1` (worktree
  `998fc04`); `engine/crates/server/src/api.rs`, `DiskExchangeInput` doc;
  HANDSHAKE Log 2026-09-18, input-side pressure row.

### Resource groups hold the pool for an under-served head

- **Decision.** Memory admission keeps one FIFO queue per resource group;
  on every release the groups with an eligible head are ranked by
  `admitted_bytes / weight`, lowest first, and when that group's head does
  not fit the pool, the pool is *held* for it — nothing from another group
  is admitted until it fits. A group at its own concurrency or share is
  skipped. No clocks, no virtual time.
- **Reason.** The alternative — admit whatever fits — starves a group
  under its share whenever the groups over theirs keep arriving with
  smaller statements; every lease ends, so the held head fits eventually,
  and the bound on the wait is the queue timeout the group already has.
  Ranking by admitted bytes over weight is a function of the current
  state alone, so the decision is reproducible from the counters.
- **Consequence.** `KAVEON_PRINCIPAL_QUERY_LIMIT` now bounds the `default`
  group as a whole (the per-principal gate is retired); refusals name the
  group and the limit (`RESOURCE_GROUP_REJECTED`); a small statement
  behind a large under-served head waits, which is the price of the
  guarantee and is visible in the group's wait percentiles. Record:
  `8c1fc109` (worktree `3b3a266d`), `15c28c5e`;
  `engine/crates/core/src/memory.rs`, `admit_queued_in` doc;
  [Governance](../engine/governance.md#the-admission-order); HANDSHAKE Log
  2026-09-19, "Platform/operations to 9".

### The audit ledger is JSONL on the coordinator's state directory, not PostgreSQL

- **Decision.** Audit records are newline-delimited JSON in segments under
  `KAVEON_AUDIT_DIR` (default `<state dir>/audit`), rotated by
  `KAVEON_AUDIT_SEGMENT_BYTES` (64 MiB), retained by
  `KAVEON_AUDIT_RETENTION_DAYS` (90), written by one thread with one
  fsync per batch, read by `GET /v1/audit` and exported as JSONL.
- **Reason.** The PostgreSQL retirement is on its own track and the Engine
  must audit without it; a ledger that needs a database to record a
  refused statement cannot record the refusal that the database caused.
  An append-only file on the coordinator's own disk is the store that is
  always there, survives a clean shutdown with nothing lost and a crash
  with at most the batch in flight, and exports without a query engine.
- **Consequence.** Per-coordinator, not global — a multi-coordinator
  ledger is a later merge, not a schema; retention is by age and size,
  not by policy per principal; the catalog store's own `audit_events` are
  drained into the ledger at every publish so `catalog_ddl.rs` is
  untouched. Record: `cc7538af` (worktree `f0032cfd`);
  `engine/crates/server/src/audit.rs` module doc;
  [Governance](../engine/governance.md#the-audit-ledger); HANDSHAKE Log
  2026-09-19.

### Versioned releases only, two rolling prereleases

- **Decision.** The Releases page holds versioned releases cut
  deliberately by tag (`cli-vX.Y.Z` today, `engine-vX.Y.Z` planned) with
  release notes, plus exactly two moving prereleases that `dev` CI updates
  in place — `engine-dev` (the binaries) and `engine-preview` (the Linux
  image digest, Helm chart and deployment bundle). Never a release per
  commit.
- **Reason.** Every green push to `dev` had created an
  `engine-preview-<sha>` prerelease — forty-seven of them beside the one
  versioned CLI release — and a page where the newest entry is a commit is
  a page nobody can pick a version from. Per-commit artifacts stay
  addressable by digest in GHCR and in `preview-artifacts.json`.
- **Consequence.** An operator installs a version or knowingly runs a
  moving prerelease, never something in between; the CLI's checksummed
  archives and `install.ps1` / `install.sh` are the supported install
  paths (winget and Homebrew listings are rendered as assets, submitted
  later with no timeline); Engine version tags, chart pinning and catalog
  migrations are the next platform workstream. Record: `0710451e`;
  `docs/upgrade-version-policy.md`; `4cb8a7d1` (CLI versioned releases);
  HANDSHAKE Log 2026-09-18 (CLI release packaging and the package-manager
  decision).

## 2026-09-19

### `sketches = true` is the ANALYZE spelling; there is no `depth` key

- **Decision.** `ANALYZE t WITH (sketches = true)` is the one spelling of
  the full read; the parser accepts `distinct`, `columns` and `sketches`
  and nothing else. `depth` is a field of the stored object (`metadata` |
  `full`), never a statement option.
- **Reason.** An interim `depth = 'full'` option named the result rather
  than the request — what the reader wants is the sketches, and a
  `depth` that is set by one statement and reported by another invites a
  third value. The architect reversed the interim addition; nothing of it
  remains.
- **Consequence.** The three forms are metadata / `sketches = true` /
  `distinct` or `columns`, they combine, and every page and the CLI's
  hint spell them the same way; a statement with any other key is refused
  naming the three. Record: `38617a5a` (the reversal in the program
  page), `788b9e12` (the parser); `engine/crates/server/src/api.rs`,
  `parse_analyze_body`; HANDSHAKE Log 2026-09-19, "ANALYZE spelling".

### Approximate is opt-in and labelled

- **Decision.** Nothing is estimated unless the statement writes
  `APPROX_COUNT_DISTINCT` / `APPROX_PERCENTILE` or sets `approximate =
  true` (which lowers plain `COUNT(DISTINCT col)` as a sketch under
  COUNT's name); every estimate is on the record as
  `execution.approximate: [{function, argument, sketch, error, error_kind}]`
  on the statistics path and the computed path alike, and Studio shows an
  Approximation row.
- **Reason.** An estimate that is not labelled is a wrong answer with a
  small error; the person who accepts 1.6 % must be the one who asked for
  it, and the record must say what was approximated and by how much so a
  reader of the history can tell an estimate from a count. The sketch
  answers from statistics only when it would be the same sketch the
  computed path builds (the object holds one per column for the whole
  table), so a grouped or filtered statement computes rather than
  answers wrongly.
- **Consequence.** `APPROX_MOST_FREQUENT` is deliberately not offered (no
  heavy-hitter sketch in the object; a third sketch kind is a step of its
  own); the differential sweep compares approximate cases within a stated
  tolerance, zero for HyperLogLog; exact `COUNT(DISTINCT)` and every other
  function are unchanged. Record: `1845a772`, `f7865b4e`, `64c08304`
  (Studio); `docs/reference/engine-sql-compatibility.md`, "Approximate
  aggregates"; HANDSHAKE Log 2026-09-19, "Sketch-answered approximate
  aggregates".

### Benchmarks bypass statistics too

- **Decision.** `settings.use_statistics = false` stands every answer from
  statistics or sketches aside — the `context` path and the `APPROX_*`
  statistics path — while file skipping by the record's bounds and join
  costing still apply; the benchmark scripts send it beside
  `result_cache = false`.
- **Reason.** A `COUNT(*)` answered from a record in 0 ms is the product
  working as designed and a benchmark row that means nothing; the
  published number measures the read path, and pruning is part of the
  read path while answering is not.
- **Consequence.** `scale-suite.py`, `benchmark-throughput.py` and
  `differential-cases.py` send both settings on every statement; a record
  whose `execution.detail` lacks `statistics bypassed` where the
  statistics would have answered, or shows `mode: "context"`, is not a
  benchmark record. Record: `0ba3d691`; `docs/engine/settings.md`,
  `use_statistics`; HANDSHAKE Log 2026-09-19.

### DataFusion: a possible SQL front end later, never the execution layer

- **Decision (architect, 2026-09-18, in conversation; first recorded here).** Apache DataFusion is considered only as a
  future SQL front end (parser and planner), never as the execution or
  storage layer; the columnar executor, the exchange, the memory model and
  the readers stay Kaveon's own.
- **Reason.** The product claim is a purpose-built engine
  with its own parser, planner, optimizer and distributed runtime — "no
  DuckDB, Trino, or Spark embedded inside it" — and a rented execution
  layer would forfeit the memory guard, the spill machinery, the exchange
  protocol and the statistics-aware planner that the qualification
  records are built on.
- **Consequence.** No engine work targets DataFusion before the ship
  date; the question is reopened only if SQL-surface misses (not
  performance) become the recurring adoption blocker, and then for the
  front end alone — `docs/engineering/upper-hand-program.md` (E1) and the
  dialect note in `api/routers/sql.py` are the only prior mentions.
