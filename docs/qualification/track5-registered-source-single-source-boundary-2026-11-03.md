# Registered-source single-source boundary — 2026-11-03

## Result

Registered SQL connectors remain a single-source pushdown boundary. A request
selects one registered source by its KaveonDB source name; the complete
statement is sent to that source's connection pool exactly once. The API does
not plan, split, join, or retry the statement across multiple registered
sources.

This is an intentional fail-closed boundary. A SQL statement containing a
join is still one statement on the selected source; it is not evidence of a
cross-source join. Cross-source planning remains unqualified until Kaveon has
an explicit snapshot, type-coercion, authentication, failure, and result
semantics contract for federation.

## Evidence

- `api/database/test_connectors.py::RegisteredSourceExecutorTests::test_registered_source_execution_never_fans_out_to_multiple_sources`
  asserts one pool invocation and one selected source for a joined statement.
- `api/database/connectors.py` documents the seam as single-source pushdown.
- `api/routers/test_sql_retirement_routing.py` verifies Engine catalogs are
  routed to the Engine path and non-Engine names to the registered-source
  pool; the two authorities are not silently mixed.

## Qualification boundary

This does not qualify cross-source joins. It proves only that the current
connector path cannot accidentally claim or perform them. A future federation
implementation must replace this boundary with independent local and
distributed result tests plus source-version, auth, timeout, retry, and
partial-failure evidence before the compatibility tracker is expanded.
