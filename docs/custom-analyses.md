# Adding a custom analysis

The engine separates GraphQL traversal from analysis-specific summarization. It
handles validated operations, fragments, possible runtime object types, Boolean
directives, response-name field collection, and recursive child scopes. A custom
analysis implements the public `Algebra` trait and then runs it through a reusable
`Analyzer`.

## The algebra API

An algebra defines one summary type and four operations:

- `empty` represents an empty selection set.
- `field` summarizes one non-empty collected response-name group after its merged
  child selection set has been summarized.
- `combine` composes response fields that may occur simultaneously.
- `join` bounds alternative Boolean assignments or possible-type cases.

`CollectedFieldGroup` exposes the possible parent object types, inherited and local
Boolean conditions, the shared response name, and the original field occurrences.
Multiple field occurrences in a group represent one collected response field, so an
analysis should not automatically charge once per occurrence.

Summary values may be reused across alternatives. Keep `Summary::clone` inexpensive
when the summary is large—for example, store an expression behind `Rc`.

## Example: count collected response fields

The following algebra counts one unit for each collected response field at every
nested level. Simultaneous fields add, while alternatives take their maximum:

```rust,ignore
use apollo_compiler::response::JsonMap;
use apollo_compiler::{ExecutableDocument, Schema};
use graphql_static_analysis::{
    Algebra, AnalysisMode, Analyzer, CollectedFieldGroup,
};

struct FieldCount;

impl Algebra for FieldCount {
    type Summary = u64;

    fn empty(&self) -> Self::Summary {
        0
    }

    fn field(
        &self,
        _group: &CollectedFieldGroup,
        child_summary: Self::Summary,
    ) -> Self::Summary {
        1_u64.saturating_add(child_summary)
    }

    fn combine(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
        left.saturating_add(right)
    }

    fn join(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
        left.max(right)
    }
}

fn analyze(
    schema: &Schema,
    document: &ExecutableDocument,
    variables: &JsonMap,
) -> Result<(u64, u64), graphql_static_analysis::AnalysisError> {
    let operation = document.operations.iter().next().expect("one operation");
    let analyzer = Analyzer::new(schema);

    let ahead_of_execution = analyzer
        .operation(document, operation)
        .analyze(&FieldCount)?;

    let for_this_request = analyzer
        .operation(document, operation)
        .mode(AnalysisMode::ExactCase)
        .variable_values(variables)
        .analyze(&FieldCount)?;

    Ok((ahead_of_execution, for_this_request))
}
```

`ExactCase` is already the default; the explicit call above shows where to select a
mode. Use `.mode(AnalysisMode::Syntactic)` when a faster, potentially less precise
result is appropriate.

## Analyses that require variables

If `field` needs concrete argument values, keep the coerced `JsonMap` in the algebra
and declare the requirement:

```rust,ignore
fn requires_variables(&self) -> bool {
    true
}
```

Then configure the operation with `.variable_values(&variables)` before calling
`analyze`. The engine returns `AnalysisError::VariablesRequired` when the map is
missing. Operation defaults are applied by the engine for variables absent from a
supplied map.

## Soundness expectations

Rust does not enforce algebraic laws in the type system. They must follow from the
meaning of the custom summary domain.

Let:

- $S$ be the set of summary values;
- $a \preceq b$ mean that $b$ is at least as conservative an approximation as $a$;
- $\mathbf{0}$ denote `empty`;
- $a \otimes b$ denote `combine(a, b)`;
- $a \sqcup b$ denote `join(a, b)`;
- $F_g(a)$ denote `field(g, a)` for a collected field group $g$, where $a$ is the
  summary of the field group's child selection set.

The approximation relation only needs to be a preorder; two different
representations may approximate each other.

1. **Reflexivity.** Every summary approximates itself.

   $$
   \forall a \in S,\quad a \preceq a
   $$

2. **Transitivity.** If $b$ bounds $a$ and $c$ bounds $b$, then $c$ also bounds $a$.

   $$
   \forall a,b,c \in S,\quad
   a \preceq b \land b \preceq c \Longrightarrow a \preceq c
   $$

3. **Associativity of simultaneous composition.** Regrouping fields that can occur
   together must not change the summary.

   $$
   \forall a,b,c \in S,\quad
   (a \otimes b) \otimes c = a \otimes (b \otimes c)
   $$

4. **Commutativity of simultaneous composition.** The order in which simultaneous
   response fields are encountered must not change the summary.

   $$
   \forall a,b \in S,\quad a \otimes b = b \otimes a
   $$

5. **Empty identity.** Combining an empty selection with a summary on either side must
   return that summary.

   $$
   \forall a \in S,\quad
   \mathbf{0} \otimes a = a
   \quad\land\quad
   a \otimes \mathbf{0} = a
   $$

6. **Empty is least.** An empty selection is bounded by every summary.

   $$
   \forall a \in S,\quad \mathbf{0} \preceq a
   $$

7. **Monotonicity of simultaneous composition.** Replacing either input with a more
   conservative approximation must not make the combined result less conservative.

   $$
   \forall a,a',b,b' \in S,\quad
   a \preceq a' \land b \preceq b'
   \Longrightarrow
   a \otimes b \preceq a' \otimes b'
   $$

8. **Join bounds both alternatives.** A joined summary must conservatively approximate
   either feasible alternative.

   $$
   \forall a,b \in S,\quad
   a \preceq a \sqcup b
   \quad\land\quad
   b \preceq a \sqcup b
   $$

ExactCases also preserves type-region alternatives beneath local Boolean decisions.
An algebra used with ExactCases must support that factorization:

9. **Join is below every common upper bound.** If one summary already bounds both
   alternatives, joining them must not exceed it. Together with the previous law,
   this makes `join` a least upper bound, up to the summary preorder.

   $$
   \forall a,b,u \in S,\quad
   a \preceq u \land b \preceq u
   \Longrightarrow
   a \sqcup b \preceq u
   $$

10. **Simultaneous-composition factoring.** Distributing a simultaneous contribution
    into both sides of an alternative may only move upward in the approximation order.

    $$
    \forall a,b,c \in S,\quad
    (a \sqcup b) \otimes c
    \preceq
    (a \otimes c) \sqcup (b \otimes c)
    $$

    Because $\otimes$ is commutative, the corresponding law with $c$ on the left
    follows from this statement.

11. **Field-transfer factoring.** Applying a field transfer after joining alternative
    child summaries may only be more precise than applying the transfer separately and
    then joining the results.

    $$
    \forall g\;\forall a,b \in S,\quad
    F_g(a \sqcup b)
    \preceq
    F_g(a) \sqcup F_g(b)
    $$

These are structural algebra laws. A sound analysis must additionally show that its
`empty`, `combine`, and `field` operations conservatively represent their corresponding
concrete response operations. That relationship is analysis-specific: for example, a
cost analysis relates an abstract numeric bound to the cost of every concrete response
represented by the collected field group.

Test the analysis in both modes and, when supported, both with and without variables.
Tests should include aliases, repeated response names, interfaces or unions,
complementary and independent Boolean variables, and nested selection sets.

## Optional verification in Lean

A custom Rust analysis can be used without Lean. For stronger assurance, model the
same summary domain and transfer operations in the
[GraphQL.lean TreeSummary framework](https://github.com/duckki/GraphQL.lean/tree/main/GraphQL/Theories/TreeSummary):

1. Define a TreeSummary `Algebra` corresponding to the Rust `empty`, `field`,
   `combine`, and `join` operations.
2. Define the concrete response interpretation, an approximation relation, and the
   order on abstract summaries.
3. Prove `Algebra.Lawful` and the backend's local soundness obligations. The generic
   framework then lifts those local results to operation-level execution soundness.
4. For ExactCases optimality, define its concrete `OutcomeSemantics` and prove
   `BestTransferLaws`: `empty`, simultaneous combination, field transfer, and
   alternative join must each preserve a least sound bound. The generic optimality
   theorem then establishes the best approximation over feasible outcomes.
5. Add an executable observation for the Lean analysis and compare it with Rust using
   deterministic cases and differential fuzzing.

The most relevant model files are
[`Core.lean`](https://github.com/duckki/GraphQL.lean/blob/main/GraphQL/Theories/TreeSummary/Core.lean),
[`Soundness.lean`](https://github.com/duckki/GraphQL.lean/blob/main/GraphQL/Theories/TreeSummary/Soundness.lean),
[`ExactCases.lean`](https://github.com/duckki/GraphQL.lean/blob/main/GraphQL/Theories/TreeSummary/ExactCases.lean),
[`ExactCasesOptimality.lean`](https://github.com/duckki/GraphQL.lean/blob/main/GraphQL/Theories/TreeSummary/ExactCasesOptimality.lean),
and
[`Syntactic.lean`](https://github.com/duckki/GraphQL.lean/blob/main/GraphQL/Theories/TreeSummary/Syntactic.lean).

See [TreeSummary differential fuzzing](fuzzing.md) for the Rust/Lean oracle protocol,
observations, retained corpus, and coverage workflow used by this repository.
