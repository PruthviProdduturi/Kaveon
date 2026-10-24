# Track 2 qualification: refusing a SELECT alias used as a column

Date: 2026-10-23

Commit: the change refusing SELECT-alias references on `dev`.

## What was wrong

Reusing a `SELECT` alias — later in the same list, or in `WHERE` — is not
supported, and nothing rejected it. The planner resolves identifiers without a
schema, so the reference survived planning and reached storage, where it
surfaced as:

```text
SELECT country, actions * 2 AS doubled FROM events_dictionary WHERE doubled > 10
  -> storage: projection references unknown column 'doubled'
```

Someone who wrote `WHERE doubled > 10` was told their Parquet file lacks a
column they never asked for. The alias is in the query beside it, and the
thing actually unsupported is referring to it.

This was found by adding the case to the differential corpus rather than by
reading the planner, which had reported both forms as planning cleanly.

## Contract

Both forms now fail closed at planning, naming the alias and a rewrite:

| Form | Refusal |
|---|---|
| `SELECT x*2 AS d FROM t WHERE d > 10` | `WHERE cannot refer to the SELECT alias 'd'; a filter is applied before the SELECT list is computed. Repeat the expression in WHERE, or wrap the query and filter outside it` |
| `SELECT x*2 AS d, d+1 AS e FROM t` | `the SELECT list cannot refer to its own alias 'd'; every item is computed from the input row. Repeat the expression, or compute it in a subquery and select from that` |

A reference is found wherever it appears in the predicate: inside a function,
a `CASE`, `BETWEEN`, an `IN` list, a negation, a cast, or one side of a
conjunction.

**Only bare identifiers count.** `t.doubled` names a column of `t`, and an
alias is not a member of any relation, so a qualified name is never treated as
an alias reference. That matters where a table has a column named like
someone's alias: `SELECT t.doubled * 2 AS doubled FROM t WHERE t.doubled > 10`
keeps working.

`ORDER BY` is unaffected and may still refer to a select alias, which is
standard and was already supported.

## Why refuse rather than implement

Trino does not support alias reuse either, so this is not a parity gap. The
comparison worth naming is BigQuery, which has just shipped it behind
`reusable_column_aliases_mode` and resolves a collision with a real column
**silently in the column's favour** — a query that quietly returns something
other than what it reads as.

Kaveon cannot currently do better than that, and should not do worse.
Detecting the collision requires knowing the relation's columns, and the
planner has no schema: it cannot tell an alias reference from a real column of
the same name. Resolving one way or the other without that knowledge would be
guessing, and guessing silently is the behaviour being avoided.

So the boundary is stated instead, in the terms the tracker already sets for
this engine — unsupported syntax fails closed with actionable SQL error names.
Implementing the resolution belongs where the columns are known, and when it
is done, the collision should be refused rather than resolved.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-sql alias
```

Observed result:

```text
test logical_plan::tests::the_select_list_refuses_its_own_alias_and_says_why ... ok
test logical_plan::tests::a_later_select_item_may_reuse_a_name_that_is_not_an_alias ... ok
test logical_plan::tests::where_refuses_a_select_alias_and_says_why ... ok
test logical_plan::tests::where_finds_a_select_alias_however_it_is_nested ... ok
test logical_plan::tests::where_still_accepts_everything_that_is_not_an_alias_reference ... ok

test result: ok. 12 passed; 0 failed
```

Two of those five exist to stop the refusal growing past what is unsupported.
A rule like this has two ways to be wrong, and over-rejection is the one that
breaks working queries: `where_still_accepts_everything_that_is_not_an_alias_reference`
and `a_later_select_item_may_reuse_a_name_that_is_not_an_alias` pin the cases
that must keep planning, including the qualified-column collision.

The refusals assert on their text, not only on failing. An error that fails
for the right reason but says the wrong thing leaves the person no better off
than the storage message it replaced.

## Not qualified

- **No execution or distributed evidence, and none is needed.** The refusal is
  at planning, so no plan reaches execution to compare. The differential cases
  that found the fault were removed once it was refused: they described
  unsupported behaviour and would now fail at planning by design.
- **The resolution itself is not implemented.** Alias reuse remains
  unsupported; this change is about how that is reported.
- **No external pinned-Trino comparison.** Trino rejects these forms too, but
  the refusal text is Kaveon's own and is not compared against Trino's.
