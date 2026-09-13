//! Indexed runtime types and compatibility regions from TreeSummary.Core.

use apollo_compiler::collections::HashMap;
use apollo_compiler::schema::ExtendedType;
use apollo_compiler::{Name, Schema};
use fixedbitset::FixedBitSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PossibleTypeSet(Arc<PossibleTypeSetData>);

impl Hash for PossibleTypeSet {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.fingerprint.hash(state);
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct PossibleTypeSetData {
    /// Membership is queried for every field occurrence and runtime-object case.
    pub(super) bits: FixedBitSet,
    /// Retains schema order so traversal remains deterministic for arbitrary algebras.
    pub(super) ordered: Vec<usize>,
    /// Cached semantic hash used by canonical condition-tree extraction.
    fingerprint: u64,
}

impl Deref for PossibleTypeSet {
    type Target = PossibleTypeSetData;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PossibleTypeSet {
    pub(super) fn empty(object_count: usize) -> Self {
        Self(Arc::new(PossibleTypeSetData {
            bits: FixedBitSet::with_capacity(object_count),
            ordered: Vec::new(),
            fingerprint: possible_type_fingerprint(&[]),
        }))
    }

    pub(super) fn from_names<'a>(
        names: impl IntoIterator<Item = &'a Name>,
        object_indices: &HashMap<Name, usize>,
        object_count: usize,
    ) -> Self {
        let mut bits = FixedBitSet::with_capacity(object_count);
        let mut ordered = Vec::new();
        for name in names {
            let Some(&index) = object_indices.get(name) else {
                continue;
            };
            if !bits.contains(index) {
                bits.insert(index);
                ordered.push(index);
            }
        }
        let fingerprint = possible_type_fingerprint(&ordered);
        Self(Arc::new(PossibleTypeSetData {
            bits,
            ordered,
            fingerprint,
        }))
    }

    pub(super) fn intersection(&self, other: &Self) -> Self {
        let ordered = self
            .ordered
            .iter()
            .copied()
            .filter(|&index| other.bits.contains(index))
            .collect::<Vec<_>>();
        if ordered.len() == self.ordered.len() {
            return self.clone();
        }
        let mut bits = FixedBitSet::with_capacity(self.bits.len());
        bits.extend(ordered.iter().copied());
        let fingerprint = possible_type_fingerprint(&ordered);
        Self(Arc::new(PossibleTypeSetData {
            bits,
            ordered,
            fingerprint,
        }))
    }

    pub(super) fn is_empty(&self) -> bool {
        self.ordered.is_empty()
    }
}

/// A nonempty equivalence class of runtime object types that activate the same type
/// conditions. The stored indices preserve schema order.
#[derive(Clone, Debug)]
pub(super) struct PossibleTypeRegion {
    pub(super) ordered: Vec<usize>,
}

impl From<&PossibleTypeSet> for PossibleTypeRegion {
    fn from(possible_types: &PossibleTypeSet) -> Self {
        Self {
            ordered: possible_types.ordered.clone(),
        }
    }
}

/// Mirrors Lean's `possibleTypeRegions`: each condition splits the current nonempty
/// regions, so one representative denotes one materializable activation product.
pub(super) fn possible_type_regions<'a>(
    scope: &PossibleTypeRegion,
    conditions: impl IntoIterator<Item = &'a PossibleTypeSet>,
) -> Vec<PossibleTypeRegion> {
    if scope.ordered.is_empty() {
        return Vec::new();
    }

    let mut regions = vec![scope.clone()];
    for allowed in conditions {
        let mut refined = Vec::with_capacity(regions.len().saturating_mul(2));
        for region in regions {
            split_possible_type_region(region, allowed, &mut refined);
        }
        regions = refined;
    }
    regions
}

fn split_possible_type_region(
    mut region: PossibleTypeRegion,
    allowed: &PossibleTypeSet,
    output: &mut Vec<PossibleTypeRegion>,
) {
    let included_count = region
        .ordered
        .iter()
        .filter(|&&index| allowed.bits.contains(index))
        .count();
    if included_count == 0 || included_count == region.ordered.len() {
        output.push(region);
        return;
    }

    let excluded_count = region.ordered.len() - included_count;
    if included_count <= excluded_count {
        let mut included = Vec::with_capacity(included_count);
        region.ordered.retain(|&index| {
            if allowed.bits.contains(index) {
                included.push(index);
                false
            } else {
                true
            }
        });
        output.push(PossibleTypeRegion { ordered: included });
        output.push(region);
    } else {
        let mut excluded = Vec::with_capacity(excluded_count);
        region.ordered.retain(|&index| {
            if allowed.bits.contains(index) {
                true
            } else {
                excluded.push(index);
                false
            }
        });
        output.push(region);
        output.push(PossibleTypeRegion { ordered: excluded });
    }
}

fn possible_type_fingerprint(ordered: &[usize]) -> u64 {
    let mut hasher = DefaultHasher::new();
    ordered.hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone, Debug)]
pub(super) struct PossibleTypesMap {
    object_names: Vec<Name>,
    by_type: HashMap<Name, PossibleTypeSet>,
}

impl PossibleTypesMap {
    pub(super) fn get(&self, type_name: &Name) -> Option<&PossibleTypeSet> {
        self.by_type.get(type_name)
    }

    pub(super) fn object_name(&self, index: usize) -> &Name {
        &self.object_names[index]
    }

    pub(super) fn intersection(
        &self,
        possible_types: &PossibleTypeSet,
        type_name: &Name,
    ) -> PossibleTypeSet {
        self.get(type_name)
            .map(|condition_types| possible_types.intersection(condition_types))
            .unwrap_or_else(|| PossibleTypeSet::empty(self.object_names.len()))
    }

    pub(super) fn names<'a>(&'a self, type_name: &Name) -> impl Iterator<Item = &'a Name> + 'a {
        self.by_type
            .get(type_name)
            .into_iter()
            .flat_map(|possible_types| possible_types.ordered.iter())
            .map(|&index| &self.object_names[index])
    }
}

pub(super) fn build_possible_types(schema: &Schema) -> PossibleTypesMap {
    let implementers = schema.implementers_map();
    let object_names = schema
        .types
        .iter()
        .filter(|(_type_name, definition)| matches!(definition, ExtendedType::Object(_)))
        .map(|(type_name, _definition)| type_name.clone())
        .collect::<Vec<_>>();
    let object_indices = object_names
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), index))
        .collect::<HashMap<_, _>>();
    let by_type = schema
        .types
        .iter()
        .filter_map(|(type_name, definition)| {
            let possible_types = match definition {
                ExtendedType::Object(_) => PossibleTypeSet::from_names(
                    std::iter::once(type_name),
                    &object_indices,
                    object_names.len(),
                ),
                ExtendedType::Union(union) => PossibleTypeSet::from_names(
                    union.members.iter().map(|member| &member.name),
                    &object_indices,
                    object_names.len(),
                ),
                ExtendedType::Interface(_) => PossibleTypeSet::from_names(
                    implementers
                        .get(type_name)
                        .into_iter()
                        .flat_map(|implementers| implementers.objects.iter()),
                    &object_indices,
                    object_names.len(),
                ),
                _ => return None,
            };
            Some((type_name.clone(), possible_types))
        })
        .collect();
    PossibleTypesMap {
        object_names,
        by_type,
    }
}

pub(super) enum TypeRegionPartition {
    Selected,
    Rejected,
    Split {
        selected: PossibleTypeRegion,
        rejected: PossibleTypeRegion,
    },
}

pub(super) fn partition_type_region(
    region: &PossibleTypeRegion,
    allowed: &PossibleTypeSet,
) -> TypeRegionPartition {
    let selected_count = region
        .ordered
        .iter()
        .filter(|&&index| allowed.bits.contains(index))
        .count();
    if selected_count == 0 {
        return TypeRegionPartition::Rejected;
    }
    if selected_count == region.ordered.len() {
        return TypeRegionPartition::Selected;
    }

    let rejected_count = region.ordered.len() - selected_count;
    let mut remainder = region.ordered.clone();
    if selected_count <= rejected_count {
        let mut selected = Vec::with_capacity(selected_count);
        remainder.retain(|&index| {
            if allowed.bits.contains(index) {
                selected.push(index);
                false
            } else {
                true
            }
        });
        TypeRegionPartition::Split {
            selected: PossibleTypeRegion { ordered: selected },
            rejected: PossibleTypeRegion { ordered: remainder },
        }
    } else {
        let mut rejected = Vec::with_capacity(rejected_count);
        remainder.retain(|&index| {
            if allowed.bits.contains(index) {
                true
            } else {
                rejected.push(index);
                false
            }
        });
        TypeRegionPartition::Split {
            selected: PossibleTypeRegion { ordered: remainder },
            rejected: PossibleTypeRegion { ordered: rejected },
        }
    }
}
