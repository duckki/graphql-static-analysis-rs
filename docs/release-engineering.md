# Release engineering

## Executive summary

To publish a release:

1. Make sure every intended change is on `main` and its required CI, fuzzing, and
   performance checks are complete.
2. Run **Actions → Prepare release → Run workflow** with `main` selected, or ask an
   agent to prepare a release. From the command line, run:

   ```sh
   gh workflow run prepare-release.yml --ref main
   ```

3. Review the generated `Release graphql-static-analysis X.Y.Z` PR. Confirm the
   proposed version and changelog, and wait for its CI to pass.
4. When the PR is ready, merge it using **Create a merge commit**. The merge publishes
   the crate, creates the `vX.Y.Z` tag, and creates the GitHub release automatically.
5. Verify the version on
   [crates.io](https://crates.io/crates/graphql-static-analysis),
   [docs.rs](https://docs.rs/graphql-static-analysis), and with
   `cargo info graphql-static-analysis@X.Y.Z`.

Do not edit the package version, run `cargo publish`, push a release tag, or create the
GitHub release manually. To cancel an unmerged release, close its release PR; no crate
has been published at that point. For agent-assisted releases, preparing the PR and
merging it require two separate explicit requests.

> [!IMPORTANT]
> If **Publish** fails after the release PR is merged, do not run **Prepare release**
> again. First rerun the failed workflow. If that cannot complete the release, run
> **Actions → Publish → Run workflow** from `main` and enter the merged release PR
> number. The recovery run verifies the reviewed commit and completes only the missing
> crates.io upload, tag, or GitHub release; conflicting state causes it to stop safely.

## Design and automation

This repository releases
[`graphql-static-analysis`](https://crates.io/crates/graphql-static-analysis) through
a release pull request managed by
[`release-plz`](https://release-plz.dev/). GitHub is the canonical source repository,
crates.io distributes releases, and docs.rs builds the published API documentation.

Releasing has two explicit maintainer actions: request a release PR, then review and
merge it. The [`prepare-release.yml`](../.github/workflows/prepare-release.yml)
workflow creates the proposal only when requested. Merging that proposal invokes
[`publish.yml`](../.github/workflows/publish.yml), which validates the merge, uploads
the crate through OpenID Connect (OIDC) trusted publishing, creates the matching
`vX.Y.Z` tag, and publishes the GitHub release. The repository stores neither a
long-lived crates.io token nor a separate release-version value.

Publishing a crate version is effectively permanent: crates.io does not allow an
uploaded version to be replaced. Never use `--allow-dirty` for an actual release.

## Release lifecycle

1. A maintainer explicitly runs **Prepare release** from `main`. Release-plz compares
   the repository with the latest crates.io package, derives the next version from
   conventional commit messages and API compatibility checks, and opens a release PR
   containing the `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md` updates. It explicitly
   dispatches CI for the bot-created branch because GitHub does not recursively trigger
   workflows from ordinary `GITHUB_TOKEN` events.
2. A maintainer reviews the proposed version and changelog, waits for CI, and merges
   the release PR.
3. The Publish workflow recognizes that the `main` push came from a merged
   `release-plz-*` PR, validates the repository on stable Rust and the declared minimum
   Rust version, and runs `release-plz release`. The additional
   `release_always = false` guard in [`release-plz.toml`](../release-plz.toml) prevents
   publication from any other commit.
4. The workflow reconciles the published crate with GitHub. If crates.io accepted the
   upload but release-plz could not finish the tag or GitHub release, the reconciler
   reads the exact source commit from the crate's `.cargo_vcs_info.json` and safely
   creates the missing metadata. It refuses to move a conflicting tag.

Ordinary pushes to `main` neither create a release PR nor publish a crate.
Configuration changes that affect release-plz behavior belong in `release-plz.toml`.
The Publish workflow's manual entry point is reserved for recovering a previously
merged release PR; it cannot select an arbitrary commit or package version.

## Prepare a release

Merge the intended development changes into `main`, then run **Actions → Prepare
release → Run workflow** with `main` selected. From the GitHub CLI, the equivalent is:

```sh
gh workflow run prepare-release.yml --ref main
```

For an agent-assisted release, an explicit request to prepare or publish a release
authorizes this workflow dispatch. Creating the release PR completes that request; the
agent must stop for review and obtain a separate explicit request before merging it.

Release-plz opens a PR named `Release graphql-static-analysis X.Y.Z`. Re-running
**Prepare release** updates an existing release PR when possible. Closing an unwanted
proposal cancels it; nothing recreates the PR until another explicit request. Do not
manually update the package version, create the release tag, publish a GitHub release,
or run `cargo publish`.

Use conventional commit prefixes when they clarify the intended semantic-version
change. Release-plz also runs `cargo-semver-checks`; its result is shown in the release
PR. Before `1.0.0`, Cargo-compatible semantic versioning normally uses a patch bump for
compatible changes and a minor bump for breaking changes.

For an engine change, complete the applicable differential-fuzzing checks in
[`fuzzing.md`](fuzzing.md). If the release includes performance claims or material
engine changes, follow [`performance-benchmark.md`](performance-benchmark.md) and
record the comparison before merging the release PR.

## Review the release PR

Before merging, confirm that:

- the proposed version matches the compatibility impact;
- the changelog accurately describes the user-visible changes;
- `Cargo.toml` and `Cargo.lock` agree on the package version;
- CI passed for the release PR head;
- `cargo package --list` contains only intended published files; and
- the tagged source will contain every change intended for the release.

For extra local verification, run:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo +1.90.0 test --all-targets
cargo package
cargo publish --dry-run
cargo package --list
```

The `+1.90.0` check must track the `rust-version` declared in `Cargo.toml`. The package
must contain the library source, public examples, README, license, and consumer-facing
documentation. It must not contain fuzzing artifacts, benchmarks, internal release or
performance notes, repository automation, secrets, or generated scratch files.

Merge the release PR with GitHub's **Create a merge commit** strategy. Release-plz can
handle other strategies, but a merge commit lets it identify the exact reviewed PR tip
even if nearby changes reach `main` concurrently. The merge starts publication; no
manual tag push or GitHub Release creation follows it.

Merging the release PR requires a distinct explicit request, because the merge starts
the irreversible crates.io publication.

## Failure recovery

The workflow is designed to be rerun safely. Start by rerunning the failed **Publish**
workflow. If the original run cannot be reused—for example, because the workflow itself
needed a fix—run **Actions → Publish → Run workflow**, select `main`, and enter the
number of the merged release PR. The command-line equivalent is:

```sh
gh workflow run publish.yml --ref main -f release_pr=123
```

Replace `123` with the merged release PR number. Recovery validates that the PR was
merged into `main` from a `release-plz-*` branch using **Create a merge commit**, checks
out that exact merge, and verifies that the reviewed PR head is one of its parents. It
then reconciles these durable states:

- If crates.io does not contain the version, release-plz publishes the reviewed source,
  creates its immutable tag, and creates the GitHub release.
- If crates.io already contains the version with the expected source commit, the
  workflow creates only a missing tag or GitHub release.
- If the upload, tag, and GitHub release are already complete, recovery succeeds as a
  no-op.
- If crates.io records another source commit, a tag points elsewhere, or the PR no
  longer belongs to `main`, recovery fails without moving or replacing anything.

The release PR number is the recovery authority: it identifies both the reviewed
source and the intended package version. Do not use **Prepare release** to recover a
merged PR; its job is to propose version and changelog changes before the merge.

Additional recovery rules:

- If the upload result is ambiguous, check crates.io before retrying. Release-plz
  treats an already-published package version as complete and will not upload it again.
- If crates.io has the version but its tag or GitHub release is missing, the same run's
  reconciliation step completes it. Recovery verifies the published commit and fills
  in anything still missing. Never move or replace a release tag.
- If the failure requires changes to the crate itself, do not silently add them to the
  already-reviewed release. Prepare and publish a subsequent corrected version.

If a published version is broken, do not try to replace it. Yank it with
`cargo yank --version X.Y.Z graphql-static-analysis`, prepare a corrected release, and
merge the next release PR. Yanking prevents new dependency resolution to that version
but does not delete it or break projects whose lockfiles already select it.

## Security model

Release security is split between controls committed to this repository and controls
maintained in GitHub and crates.io settings. Neither side is sufficient by itself.

### Controls in the repository

- [`.github/CODEOWNERS`](../.github/CODEOWNERS) assigns every path to `@duckki`.
  CODEOWNERS identifies the required human reviewer; the `main` ruleset described below
  must require code-owner review for this assignment to block merges.
- Workflows default to `contents: read`. Only the Prepare release job receives
  `actions: write`, `contents: write`, and `pull-requests: write`, and only the Publish
  job receives `contents: write` and `id-token: write`.
- Prepare release is manually dispatched and refuses to operate anywhere except the
  current tip of `main`. The write-capable job does not approve or merge pull requests.
- Third-party actions are pinned to full commit identifiers, and the release-plz CLI
  version is explicit. Review dependency updates before changing either value.
- Automatic publishing accepts only the merge commit of a merged `release-plz-*` PR.
  Manual recovery accepts only an explicitly named merged release PR, verifies that
  its reviewed head is a parent of the merge, and checks that the merge remains on
  `main`.
- Reconciliation downloads the immutable crates.io archive and requires its recorded
  Git source commit to match the reviewed release PR exactly. It creates missing
  metadata but refuses to move a conflicting tag or replace an uploaded crate.
- Publishing uses crates.io OIDC trusted publishing. The repository stores no
  long-lived registry token, and overlapping publish or recovery runs are serialized.

### GitHub settings checklist

Maintain these settings for the repository:

- **Settings → Rules → Rulesets → branch ruleset for `main`**
  - Set enforcement to **Active** and target only the `main` branch.
  - Require a pull request before merging and require at least one approval.
  - Require review from Code Owners.
  - Dismiss stale approvals when new commits are pushed.
  - Require approval of the most recent reviewable push.
  - Require the CI checks `test` and `msrv` to pass.
  - Permit the **merge commit** method used by the release workflow; do not require
    linear history.
  - Block force pushes and branch deletion.
  - Do not grant GitHub Actions or `github-actions[bot]` a bypass. A solo maintainer may
    give the repository administrator a **pull requests only** bypass so their own PRs
    remain workable without permitting direct pushes to `main`.
- **Settings → Rules → Rulesets → tag ruleset for `v*`**
  - Allow creation so the Publish workflow can create a new release tag.
  - Restrict updates and deletions so an existing release tag cannot be moved or
    removed.
  - Do not grant GitHub Actions a bypass for update or deletion restrictions.
- **Settings → Environments → `release`**
  - Allow deployments from the `main` branch only. The current workflow does not deploy
    from release tags, so remove obsolete `v*` tag policies.
  - Keep environment administrators and protection changes limited to maintainers.
  - An environment required reviewer is optional defense in depth, but it adds a third
    manual approval after merging the release PR.
- **Settings → Actions → General → Actions permissions**
  - Keep the action allow-list, if enabled, limited to the pinned actions used by the
    workflows. Do not allow untrusted actions or mutable action references.
- **Settings → Actions → General → Workflow permissions**
  - Select **Read repository contents and packages permissions** as the default.
  - Enable **Allow GitHub Actions to create and approve pull requests** only after the
    `main` ruleset below is active. GitHub bundles creation and approval; the ruleset
    makes an Actions approval insufficient by requiring review from the human code
    owner.
- **Repository secrets and variables**
  - Do not add `CARGO_REGISTRY_TOKEN`; publishing must continue to use OIDC.
  - Review new secrets before exposing them to any workflow with third-party steps.

Revisit this checklist whenever the release workflow, branch strategy, repository
ownership, trusted publisher, or required CI jobs change. In particular, renaming a CI
job requires updating the required status checks, and changing the release environment
or workflow filename requires updating the crates.io trusted-publisher identity.

## One-time repository setup

The crates.io trusted publisher for this crate must use these values:

- Owner: `duckki`
- Repository: `graphql-static-analysis-rs`
- Workflow: `publish.yml`
- Environment: `release`

The GitHub repository must have an environment named `release` that permits `main`.
Complete and maintain the GitHub settings checklist above before enabling Actions to
create pull requests. Release-plz performs the trusted-publishing exchange itself; do
not add `CARGO_REGISTRY_TOKEN` as a repository secret.

After one successful trusted publication, consider enabling **Require trusted
publishing for all new versions** in the crate's crates.io settings.

## Verify a release

Confirm all three public surfaces:

- [crates.io](https://crates.io/crates/graphql-static-analysis) shows the new version
  and repository metadata.
- [docs.rs](https://docs.rs/graphql-static-analysis) successfully builds that version.
- `cargo info graphql-static-analysis@X.Y.Z` resolves the published crate.

See release-plz's [GitHub Actions guide](https://release-plz.dev/docs/github/quickstart),
the crates.io [trusted-publishing guide](https://crates.io/docs/trusted-publishing), and
Cargo's official [publishing guide](https://doc.rust-lang.org/cargo/reference/publishing.html)
for upstream details.
