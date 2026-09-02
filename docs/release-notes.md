# Release Notes and Changelog Guidance

The repository does not yet maintain a canonical historical changelog. Git history,
`STATUS.md`, and `HANDSHAKE.md` serve different purposes and must not be presented as
release notes: status is current capability state, while the handshake is an
engineering coordination log.

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
