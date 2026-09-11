# Cached Engine builds in ACR

Use the explicit dependency image when iterating on Engine source in an ACR
environment. A cold `engine/Dockerfile` build cooks the complete Rust dependency
graph on every agent. This flow publishes that cooked graph once and addresses
it by immutable digest for subsequent source builds.

The dependency tag is derived from `Cargo.lock`, every `Cargo.toml`, build
scripts, Cargo configuration, Rust target paths, the dependency Dockerfile and
the immutable cargo-chef base. The dependency image also stores the SHA-256 of
cargo-chef's exact `recipe.json`. Every source build regenerates that recipe and
fails before compilation if it differs. A cache tag therefore cannot silently
serve a different dependency recipe. Both ACR builds explicitly target
`linux/amd64`, matching the current AKS node pools, and both Cargo commands use
`--locked`.

Both base inputs must be digest references. Do not pass mutable tags such as
`latest-rust-1.88` or `bookworm-slim`. Mirror each base into the controlled ACR
once, record its resolved digest, and retain those values with the build
evidence. For example:

```powershell
$subscription = "<subscription-id>"
$registry = "<acr-name>"

az acr import --subscription $subscription --name $registry `
  --source "docker.io/lukemathwalker/cargo-chef:latest-rust-1.88" `
  --image "build-base/cargo-chef:rust-1.88"
$chefDigest = az acr repository show --name $registry `
  --image "build-base/cargo-chef:rust-1.88" --query digest -o tsv

az acr import --subscription $subscription --name $registry `
  --source "docker.io/library/debian:bookworm-slim" `
  --image "build-base/debian:bookworm-slim"
$runtimeDigest = az acr repository show --name $registry `
  --image "build-base/debian:bookworm-slim" --query digest -o tsv

$loginServer = az acr show --subscription $subscription --name $registry `
  --query loginServer -o tsv
$chefImage = "$loginServer/build-base/cargo-chef@$chefDigest"
$runtimeImage = "$loginServer/build-base/debian@$runtimeDigest"
```

Check that both resolved values match `sha256:` followed by 64 lowercase hex
characters before using them. Later builds should reuse those exact references;
do not repeat the imports merely to refresh a mutable upstream tag.

Build an Engine image with a unique tag:

```powershell
./scripts/build-engine-acr-cache.ps1 `
  -Subscription $subscription `
  -Registry $registry `
  -ImageTag "engine-$(git rev-parse --short=12 HEAD)" `
  -ChefImage $chefImage `
  -RuntimeImage $runtimeImage
```

The script requires a clean `engine` tree so the recorded source revision fully
identifies the build context. Use `-PlanOnly` to validate inputs and print the
derived cache tag without contacting ACR.

The first invocation for a recipe builds
`kaveon-engine-dependencies:recipe-<sha256>`. Later source-only changes resolve
that tag, pin its manifest digest in `DEPENDENCY_IMAGE`, verify the exact recipe,
and compile only the workspace crates. `-ForceDependencyRebuild` replaces the
cache tag when investigating a damaged cache; the final build still consumes
the replacement by its resolved digest.

The script prints a JSON object containing the final runtime image digest, the
dependency image digest, recipe-input identity and source commit. Deploy only
the value in `image`, which uses `repository@sha256:digest`. Keep the JSON with
the deployment evidence. A source build failure containing the recipe comparison
means the cache inputs changed in a way the local key did not predict; rerun with
`-ForceDependencyRebuild` and retain that event in the evidence.

This flow does not deploy the image or update an AKS workload. It only creates
ACR image manifests. The existing `engine/Dockerfile` remains the portable cold
build and CI fallback.
