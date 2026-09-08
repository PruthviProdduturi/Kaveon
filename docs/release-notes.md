# Release Notes and Changelog Guidance

The repository does not yet maintain a canonical historical changelog. Git history,
`STATUS.md`, and `HANDSHAKE.md` serve different purposes and must not be presented as
release notes: status is current capability state, while the handshake is an
engineering coordination log.

## September 8, 2026 — CLI and Engine UI preview

The Engine coordinator UI update is deployed from `ad3d9c8`; the CLI is distributed
through the [engine-dev preview release](https://github.com/PruthviProdduturi/Kaveon/releases/tag/engine-dev).
These are preview changes to the CLI, Engine UI/authenticated display identity and
documentation. No DLM behavior or subscription policy changed.

- Added familiar remote CLI `SHOW CATALOGS`, `SHOW SCHEMAS`, `SHOW TABLES`, validated
  `USE`, and bare `help`, `clear`, `exit` and `quit`. Dot shortcuts remain available.
- Aligned metadata/query tables, showed the selected schema in the prompt and added
  blank-line separation for human-readable output.
- Added query-summary execution nodes and task completion from recorded telemetry,
  returned rows/JSON result bytes and rates; unavailable scan metrics are explicit.
- Reused Azure login automatically with device sign-in fallback; fixed Windows CLI
  replacement, localhost proxy handling and Microsoft's device verification URL.
- Showed the reported client and verified Entra username in query history. Stable
  tenant/object-ID principal still controls query ownership and authorization.
- Updated CLI, Azure installation/testing, UI access and compatibility documentation.

Upgrade with the [CLI guide](guides/engine-cli.md). Restart port-forwarding after a
coordinator rollout. In-memory query history resets on coordinator restart; the
catalog remains on its PVC. There are no metadata-schema or authentication-config
changes required for this display update. For rollback, use a previously verified
CLI binary or coordinator digest documented in the deployment history.

Validation: 29 CLI tests pass (one subprocess fixture is intentionally ignored as
a standalone test), 96 server tests pass, strict Clippy/formatting pass, and browser
checks cover display identity and existing authentication behavior. Live AKS tests
cover metadata/context commands, exact SQL results, error recovery, exit, execution
summary/spacing, three workers, and rejection of a spoofed display username.
Documentation validation and Studio type checking pass.

Full Trino CLI parity is not claimed. History/editing, completion, `SHOW ... LIKE`,
batch files/multiple statements, advanced formats and complete distributed scan
telemetry remain tracked in the [compatibility checkpoint](engineering/cli-compatibility.md).

## Required release-note structure

For each tagged release or dated deployment, record:

- release identifier, commit SHA, date, and maturity (`alpha`, `preview`, or stable);
- affected components: Studio, API/DLM, Engine, infrastructure, documentation;
- user-visible additions, changes, fixes, and removals;
- breaking API, configuration, authentication, or metadata-schema changes;
- required upgrade and rollback steps;
- known limitations and security fixes without exploit-enabling detail;
- validation performed and links to reproducible benchmark artifacts for any
  performance statement.

## Changelog categories

Use `Added`, `Changed`, `Fixed`, `Deprecated`, `Removed`, and `Security`. Keep roadmap
items out of release notes until executable. Label Engine-only alpha work separately
from the shipping Studio/API path.

## Current release channels

- Platform CI/CD follows `dev`; workflow success does not itself create a versioned
  platform release note.
- Engine CI creates a moving `engine-dev` prerelease after successful `dev` builds.
- There is no declared stable support channel or deprecation window yet.

Future tagged releases should add dated entries to this file (newest first) or adopt
a root `CHANGELOG.md` and link it here. See [upgrade policy](upgrade-version-policy.md)
and [current status](../STATUS.md).
