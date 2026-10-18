# Track 2 parameter placeholder style qualification

## Scope

The SQL binder accepts both JDBC-style `?` and PostgreSQL-style `$n`
positional placeholders. A statement must use one positional style consistently;
mixing them is ambiguous because the two syntaxes have different ordinal rules.

## Evidence

- `cargo test -p kaveon-sql mixed_positional_parameter_styles_fail_before_planning --quiet`
- `cargo clippy -p kaveon-sql --all-targets -- -D warnings`
- `cargo fmt --all -- --check`

The regression covers both encounter orders (`?` then `$n` and `$n` then `?`).
Each returns `cannot mix '?' and '$n' parameter placeholders` before planning.
Repeated placeholders within one style remain supported. Named parameters remain
an independent declared API surface and are unaffected by this guard.

## Boundary

This is local binder coverage. Differential execution through external Trino
clients remains part of the broader Parameters acceptance gate.
