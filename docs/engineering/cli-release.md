# Cutting a CLI release

Status: **Current** for the tagged GitHub release and the install scripts;
the winget and Homebrew channels need a one-time submission per version by
the maintainer, described below.

A CLI release is a git tag `cli-vX.Y.Z` on the commit to ship. The
[CLI release workflow](../../.github/workflows/cli-release.yml) does the rest:
it checks the tag against the crate version, builds four targets, packages
them with `LICENSE`, writes `SHA256SUMS`, publishes the GitHub release and
attaches the rendered winget manifests and Homebrew formula. The moving
`engine-dev` prerelease from the [Engine workflow](../../.github/workflows/engine.yml)
is unaffected.

## 1. Bump the version

The release version is `[package].version` in
`engine/crates/cli/Cargo.toml`; nothing else declares it. Bump it on `dev`
together with the `Cargo.lock` update, and add the release's entry to
[release notes](../release-notes.md):

```bash
cd engine
sed -i 's/^version = "0.3.0"/version = "0.4.0"/' crates/cli/Cargo.toml
cargo update --workspace          # refreshes Cargo.lock for the new version
cargo build -p kaveon-cli && target/debug/kaveon --version   # kaveon 0.4.0
git commit -am "cli: 0.4.0"
git push origin dev
```

Wait for the Engine workflow on that push to pass (`cargo fmt`, clippy,
tests and the CLI workflow gate on all three preview targets).

## 2. Tag and push

```bash
git tag cli-v0.4.0 <commit>
git push origin cli-v0.4.0
```

The tag must equal the crate version; the workflow fails before building
otherwise. `workflow_dispatch` with a `version` input works too: it creates
the tag at the dispatched commit if it does not exist and refuses to run
when the tag exists elsewhere.

The workflow then:

1. `version` — checks `cli-vX.Y.Z` against `engine/crates/cli/Cargo.toml`.
2. `build` (matrix) — `cargo build --release --locked -p kaveon-cli` for
   `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-apple-darwin`
   and `x86_64-pc-windows-msvc`; runs
   `engine/qualification/cli_workflows.py` against the built binary; checks
   `kaveon --version` reports the release version; packages
   `kaveon-X.Y.Z-<target>.zip` (Windows) or `.tar.gz` (others) with the
   binary and `LICENSE` at the archive root.
3. `release` — writes `SHA256SUMS`, renders the packaging files with
   `scripts/release/render-packaging.py`, creates the GitHub release
   `cli-vX.Y.Z` (marked latest, not a prerelease) and uploads:

| Asset | Purpose |
|---|---|
| `kaveon-X.Y.Z-x86_64-pc-windows-msvc.zip` | Windows x64 |
| `kaveon-X.Y.Z-aarch64-apple-darwin.tar.gz` | macOS Apple Silicon |
| `kaveon-X.Y.Z-x86_64-apple-darwin.tar.gz` | macOS Intel |
| `kaveon-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz` | Linux x64 |
| `SHA256SUMS` | `sha256sum` lines for the four archives |
| `PruthviProdduturi.Kaveon.yaml`, `PruthviProdduturi.Kaveon.installer.yaml`, `PruthviProdduturi.Kaveon.locale.en-US.yaml` | winget manifests for this version |
| `kaveon.rb` | Homebrew formula for this version |

Re-running the workflow for the same tag updates the release in place
(`gh release edit` plus `--clobber` uploads).

Once the release exists, `KAVEON_VERSION=X.Y.Z` with
[the install scripts](../guides/engine-cli.md#install) installs it with
checksum verification; no script change is needed per release.

## 3. Package managers — a later item, no timeline

The release is complete once the assets are up and the install scripts
resolve the version. Two more files are rendered for the day a
package-manager listing is wanted; neither needs a repository of its own to
sit in until then.

**winget.** The three `PruthviProdduturi.Kaveon*.yaml` assets are a
complete manifest for `manifests/p/PruthviProdduturi/Kaveon/X.Y.Z/` in
[winget-pkgs](https://github.com/microsoft/winget-pkgs): download them into a
directory named after the version, `winget validate --manifest .`, then
`wingetcreate submit --token $env:GITHUB_TOKEN .` (or a pull request by
hand). After the first accepted submission, `winget install
PruthviProdduturi.Kaveon` resolves and later versions are one directory
each.

**Homebrew.** `kaveon.rb` is a formula that installs the release tarball for
Apple Silicon, Intel macOS or Linux x64 and verifies its SHA-256. Homebrew
installs formulas from homebrew-core (a pull request there, once the project
meets their notability bar) or from a tap — a repository named
`homebrew-<name>` holding `Formula/kaveon.rb`, which is what projects that
run their own distribution do. Neither is planned; the file is there so the
choice stays open. Until then, macOS and Linux users use `install.sh` or the
tarball.

To re-render either locally (a dry run or a template change):

```bash
gh release download cli-v0.4.0 --pattern SHA256SUMS --output SHA256SUMS
python scripts/release/render-packaging.py --version 0.4.0 --sums SHA256SUMS --out out
```

## Checklist

- [ ] `engine/crates/cli/Cargo.toml` version bumped, `Cargo.lock` updated, release notes entry written, Engine workflow green on `dev`.
- [ ] `cli-vX.Y.Z` pushed; CLI release workflow green; release page lists the four archives, `SHA256SUMS`, three winget manifests and `kaveon.rb`.
- [ ] `KAVEON_VERSION=X.Y.Z` install script run on one Windows and one Unix machine.
- [ ] (Only if a listing is wanted) winget manifests submitted; formula sent to homebrew-core.
