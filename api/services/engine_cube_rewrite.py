"""Let the Engine's cube answer a sorted chart breakdown.

The Engine answers a *cube-shaped* grouped aggregate from precomputed cells
instead of scanning the table, but an ``ORDER BY`` or a ``LIMIT`` disqualifies
that match (``cube_answer`` in ``engine/crates/server/src/api.rs``; the
Engine's own coverage test lists ordering among the uncovered shapes), and
every chart statement carries both.  A breakdown returns a handful of rows, so
the statement can be issued without its ordering and its limit and both can be
applied here instead.

This module decides whether one statement may be rewritten that way and
reproduces the Engine's ordering over the rows that come back.  It never talks
to the Engine: the caller owns execution, the row cap and the fallback.

The recognizer is deliberately narrow.  It parses the statement's own token
structure and accepts exactly one shape --

    SELECT <projections> FROM <one table> [WHERE ...] [GROUP BY ...]
                                          [ORDER BY ...] [LIMIT <n>]

-- and refuses everything else, including CTEs, subqueries, set operations,
joins, ``DISTINCT``, ``HAVING``, window functions, ``OFFSET``/``FETCH``,
comments, and any statement whose ``ORDER BY`` key cannot be resolved to one
projected output column.  A statement it is not certain about is left alone.

Two shapes are accepted, for different reasons:

*No ``GROUP BY``, every projection an aggregate call.*  The result is exactly
one row, so the caller's ``ORDER BY`` is a no-op and its ``LIMIT`` (of at
least one) is a no-op.  Dropping both cannot change anything.

*``GROUP BY`` present.*  One row per group, and the count is not known in
advance.  The ordering is reproduced here and the limit applied after it, so
the rows match the ones the Engine's own Sort/TopN would have produced.  The
caller bounds the result: a grouped result wider than its cap is discarded and
the caller's own statement is run unchanged rather than truncated wrongly.

A ``LIMIT`` with no ``ORDER BY`` names an arbitrary subset of the groups --
SQL defines neither which rows nor in what order -- so the rows are kept in
the order the Engine returned them and the first *n* are taken.  No ordering
is invented, because there is no ordering in the statement to preserve.
"""

from dataclasses import dataclass
import math

# Aggregate calls whose presence makes a projection a measure rather than a
# key.  APPROX_COUNT_DISTINCT is absent on purpose and refused below: its
# value may come from the cube's cell sketches in the rewritten statement and
# from a freshly built sketch in the original, and those need not agree.
_AGGREGATES = frozenset({"sum", "count", "min", "max", "avg"})

# Words that put a statement outside the recognised shape.  Matched only
# against unquoted word tokens, so a quoted identifier or a string literal
# spelled the same way never trips them.
_REFUSED_WORDS = frozenset({
    "approx_count_distinct", "cube", "except", "fetch", "for", "grouping",
    "having", "intersect", "into", "join", "lateral", "minus", "offset",
    "over", "pivot", "qualify", "recursive", "rollup", "tablesample", "top",
    "union", "unnest", "unpivot", "using", "values", "window", "with",
})

# Clause keywords in the only order they may appear in.
_CLAUSE_ORDER = ("from", "where", "group by", "order by", "limit")


class _Refused(Exception):
    """The statement is outside the shape this module recognises."""


@dataclass(frozen=True)
class _Token:
    kind: str    # "word" | "quoted" | "string" | "number" | "punct"
    text: str    # as written, except that a quoted identifier is unescaped
    start: int   # offset of the token's first character in the statement


@dataclass(frozen=True)
class _Projection:
    name: str | None       # the output column's name, when it is knowable
    expr: tuple            # the expression's tokens, without any alias
    aggregate: bool        # whether the expression is one aggregate call


@dataclass(frozen=True)
class EngineCubeRewrite:
    """A statement to issue in place of the caller's, and how to finish it."""

    statement: str                      # the caller's statement without ORDER BY/LIMIT
    sort_keys: tuple                     # (projection index, descending) per ORDER BY key
    limit: int | None                    # the caller's LIMIT, applied after ordering
    grouped: bool                        # whether the statement has a GROUP BY


def plan(sql: str) -> EngineCubeRewrite | None:
    """The rewrite for *sql*, or None when it must run exactly as written."""
    try:
        return _plan(sql or "")
    except _Refused:
        return None


def order_rows(rows: list, rewrite: EngineCubeRewrite) -> list | None:
    """The rows the Engine's own Sort/TopN would have produced for *rewrite*.

    None when a sort column holds values that cannot be ordered the way the
    Engine orders them; the caller then runs the original statement.
    """
    ordered = list(rows)
    # Least significant key first, so the stable sorts compose into the
    # multi-key ordering the statement asks for.
    for index, descending in reversed(rewrite.sort_keys):
        keys = _sort_keys([
            row[index] if index < len(row) else None for row in ordered
        ])
        if keys is None:
            return None
        ordered = [
            row for _, row in
            sorted(zip(keys, ordered), key=lambda pair: pair[0], reverse=descending)
        ]
    if rewrite.limit is not None:
        ordered = ordered[:rewrite.limit]
    return ordered


# ── Ordering ──────────────────────────────────────────────────────────────────

def _sort_keys(values: list) -> list | None:
    """Comparable keys for one column, ordered the way the Engine orders it.

    The Engine ranks NULL above every value -- ascending puts nulls last,
    descending puts them first -- and it ignores an explicit ``NULLS
    FIRST``/``NULLS LAST`` clause entirely, so the clause is not reproduced
    either: honouring it here would change the rows a chart shows.

    Numbers are compared as numbers and text as text; a column mixing the two,
    or carrying a NaN, has no ordering to reproduce and returns None.
    """
    present = [value for value in values if value is not None]
    if any(isinstance(value, bool) for value in present):
        if not all(isinstance(value, bool) for value in present):
            return None
        return [(1, 0) if value is None else (0, int(value)) for value in values]
    if all(isinstance(value, (int, float)) for value in present):
        if any(isinstance(value, float) and math.isnan(value) for value in present):
            return None
        return [(1, 0) if value is None else (0, value) for value in values]
    if all(isinstance(value, str) for value in present):
        # Arrow compares UTF-8 bytes, which is the same order as Python's
        # code-point comparison; neither applies a locale collation.
        return [(1, "") if value is None else (0, value) for value in values]
    return None


# ── Lexer ─────────────────────────────────────────────────────────────────────

def _scan(sql: str) -> list:
    """The statement's tokens, with the offset each one starts at.

    Strings, quoted identifiers and numbers are recognised as themselves so
    that a keyword inside a literal cannot be read as structure.  A comment or
    a dollar-quoted literal is refused rather than interpreted.
    """
    tokens: list = []
    index, size = 0, len(sql)
    while index < size:
        char = sql[index]
        if char.isspace():
            index += 1
        elif sql.startswith("--", index) or sql.startswith("/*", index):
            raise _Refused("comment")
        elif char == "'":
            start = index
            index += 1
            while True:
                if index >= size:
                    raise _Refused("unterminated string")
                if sql[index] == "'":
                    if index + 1 < size and sql[index + 1] == "'":
                        index += 2
                        continue
                    index += 1
                    break
                index += 1
            tokens.append(_Token("string", sql[start:index], start))
        elif char in '"`':
            start, quote, value = index, char, []
            index += 1
            while True:
                if index >= size:
                    raise _Refused("unterminated identifier")
                if sql[index] == quote:
                    if index + 1 < size and sql[index + 1] == quote:
                        value.append(quote)
                        index += 2
                        continue
                    index += 1
                    break
                value.append(sql[index])
                index += 1
            tokens.append(_Token("quoted", "".join(value), start))
        elif char.isdigit():
            start = index
            while index < size and (sql[index].isdigit() or sql[index] == "."):
                index += 1
            if index < size and (sql[index] in "eE" or sql[index].isalpha()):
                raise _Refused("numeric literal with a suffix or exponent")
            tokens.append(_Token("number", sql[start:index], start))
        elif char.isalpha() or char == "_":
            start = index
            index += 1
            while index < size and (sql[index].isalnum() or sql[index] in "_$"):
                index += 1
            tokens.append(_Token("word", sql[start:index], start))
        elif char == "$":
            raise _Refused("dollar-quoted literal")
        else:
            tokens.append(_Token("punct", char, index))
            index += 1
    return tokens


def _word(token: _Token) -> str:
    return token.text.casefold() if token.kind == "word" else ""


def _punct(token: _Token, text: str) -> bool:
    return token.kind == "punct" and token.text == text


# ── Recognizer ────────────────────────────────────────────────────────────────

def _plan(sql: str) -> EngineCubeRewrite | None:
    tokens = _scan(sql)
    semicolons = [at for at, token in enumerate(tokens) if _punct(token, ";")]
    if semicolons:
        if len(semicolons) > 1 or semicolons[0] != len(tokens) - 1:
            raise _Refused("more than one statement")
        tokens = tokens[:-1]
    if not tokens or _word(tokens[0]) != "select":
        raise _Refused("not a bare SELECT")
    _refuse_unrecognised(tokens)

    clauses = _clauses(tokens)
    if "from" not in clauses:
        raise _Refused("no FROM clause")
    present = [clauses[name] for name in _CLAUSE_ORDER if name in clauses]
    if present != sorted(present):
        raise _Refused("clauses out of order")
    if "order by" not in clauses and "limit" not in clauses:
        raise _Refused("nothing to remove")

    bounds = _bounds(tokens, clauses)
    _require_single_table(tokens, *bounds["from"])
    for name in ("where", "group by", "order by"):
        if name in bounds and bounds[name][0] >= bounds[name][1]:
            raise _Refused("empty " + name.upper() + " clause")

    projections = [
        _projection(tokens, start, stop)
        for start, stop in _split(tokens, 1, clauses["from"])
    ]
    if not projections:
        raise _Refused("empty select list")

    limit = _limit(tokens, bounds) if "limit" in bounds else None
    grouped = "group by" in clauses

    if grouped:
        if not any(projection.aggregate for projection in projections):
            raise _Refused("grouped statement with no aggregate")
        sort_keys = tuple(
            _sort_key(tokens, start, stop, projections)
            for start, stop in _split(tokens, *bounds["order by"])
        ) if "order by" in bounds else ()
    else:
        if not all(projection.aggregate for projection in projections):
            raise _Refused("no GROUP BY and a projection that is not an aggregate")
        # One row comes back, so there is no ordering and no limiting left to
        # do -- the clauses the statement carries are both no-ops over it.
        sort_keys, limit = (), None

    cut_at = min(clauses[name] for name in ("order by", "limit") if name in clauses)
    statement = sql[:tokens[cut_at].start].rstrip()
    if not statement:
        raise _Refused("nothing left after the removed clauses")
    return EngineCubeRewrite(statement=statement, sort_keys=sort_keys,
                             limit=limit, grouped=grouped)


def _refuse_unrecognised(tokens: list) -> None:
    """Refuse any word that puts the statement outside the recognised shape."""
    selects = 0
    for at, token in enumerate(tokens):
        if token.kind != "word":
            continue
        word = _word(token)
        if word == "select":
            selects += 1
        elif word == "distinct":
            # COUNT(DISTINCT x) is a measure; SELECT DISTINCT changes the rows.
            if not (at and _punct(tokens[at - 1], "(")):
                raise _Refused("DISTINCT outside an aggregate call")
        elif word == "all" and at == 1:
            raise _Refused("SELECT ALL")
        elif word in _REFUSED_WORDS:
            raise _Refused(word)
    if selects != 1:
        raise _Refused("a nested or set-operation SELECT")


def _clauses(tokens: list) -> dict:
    """Where each top-level clause starts, by name."""
    found: dict = {}
    depth = 0
    for at, token in enumerate(tokens):
        if token.kind == "punct":
            if token.text == "(":
                depth += 1
            elif token.text == ")":
                depth -= 1
                if depth < 0:
                    raise _Refused("unbalanced parentheses")
            continue
        if depth or token.kind != "word":
            continue
        word = _word(token)
        if word in ("from", "where", "limit"):
            name = word
        elif word in ("group", "order"):
            if at + 1 >= len(tokens) or _word(tokens[at + 1]) != "by":
                raise _Refused(word.upper() + " without BY")
            name = word + " by"
        else:
            continue
        if name in found:
            raise _Refused("repeated " + name.upper() + " clause")
        found[name] = at
    if depth:
        raise _Refused("unbalanced parentheses")
    return found


def _bounds(tokens: list, clauses: dict) -> dict:
    """The token range each clause's body occupies."""
    starts = sorted(clauses.values())
    bounds = {}
    for name, at in clauses.items():
        body = at + (2 if name.endswith(" by") else 1)
        later = [other for other in starts if other > at]
        bounds[name] = (body, later[0] if later else len(tokens))
    return bounds


def _split(tokens: list, start: int, stop: int) -> list:
    """Comma-separated ranges within [start, stop), split at depth zero."""
    parts, depth, item = [], 0, start
    for at in range(start, stop):
        token = tokens[at]
        if token.kind != "punct":
            continue
        if token.text == "(":
            depth += 1
        elif token.text == ")":
            depth -= 1
        elif token.text == "," and depth == 0:
            if at == item:
                raise _Refused("empty list item")
            parts.append((item, at))
            item = at + 1
    if stop > item:
        parts.append((item, stop))
    elif parts:
        raise _Refused("trailing comma")
    return parts


def _require_single_table(tokens: list, start: int, stop: int) -> None:
    """Accept only one dotted table name, optionally aliased."""
    at = _identifier(tokens, start, stop)
    while at + 1 < stop and _punct(tokens[at], "."):
        at = _identifier(tokens, at + 1, stop)
    if at < stop and _word(tokens[at]) == "as":
        at = _identifier(tokens, at + 1, stop)
    elif at < stop and tokens[at].kind in ("word", "quoted"):
        at += 1
    if at != stop:
        raise _Refused("FROM is not one plain table reference")


def _identifier(tokens: list, at: int, stop: int) -> int:
    if at >= stop or tokens[at].kind not in ("word", "quoted"):
        raise _Refused("expected an identifier")
    return at + 1


def _limit(tokens: list, bounds: dict) -> int:
    start, stop = bounds["limit"]
    if stop - start != 1 or tokens[start].kind != "number" or "." in tokens[start].text:
        raise _Refused("LIMIT is not one plain integer")
    value = int(tokens[start].text)
    if value < 1:
        # LIMIT 0 asks for no rows at all; it is not a bound to re-apply.
        raise _Refused("LIMIT below one")
    return value


def _projection(tokens: list, start: int, stop: int) -> _Projection:
    """One select-list item: its output name, its expression and whether it
    is a single aggregate call."""
    alias_at = None
    depth = 0
    for at in range(start, stop):
        token = tokens[at]
        if token.kind == "punct":
            if token.text == "(":
                depth += 1
            elif token.text == ")":
                depth -= 1
            elif depth == 0 and token.text == "*":
                raise _Refused("* at the top level of a projection")
            continue
        if depth == 0 and _word(token) == "as":
            if at != stop - 2:
                raise _Refused("AS is not followed by exactly one alias")
            alias_at = stop - 1
            stop = at
            break
    if stop <= start:
        raise _Refused("empty projection")
    if alias_at is not None:
        alias = tokens[alias_at]
        if alias.kind not in ("word", "quoted"):
            raise _Refused("alias is not an identifier")
        name = alias.text
    else:
        name = _terminal_name(tokens, start, stop)
    expr = tuple(tokens[start:stop])
    return _Projection(name=name, expr=expr, aggregate=_is_aggregate(expr))


def _terminal_name(tokens: list, start: int, stop: int) -> str | None:
    """The output name of an unaliased projection, when it is knowable.

    A dotted identifier chain is named after its last part.  Anything else --
    an aggregate call, an arithmetic expression -- is named by the Engine in a
    way this module does not predict, so it has no name here and an ORDER BY
    can only reach it by repeating its expression.
    """
    at = start
    last = None
    while at < stop:
        if tokens[at].kind not in ("word", "quoted"):
            return None
        last = tokens[at]
        at += 1
        if at < stop:
            if not _punct(tokens[at], "."):
                return None
            at += 1
            if at >= stop:
                return None
    return None if last is None else last.text


def _is_aggregate(expr: tuple) -> bool:
    """Whether *expr* is exactly one aggregate call over its whole extent."""
    if len(expr) < 3 or _word(expr[0]) not in _AGGREGATES:
        return False
    if not _punct(expr[1], "(") or not _punct(expr[-1], ")"):
        return False
    depth = 0
    for at, token in enumerate(expr[1:], start=1):
        if _punct(token, "("):
            depth += 1
        elif _punct(token, ")"):
            depth -= 1
            if depth == 0:
                return at == len(expr) - 1
    return False


def _sort_key(tokens: list, start: int, stop: int, projections: list):
    """One ORDER BY key as (projection index, descending)."""
    stop, descending = _sort_modifiers(tokens, start, stop)
    span = tokens[start:stop]
    if len(span) == 1 and span[0].kind == "number":
        # The Engine does not read an ordinal key as an output position: it
        # evaluates the literal, so the rows come back in no particular order.
        # There is no ordering to reproduce, and inventing one would change
        # what the chart shows.
        raise _Refused("ordinal ORDER BY key")
    matches: list = []
    if len(span) == 1 and span[0].kind == "quoted":
        matches = [at for at, projection in enumerate(projections)
                   if projection.name == span[0].text]
    elif len(span) == 1 and span[0].kind == "word":
        wanted = _word(span[0])
        matches = [at for at, projection in enumerate(projections)
                   if projection.name is not None
                   and projection.name.casefold() == wanted]
    if not matches:
        matches = [at for at, projection in enumerate(projections)
                   if _same_expression(projection.expr, span)]
    if len(matches) != 1:
        raise _Refused("ORDER BY key is not exactly one projected column")
    return matches[0], descending


def _sort_modifiers(tokens: list, start: int, stop: int):
    """Strip a trailing NULLS FIRST/LAST and ASC/DESC from one ORDER BY key.

    The NULLS clause is parsed only so that it can be removed.  The Engine
    ignores it and always ranks NULL above every value, so reproducing the
    clause would change the result rather than preserve it.
    """
    if stop - start >= 2 and _word(tokens[stop - 2]) == "nulls" \
            and _word(tokens[stop - 1]) in ("first", "last"):
        stop -= 2
    descending = False
    if stop - start >= 1 and _word(tokens[stop - 1]) in ("asc", "desc"):
        descending = _word(tokens[stop - 1]) == "desc"
        stop -= 1
    if stop <= start:
        raise _Refused("empty ORDER BY key")
    return stop, descending


def _same_expression(left: tuple, right: list) -> bool:
    """Whether two token runs are the same expression written the same way.

    Unquoted words match case-insensitively, as SQL reads them; quoted
    identifiers, literals and punctuation must match exactly.
    """
    if len(left) != len(right):
        return False
    for one, other in zip(left, right):
        if one.kind != other.kind:
            return False
        if one.kind == "word":
            if _word(one) != _word(other):
                return False
        elif one.text != other.text:
            return False
    return True
