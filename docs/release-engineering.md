# Release engineering

This repository uses a simple, manual release process for
[`graphql-static-analysis`](https://crates.io/crates/graphql-static-analysis).
[GitHub](https://github.com/duckki/graphql-static-analysis-rs) is the canonical source
repository, crates.io distributes releases, and docs.rs builds the published API
documentation. Automated publishing can be added later if the release frequency or
number of maintainers makes it worthwhile.

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

## Publish

The maintainer performing the release must be authenticated with crates.io. The first
publisher creates the crate and becomes its initial owner; subsequent publishers must
already have owner access. From the clean, validated release commit, publish once:

```sh
cargo publish
```

For an agent-assisted release, `cargo publish` and the Git tag push require an
explicit user request. A dry run does not authorize uploading the crate.

After publication succeeds, create and push an annotated tag matching the crate
version:

```sh
git tag -a v0.1.0 -m "graphql-static-analysis 0.1.0"
git push origin v0.1.0
```

Replace `0.1.0` with the released version. Then create a GitHub release from that
tag with a concise summary of user-visible changes.

## Verify the release

Confirm all three public surfaces:

- [crates.io](https://crates.io/crates/graphql-static-analysis) shows the new
  version and repository metadata.
- [docs.rs](https://docs.rs/graphql-static-analysis) successfully builds that
  version.
- `cargo info graphql-static-analysis@0.1.0` resolves the published crate (using the
  released version in place of `0.1.0`).

If a published version is broken, do not move or reuse its tag and do not try to
replace the upload. Yank it with
`cargo yank --version <version> graphql-static-analysis`, prepare a corrected version,
and repeat the process. Yanking prevents new dependency resolution to that version
but does not delete it or break projects whose lockfiles already select it.

See Cargo's official [publishing guide](https://doc.rust-lang.org/cargo/reference/publishing.html)
for crates.io authentication, ownership, and command details.
