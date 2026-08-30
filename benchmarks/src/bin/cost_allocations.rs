use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};

use apollo_compiler::response::serde_json_bytes::json;
use apollo_compiler::validation::Valid;
use apollo_compiler::{ExecutableDocument, Schema};
use graphql_static_analysis::cost::{CostEstimator, CostModel};
use graphql_static_analysis::AnalysisMode;

const ABSTRACT_TYPE_COUNT: usize = 80;

struct CountingAllocator;

static ALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static DEALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let result = unsafe { System.alloc(layout) };
        if !result.is_null() {
            ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        result
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        DEALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
        DEALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(pointer, layout, new_size) };
        if !result.is_null() {
            DEALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
            DEALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        }
        result
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Clone, Copy)]
struct Snapshot {
    allocation_calls: u64,
    allocated_bytes: u64,
    deallocation_calls: u64,
    deallocated_bytes: u64,
}

impl Snapshot {
    fn now() -> Self {
        Self {
            allocation_calls: ALLOCATION_CALLS.load(Ordering::Relaxed),
            allocated_bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
            deallocation_calls: DEALLOCATION_CALLS.load(Ordering::Relaxed),
            deallocated_bytes: DEALLOCATED_BYTES.load(Ordering::Relaxed),
        }
    }

    fn since(self, before: Self) -> Self {
        Self {
            allocation_calls: self.allocation_calls - before.allocation_calls,
            allocated_bytes: self.allocated_bytes - before.allocated_bytes,
            deallocation_calls: self.deallocation_calls - before.deallocation_calls,
            deallocated_bytes: self.deallocated_bytes - before.deallocated_bytes,
        }
    }

    fn net_bytes(self) -> i128 {
        i128::from(self.allocated_bytes) - i128::from(self.deallocated_bytes)
    }
}

fn schema_source(object_count: usize) -> String {
    let mut output = String::from(
        "directive @cost(weight: String!) on ARGUMENT_DEFINITION | ENUM | FIELD_DEFINITION | INPUT_FIELD_DEFINITION | OBJECT | SCALAR\n\n\
         type Query { nodes: [Node] }\ninterface Node { id: ID! }\n",
    );
    for index in 0..ABSTRACT_TYPE_COUNT {
        output.push_str(&format!(
            "interface NodeSubset{index} implements Node {{ id: ID! includedValue: String skippedValue: String }}\n"
        ));
    }
    for index in 0..object_count {
        let interfaces = (0..4)
            .map(|offset| format!("NodeSubset{}", (index + offset) % ABSTRACT_TYPE_COUNT))
            .collect::<Vec<_>>()
            .join(" & ");
        output.push_str(&format!(
            "type NodeType{index} implements Node & {interfaces} {{ id: ID! includedValue: String @cost(weight: \"1\") skippedValue: String @cost(weight: \"7\") }}\n"
        ));
    }
    output
}

fn operation_source(query_spreads: usize) -> String {
    let mut output =
        String::from("query Benchmark($includeBranch: Boolean!, $skipBranch: Boolean!) { nodes {");
    for index in 0..query_spreads {
        if index.is_multiple_of(2) {
            output.push_str(&format!(
                " ... on NodeSubset{index} @include(if: $includeBranch) {{ sharedIncluded: includedValue }}"
            ));
        } else {
            output.push_str(&format!(
                " ... on NodeSubset{index} @skip(if: $skipBranch) {{ sharedSkipped: skippedValue }}"
            ));
        }
    }
    output.push_str(" } }");
    output
}

fn measure_point(object_count: usize, query_spreads: usize, mode: AnalysisMode, name: &str) {
    let schema = Schema::parse_and_validate(schema_source(object_count), "schema.graphql")
        .expect("valid schema");
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        operation_source(query_spreads),
        "query.graphql",
    )
    .expect("valid query")
    .into_inner();
    let schema = schema.into_inner();
    let operation = document.operations.iter().next().expect("one operation");
    let variables = json!({"includeBranch": true, "skipBranch": true})
        .as_object()
        .expect("object variables")
        .clone();

    let before_model = Snapshot::now();
    let model = CostModel::from_schema(&schema).expect("cost model");
    let model_allocations = Snapshot::now().since(before_model);
    let before_estimator = Snapshot::now();
    let estimator = CostEstimator::new(model).mode(mode);
    let estimator_allocations = Snapshot::now().since(before_estimator);
    let before_estimate = Snapshot::now();
    let cost = black_box(
        estimator
            .estimate(&document, operation, Valid::assume_valid_ref(&variables))
            .expect("cost estimate"),
    );
    let estimate_allocations = Snapshot::now().since(before_estimate);
    black_box(cost);

    for (phase, allocations) in [
        ("cost-model", model_allocations),
        ("cost-estimator", estimator_allocations),
        ("estimate", estimate_allocations),
    ] {
        println!(
            "{name},{object_count},{query_spreads},{phase},{},{},{},{},{}",
            allocations.allocation_calls,
            allocations.allocated_bytes,
            allocations.deallocation_calls,
            allocations.deallocated_bytes,
            allocations.net_bytes(),
        );
    }
}

fn main() {
    println!(
        "backend,object_types,query_spreads,phase,allocation_calls,allocated_bytes,deallocation_calls,deallocated_bytes,net_bytes"
    );
    for object_count in (1..=10).map(|scale| scale * 1024) {
        for (mode, name) in [
            (AnalysisMode::ExactCase, "exact-case"),
            (AnalysisMode::Syntactic, "syntactic"),
        ] {
            measure_point(object_count, 8, mode, name);
        }
    }
    for query_spreads in (1..=10).map(|scale| scale * 8) {
        for (mode, name) in [
            (AnalysisMode::ExactCase, "exact-case"),
            (AnalysisMode::Syntactic, "syntactic"),
        ] {
            measure_point(1024, query_spreads, mode, name);
        }
    }
}
