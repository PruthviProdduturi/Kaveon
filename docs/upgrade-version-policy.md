# Upgrade and Version Policy

Kaveon is pre-1.0 and does not currently publish a stable compatibility or support
window. The root package and FastAPI report `2.0.0`, while the Rust workspace has its
own crate versions and dev release artifacts. These identifiers do not yet imply
semantic-versioning compatibility across persisted metadata or APIs.

## Current policy

- `dev` is the active integration branch. Engine CI may publish a moving
  `engine-dev` prerelease from successful `dev` builds.
- Database schema creation/migration is performed by application startup and the
  checked-in schema/setup paths; there is no documented downgrade mechanism.
- HTTP routes and stored JSON structures may change before 1.0.
- Release notes must distinguish Studio/API changes from Engine changes and state
  any metadata migration or configuration action.

## Upgrade procedure

1. Read [release notes](release-notes.md) and compare configuration with
   [the configuration reference](reference/configuration.md).
2. Back up the metadata database and retain the currently deployed images/binaries.
3. Test the new revision against a non-production database copy and representative
   registered sources.
4. Run platform and Engine checks appropriate to the changed components.
5. Deploy API before or together with compatible Studio changes; verify health,
   authentication, SQL Lab, one DLM query, and representative dashboards.
6. Treat rollback as an application/image rollback plus metadata restore when a
   migration is not backward compatible.

## Versioning target

A stable release policy should define supported upgrade paths, schema migration
IDs, API deprecation periods, security-support windows, and coordinated component
versions. Until that policy is adopted, pin deployments to a commit SHA or immutable
image digest rather than the moving `dev` branch or `latest` tag.
