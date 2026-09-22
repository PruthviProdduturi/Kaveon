"""Demo mode — the public demo cluster's read-only posture.

`KAVEON_DEMO_MODE=true` makes every mutating platform route answer 403
`demo_read_only` for any principal below Admin, and confines the SQL a
non-admin submits to read statements. Off by default: a self-hosted install
is unaffected. The Engine holds the other half of the posture — the per-
principal live-read quota — in its resource-group configuration, so it binds
whichever client reaches the coordinator.

One dependency, `demo_read_only`, is applied at the application level in
`main.py`, so a route added later is covered without opting in. What it does
by HTTP method and marker:

  GET / HEAD / OPTIONS            never touched — viewing everything stays.
  POST / PUT / PATCH / DELETE     403 `demo_read_only` unless the caller is an
                                  Admin or the handler carries a marker:
    @allowed_in_demo              a per-user preference (favorites, pins, theme,
                                  recents, the user's own chat and query
                                  history, cancelling the user's own
                                  statement) or a request that mutates nothing.
    @statement_route("field")     an execution route: allowed when the SQL in
                                  that JSON body field is one read statement
                                  (SELECT, WITH, SHOW, DESCRIBE, EXPLAIN);
                                  DDL, DML, CALL, ANALYZE, OPTIMIZE and
                                  `SET SESSION` are refused with the same code.

The markers are attributes on the handler function (`scope["endpoint"]`), so
a route cannot be allowed by its path alone, and the allowance travels with
the handler if the route moves.
"""

import re
from typing import Callable, Optional, TypeVar

from fastapi import Depends, HTTPException, Request

from config import settings
from middleware.auth import UserContext, get_user_context

DEMO_READ_ONLY_CODE = "demo_read_only"
DEMO_READ_ONLY_MESSAGE = "This demo is read-only. Sign in as an administrator to make changes."
DEMO_STATEMENT_MESSAGE = (
    "This demo is read-only. Only read statements (SELECT, WITH, SHOW, DESCRIBE, EXPLAIN) "
    "run here; sign in as an administrator to make changes."
)

MUTATING_METHODS = frozenset({"POST", "PUT", "PATCH", "DELETE"})
READ_VERBS = frozenset({"select", "values", "with", "show", "describe", "desc", "explain"})
# Every statement verb the scanner recognises at parenthesis depth 0, so the
# main verb of a `WITH … <statement>` is the first of these after the CTEs.
STATEMENT_VERBS = READ_VERBS | frozenset({
    "insert", "update", "delete", "merge", "upsert", "replace", "create", "drop", "alter",
    "truncate", "rename", "grant", "revoke", "call", "execute", "exec", "analyze", "analyse",
    "optimize", "vacuum", "refresh", "set", "reset", "use", "copy", "load", "import", "export",
    "begin", "start", "commit", "rollback", "deallocate", "prepare", "declare", "lock",
    "comment", "attach", "detach", "install", "pragma", "kill", "deny",
})

_MARKER = "__kaveon_demo__"
_ALLOWED = "allowed"
_STATEMENT = "statement"
_F = TypeVar("_F", bound=Callable)

# Comments, string literals and quoted identifiers are removed before the
# scan: a keyword inside any of them is text, not a verb.
_NOISE_RE = re.compile(
    r"--[^\n]*"                      # line comment
    r"|/\*.*?\*/"                    # block comment
    r"|'(?:[^']|'')*'"               # string literal ('' escapes)
    r"|\"(?:[^\"]|\"\")*\""          # quoted identifier
    r"|`[^`]*`"                      # MySQL-quoted identifier
    r"|\[[^\]]*\]",                  # T-SQL bracketed identifier
    re.DOTALL,
)
_WORD_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def enabled() -> bool:
    return bool(settings.KAVEON_DEMO_MODE)


def allowed_in_demo(handler: _F) -> _F:
    """Mark a mutating handler as allowed for every role in demo mode."""
    setattr(handler, _MARKER, _ALLOWED)
    return handler


def statement_route(field: str) -> Callable[[_F], _F]:
    """Mark an execution handler: allowed when the SQL in the JSON body's
    `field` is a read statement; absent or null SQL is left to the handler."""
    def mark(handler: _F) -> _F:
        setattr(handler, _MARKER, (_STATEMENT, field))
        return handler
    return mark


def _depth_zero_words(sql: str) -> list[str]:
    """The lower-cased words of `sql` outside parentheses, comments, string
    literals and quoted identifiers, in order."""
    text = _NOISE_RE.sub(" ", sql)
    words: list[str] = []
    depth = 0
    index = 0
    while index < len(text):
        char = text[index]
        if char == "(":
            depth += 1
        elif char == ")":
            depth = max(depth - 1, 0)
        elif depth == 0 and (char.isalpha() or char == "_"):
            match = _WORD_RE.match(text, index)
            if match:
                words.append(match.group(0).lower())
                index = match.end()
                continue
        index += 1
    return words


def _single_statement(sql: str) -> Optional[str]:
    """`sql` without a trailing terminator, or None when it stacks statements."""
    text = _NOISE_RE.sub(" ", sql).strip().rstrip(";").strip()
    if ";" in text:
        return None
    return sql


def is_read_statement(sql: str) -> bool:
    """True for one SELECT, VALUES, WITH … SELECT, SHOW, DESCRIBE or EXPLAIN
    over a read statement; false for anything that could write, change a
    session, or stack a second statement."""
    if not isinstance(sql, str) or _single_statement(sql) is None:
        return False
    words = _depth_zero_words(sql)
    if not words:
        return False
    verb = words[0]
    if verb == "explain":
        # EXPLAIN [ANALYZE] [VERBOSE] [(options)] <statement>: the explained
        # statement decides, and EXPLAIN ANALYZE runs it.
        rest = [word for word in words[1:] if word not in {"analyze", "analyse", "verbose"}]
        return bool(rest) and rest[0] in STATEMENT_VERBS and _main_verb_is_read(rest)
    return _main_verb_is_read(words)


def _main_verb_is_read(words: list[str]) -> bool:
    verb = words[0]
    if verb in {"show", "describe", "desc"}:
        return True
    if verb == "with":
        # The CTE list is `name [AS] (…)` pairs at depth 0; the main verb is
        # the first statement verb after them.
        main = next((word for word in words[1:] if word in STATEMENT_VERBS), None)
        if main is None:
            return False
        return main in {"select", "values"} and "into" not in words
    if verb in {"select", "values"}:
        # SELECT … INTO writes a table.
        return "into" not in words
    return False


def assert_read_statement(sql) -> None:
    if not is_read_statement(sql):
        raise HTTPException(
            status_code=403,
            detail={"code": DEMO_READ_ONLY_CODE, "message": DEMO_STATEMENT_MESSAGE},
        )


def _refuse() -> HTTPException:
    return HTTPException(
        status_code=403,
        detail={"code": DEMO_READ_ONLY_CODE, "message": DEMO_READ_ONLY_MESSAGE},
    )


async def demo_read_only(request: Request, ctx: Optional[UserContext] = Depends(get_user_context)) -> None:
    """The application-level dependency. Reads never enter; Admins never
    wait; an unauthenticated mutating request is left to the route's own
    authentication so the answer stays 401."""
    if not enabled() or request.method not in MUTATING_METHODS:
        return
    if ctx is None or ctx.role == "Admin":
        return
    marker = getattr(request.scope.get("endpoint"), _MARKER, None)
    if marker == _ALLOWED:
        return
    if isinstance(marker, tuple) and marker[0] == _STATEMENT:
        try:
            body = await request.json()
        except ValueError:
            return   # not JSON: the handler's own validation answers
        sql = body.get(marker[1]) if isinstance(body, dict) else None
        if sql is None:
            return   # absent: the handler's validation, or a non-SQL request
        assert_read_statement(sql)
        return
    raise _refuse()
