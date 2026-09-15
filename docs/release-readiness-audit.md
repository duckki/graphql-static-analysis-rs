# Rust/Lean alignment and release-readiness audit (2026-09-14)

The Rust implementation agrees with the updated Lean executable model on the checked
inputs. The source audit found no runtime correction needed. This review updates the
oracle revision, adds a regression for an abstract output with no runtime objects,
and fixes the crate's published-file boundary and associated documentation links.

## Revisions and scope

- Rust: `fd3b4c0` on `optimization`, plus the uncommitted audit changes described here.
- Lean: clean merged `main` at `41f1c4a240c30419c4ba0ffcdfeae612ee4d5810`
  (`Optimize exact cases (#8)`), matching the checkout's `origin/main`.
- Rust `origin/main` was refreshed during the audit and remains
  `ca85a645cf4e08c91f76a7bddd83d65990c0bef4`. The branch contains it and is three
  commits ahead before the audit changes.
- The audit covers the multiplier optimization, both ExactCase schedules, shared
  field/type/variable helpers, the public API changes relative to main, the native
  oracle, and local release checks. It does not publish a package or create a release PR.

The final checks rebuild the oracle from `~/work/apollo-graphql/graphql-lean`, after
the optimization PR merged. Compared with the previously audited PR revision
`a9a1d038fb40cd8558224cc97579b8b1615de774`, all nine changed Lean files differ only in
whitespace, including the definitions, proofs, and tests. The current oracle guard,
README, custom-analysis model links, and architecture reference now pin the merged
commit. Historical performance records retain the revisions of their measured binaries.

## Definition correspondence

Lean now computes `fieldListMultiplier` from its first possible runtime parent,
returning 1 for an empty list or missing field. The previous full scan is retained as
`fieldListMultiplierForAllDefinitions`. Rust instead reads Apollo Compiler's validated
static field definition, then calculates the same `max(1, list multiplier)` expression
as Lean's `fieldListMultiplierForDefinition`.

The new Lean proofs directly explain this representation difference:

| Lean theorem | Correspondence checked in Rust |
| --- | --- |
| `listMultiplier_eq_of_outputTypeSubtype` | Named output covariance and stronger nullability preserve the list multiplier. |
| `fieldListMultiplier_eq_forDefinition` | With a nonempty runtime group and successful covariant runtime lookups, Lean's first-parent calculation equals Rust's static-definition calculation. |
| `fieldListMultiplierForAllDefinitions_eq_fieldListMultiplier` | Under `FieldDefinitionsCompatible`, the shortcut preserves the original full-scan result. |
| `fieldListMultiplier_eq_forDefinition_of_schemaWellFormed` | A valid static-parent lookup and runtime-scope membership discharge the lookup/subtype premises. |

The theorems live in Lean's `Proofs/GraphQL/Theories/TreeSummary/MaxResponseSize.lean`.
The new `FieldDefinitions.lean` proofs carry compatibility through extraction and
grouping; soundness proofs supply it to the algebra obligations. Changes to the
ExactCase/Syntactic soundness structures and the static-parent validity premise in
`SelectionSetExecutionCovered` are proof-contract changes. The executable traversal
schedules did not change in the new Lean revision.

Rust's engine supplies nonempty feasible type scopes to field callbacks:
`ConditionTree::extract` rejects empty scopes, and partitioning retains only nonempty
regions. The public API requires validated schemas/documents. Missing runtime field
definitions are therefore outside the shortcut's intended domain. The new Rust test
checks a valid schema containing an interface with no implementers: nested list
selections beneath that interface contribute no children in all four configurations,
even with a `u64::MAX` list bound.

Existing tests cover covariant named outputs, interface inheritance, nested
nullability, repeated aliases with merged children, zero list bounds, and saturation.
The minimum multiplier remains 1 at a zero bound. Rust's saturating `u64` arithmetic
remains an intentional representation difference from Lean's unbounded `Nat`.

The scheduler audit rechecked these boundaries:

- Without supplied variables, symbolic child/type alternatives remain structural
  joins until parent field transfers finish; compaction occurs at the operation boundary.
- With a supplied map, including an empty map, the forest batches active frontiers,
  preserves selected-child/sibling occurrence order, and joins completed region
  summaries right-associatively. Missing modeled Boolean values resolve to false.
- Speculative symbolic assignments remain separate from immutable request values and
  operation defaults used for extraction pruning.
- Child selection sets from every retained occurrence are merged. Child output types
  and IBM cost still use runtime-specific named definitions; list-shape equivalence
  does not justify substituting the static named output in those paths.

The public exports, callable signatures, and public field types are unchanged by the
branch's implementation refactoring. This is a source-level API review; the automated
`cargo-semver-checks` executable is not installed locally. Release-plz's compatibility
check remains part of the subsequent release PR review.

## Packaging corrections

The former exclusion list did not cover the newly added internal performance and
alignment reports or the handoff document. A package listing initially stopped on
the untracked handoff file, demonstrating that Cargo considered it a package input.
The user subsequently moved the handoff into ignored `.scratch/`.

`Cargo.toml` now explicitly includes library sources, examples, public integration
tests, Cargo metadata/lockfile, README, license, changelog, and the custom-analysis
guide. The verified archive has 28 files, including Cargo's generated
`Cargo.toml.orig`; internal documents, fuzzing content, benchmarks, automation, and
scratch artifacts are absent. Links from packaged documents to excluded internal
guides now point to the repository. The package version remains `0.2.1`;
release-plz owns the eventual version change.

Package validation copied the current source to an isolated directory outside Git,
then ran `cargo package --list --offline` and `cargo package --offline --locked`.
The archive's extracted source compiled successfully. This avoids both a premature
commit and a dirty-package override. The release workflow must still verify the exact
reviewed commit before publishing.

The fuzzing instructions now create their campaign output directories explicitly.
The release checklist uses local package verification and reserves publication for
release-plz, consistent with the repository's agent instructions.

## Validation results

| Check | Result |
| --- | --- |
| Rust 1.95, all targets | 76 library tests and one integration test passed; examples compile. |
| Rust 1.90 MSRV, all targets | Same 77 tests passed. |
| Rustdoc with warnings denied | Passed; doctest run has five intentionally ignored documentation examples. |
| Formatting and strict Clippy | Root and fuzz packages passed. |
| Fuzz package unit tests | Two passed. |
| Maintained benchmark-tool unit tests | Six passed. |
| Locked release benchmark smoke | All 12 response-size and six IBM-cost endpoint configurations passed. |
| Lean full build | Passed, 387 build jobs. |
| Lean lint | Passed, including import closure over 388 tracked Lean files. |
| Standard differential matrix | All 15,840 profiles agree with the rebuilt oracle. |
| Generated differential cases | All 5,000 agree, seed `20260914`. |
| ExactCase schedule audit | All 2,979 comparisons agree. |
| Previously minimized mismatches | Inputs `6162` and `010200000200` pass. |
| Stale-oracle guard | The previous PR revision `a9a1d038...` is rejected with the override unset. |
| Differential fuzz campaign | 26,476 executions in a 30-second-budget campaign; no failure. |
| Rust-only fuzz campaign | 1,918 executions in a separate 30-second-budget campaign; no failure. |
| Mutation sentinel | Baseline agrees; intentional observation corruption is detected. |
| Coverage replay | 4,748 inputs; 91.86% regions, 96.69% functions, 93.88% lines; 90% gate passed. |
| Package contents/build | 28 intended files; extracted package source compiles. |

General proofs in the updated Lean definition/proof diff introduce no `sorry`,
`admit`, `sorryAx`, or new axiom declarations. The full-build logs contain no
proof-hole warnings. Differential testing remains bounded evidence, not a formal
proof of the compiled Rust implementation.

The table records the final merged-main rerun. Its logs, package listing/source-snapshot
location and hashes, and benchmark smoke CSVs are under
`.scratch/merged-lean-release-check/`. Earlier PR-snapshot audit artifacts remain under
`.scratch/release-audit-20260914/`. The native oracle and its revision sidecar remain
under ignored `fuzz/target/`. The retained seed directories are unchanged.

## Performance and remaining release steps

This audit changes no production traversal or multiplier calculation. The
[three-process performance comparison](response-size-list-shape-performance.md)
remains the performance evidence for the algorithm at `fd3b4c0`. New locked release
benchmark endpoints are a result-assertion smoke check; they establish no new timing
baseline or speedup claim.

The audit changes are ready for review and commit after these local checks. CI must
pass on the committed branch before merge. A later explicit release-preparation
request should use the repository's release-plz workflow from main; its generated
version/changelog and compatibility result require review before a separately
authorized release-PR merge. No release workflow was dispatched during this audit.
