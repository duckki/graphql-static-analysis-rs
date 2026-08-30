# Release engineering

This repository uses a simple, maintainer-triggered release process for
[`graphql-static-analysis`](https://crates.io/crates/graphql-static-analysis).
[GitHub](https://github.com/duckki/graphql-static-analysis-rs) is the canonical source
repository, crates.io distributes releases, and docs.rs builds the published API
documentation. Publishing a GitHub release runs
[`publish.yml`](../.github/workflows/publish.yml), which authenticates to crates.io
through OpenID Connect (OIDC) trusted publishing. The repository does not store a
long-lived crates.io token.

Publishing a crate version is effectively permanent: crates.io does not allow an
uploaded version to be replaced. Run every check below from the repository root,
and never use `--allow-dirty` for an actual release.

## Prepare the release

1. Choose the next version according to semantic versioning and update `version` in
   `Cargo.toml`. Review the minimum supported Rust version and dependency
   requirements at the same time.
2. Commit the release changes and push them to `main`.
3. Confirm that the intended release commit is checked out, CI is green, and the
   working tree is clean:

   ```sh
   git switch main
   git pull --ff-only
   git status --short
   ```

For an engine change, also complete the applicable differential-fuzzing checks in
[`fuzzing.md`](fuzzing.md). If the release includes performance claims or material
engine changes, follow [`performance-benchmark.md`](performance-benchmark.md) and
record the comparison before publishing.

## Validate the package

Run the same general checks as CI, plus the declared minimum Rust version:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo +1.90.0 test --all-targets
cargo package
cargo publish --dry-run
```

The `+1.90.0` check must track the `rust-version` declared in `Cargo.toml`.

Inspect the exact file list that Cargo will upload:

```sh
cargo package --list
```

The package must contain the library source, public examples, README, license, and
consumer-facing documentation. It must not contain fuzzing artifacts, benchmarks,
internal release or performance notes, repository automation, secrets, or generated
scratch files.

## One-time trusted-publisher setup

The crate must already exist on crates.io before trusted publishing can be configured.
As a crate owner, follow the
[crates.io trusted-publishing setup](https://crates.io/docs/trusted-publishing): open
the crate's **Settings** page, add a GitHub Actions trusted publisher, and enter these
values exactly:

- Owner: `duckki`
- Repository: `graphql-static-analysis-rs`
- Workflow: `publish.yml`
- Environment: `release`

In the GitHub repository settings, create an environment named `release`. Allow
deployments from `main` and release tags such as `v*`, and add any desired approval
protection. The environment name in GitHub, crates.io, and the workflow must match
exactly.

The publish job has `id-token: write` permission only so it can request a GitHub OIDC
identity token. The official
[`rust-lang/crates-io-auth-action`](https://github.com/rust-lang/crates-io-auth-action)
exchanges that identity for a short-lived crates.io token and revokes the token when
the job finishes. Do not add a `CARGO_REGISTRY_TOKEN` repository secret.

After one successful trusted publication, consider enabling **Require trusted
publishing for all new versions** in the crate's crates.io settings.

Before creating the first release, manually run the **Publish** workflow from `main`.
A manual run performs all release checks and completes the trusted-publishing token
exchange, but its publish step is disabled. A successful run confirms the crates.io
configuration without uploading a version.

## Publish

Create and push an annotated tag from the clean release commit:

```sh
VERSION=X.Y.Z
git tag -a "v${VERSION}" -m "graphql-static-analysis ${VERSION}"
git push origin "v${VERSION}"
```

Replace `X.Y.Z` with the release version. Create and publish a GitHub release from the
existing tag with a concise summary of user-visible changes. Publishing the GitHub
release triggers the workflow, which verifies that the tag matches `Cargo.toml`, that
the tagged commit is on `main`, and that all release checks pass before it uploads the
crate.

For an agent-assisted release, pushing the tag and publishing the GitHub release
require an explicit user request. Do not run `cargo publish` locally for an actual
release.

If the workflow fails before upload, fix the cause and rerun the same workflow. Do not
move or replace the release tag. If Cargo reports a timeout or another ambiguous result,
check crates.io before rerunning because the upload may have succeeded.

## Verify the release

Confirm all three public surfaces:

- [crates.io](https://crates.io/crates/graphql-static-analysis) shows the new
  version and repository metadata.
- [docs.rs](https://docs.rs/graphql-static-analysis) successfully builds that
  version.
- `cargo info graphql-static-analysis@X.Y.Z` resolves the published crate (using the
  release version in place of `X.Y.Z`).

If a published version is broken, do not move or reuse its tag and do not try to
replace the upload. Yank it with
`cargo yank --version <version> graphql-static-analysis`, prepare a corrected version,
and repeat the process. Yanking prevents new dependency resolution to that version
but does not delete it or break projects whose lockfiles already select it.

See Cargo's official [publishing guide](https://doc.rust-lang.org/cargo/reference/publishing.html)
for crates.io authentication, ownership, and command details.
