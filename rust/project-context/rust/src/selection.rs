//! Revision-aware, diagnostic *candidate* test selection. No scheduler admission.

use crate::{CargoGraph, Coverage, Error, MAX_CHANGES, MAX_EDGES, check_member_path};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Canonical new path (for deletion this may be the removed path).
    pub path: String,
    /// Prior path when moved/renamed; old graph supplies its former membership.
    pub previous_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImpactReasonKind {
    CurrentMembership,
    CurrentDependency,
    PredecessorMembership,
    PredecessorDependency,
    UnresolvedConservativeFallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImpactReason {
    pub kind: ImpactReasonKind,
    pub path: String,
    /// Workspace package IDs, from the original owner to the selected package.
    pub chain: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImpactTarget {
    pub package_id: String,
    pub package_name: String,
    pub reasons: Vec<ImpactReason>,
    pub omitted_reasons: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Impact {
    pub current_context: String,
    pub selected: Vec<ImpactTarget>,
    pub deferred_package_ids: Vec<String>,
    pub unresolved: Vec<String>,
    pub conservative_fallback: bool,
    /// An unresolved frontier means selection MUST NOT be represented as exact.
    pub exact: bool,
}

/// Select package-level test candidates. Cargo metadata knows declared target
/// entry points, NOT the full `mod` tree or macro-generated test membership.
/// Unknown paths and missing predecessor state conservatively touch every
/// current workspace package. The owner still decides whether any test may run.
pub fn select_tests(
    previous: Option<&CargoGraph>,
    current: &CargoGraph,
    changes: &[Change],
    max_targets: usize,
) -> Result<Impact, Error> {
    if changes.len() > MAX_CHANGES {
        return Err(Error::Limit("source changes per selection"));
    }
    if max_targets == 0 || max_targets > crate::MAX_PACKAGES {
        return Err(Error::Invalid("selection target budget"));
    }
    let root = &current.binding.workspace_root;
    let mut unresolved = Vec::<String>::new();
    let mut candidates = BTreeMap::<String, (Vec<ImpactReason>, usize)>::new();
    let mut fallback = current.coverage != Coverage::EvaluatedCargoGraph;
    if fallback {
        push_unique(&mut unresolved, "current_cargo_graph_partial")?;
    }
    let comparable = previous.is_some_and(|old| old.identity_matches(current));
    if previous.is_some_and(|old| old.coverage != Coverage::EvaluatedCargoGraph) {
        fallback = true;
        push_unique(&mut unresolved, "predecessor_cargo_graph_partial")?;
    }
    if previous.is_some() && !comparable {
        fallback = true;
        push_unique(&mut unresolved, "predecessor_variant_mismatch")?;
    }
    if previous.is_none() && !changes.is_empty() {
        fallback = true;
        push_unique(&mut unresolved, "predecessor_not_available")?;
    }
    for change in changes {
        check_member_path(root, &change.path)?;
        if let Some(prior) = &change.previous_path {
            check_member_path(root, prior)?;
        }
        let mut path_variants = vec![(&change.path, current, false)];
        if comparable && let Some(old) = previous {
            path_variants.push((
                change.previous_path.as_ref().unwrap_or(&change.path),
                old,
                true,
            ));
        }
        for (path, graph, old) in path_variants {
            let mut found = false;
            if path == &format!("{root}/Cargo.lock")
                || path.starts_with(&format!("{root}/.cargo/"))
                || path == &format!("{root}/rust-toolchain")
                || path == &format!("{root}/rust-toolchain.toml")
            {
                fallback = true;
                push_unique(&mut unresolved, "global_cargo_inputs_changed")?;
                continue;
            }
            for member_id in &graph.workspace_members {
                let package = &graph.packages[member_id];
                if package.manifest_path == *path
                    || package
                        .targets
                        .iter()
                        .any(|target| target.source_path == *path)
                {
                    found = true;
                    for (id, chain) in reverse_dependents(graph, member_id)? {
                        if !current.workspace_members.contains(&id) {
                            fallback = true;
                            push_unique(
                                &mut unresolved,
                                "removed_or_reidentified_workspace_member",
                            )?;
                            continue;
                        }
                        let reason = ImpactReason {
                            kind: match (old, chain.len() == 1) {
                                (false, true) => ImpactReasonKind::CurrentMembership,
                                (false, false) => ImpactReasonKind::CurrentDependency,
                                (true, true) => ImpactReasonKind::PredecessorMembership,
                                (true, false) => ImpactReasonKind::PredecessorDependency,
                            },
                            path: path.to_string(),
                            chain,
                        };
                        let (reasons, omitted) = candidates.entry(id).or_default();
                        if !reasons.contains(&reason) {
                            if reasons.len() < 8 {
                                reasons.push(reason);
                            } else {
                                *omitted += 1;
                            }
                        }
                    }
                }
            }
            if !found {
                // Cargo does not enumerate module files, include_str!/include_bytes!
                // assets or build-script inputs. No extension proves irrelevance,
                // and a match in the other revision cannot close this frontier.
                fallback = true;
                push_unique(
                    &mut unresolved,
                    if old {
                        "prior_source_membership_not_enumerated"
                    } else {
                        "source_membership_not_enumerated_by_cargo"
                    },
                )?;
            }
        }
    }
    if fallback {
        // This is a diagnostic projection, not an authorization to execute all.
        for id in &current.workspace_members {
            let (reasons, omitted) = candidates.entry(id.clone()).or_default();
            let fallback_reason = ImpactReason {
                kind: ImpactReasonKind::UnresolvedConservativeFallback,
                path: String::new(),
                chain: vec![id.clone()],
            };
            if !reasons.contains(&fallback_reason) {
                if reasons.len() < 8 {
                    reasons.push(fallback_reason);
                } else {
                    *omitted += 1;
                }
            }
        }
    }
    let mut selected = Vec::new();
    let mut deferred_package_ids = Vec::new();
    for (id, (mut reasons, omitted_reasons)) in candidates {
        if selected.len() >= max_targets {
            deferred_package_ids.push(id);
            continue;
        }
        reasons.sort_by(|a, b| (&a.kind, &a.path, &a.chain).cmp(&(&b.kind, &b.path, &b.chain)));
        reasons.dedup();
        // The omitted count was accumulated as the bounded explanations filled.
        let package = &current.packages[&id];
        selected.push(ImpactTarget {
            package_id: id,
            package_name: package.name.clone(),
            reasons,
            omitted_reasons,
        });
    }
    let exact = !fallback && deferred_package_ids.is_empty() && unresolved.is_empty();
    Ok(Impact {
        current_context: current.context_sha256.clone(),
        selected,
        deferred_package_ids,
        unresolved,
        conservative_fallback: fallback,
        exact,
    })
}

fn push_unique(frontiers: &mut Vec<String>, code: &str) -> Result<(), Error> {
    if frontiers.len() >= MAX_EDGES {
        return Err(Error::Limit("impact frontier count"));
    }
    if !frontiers.iter().any(|s| s == code) {
        frontiers.push(code.to_owned());
    }
    Ok(())
}

fn reverse_dependents(
    graph: &CargoGraph,
    owner: &str,
) -> Result<Vec<(String, Vec<String>)>, Error> {
    let mut dependents = BTreeMap::<String, Vec<String>>::new();
    for edge in &graph.resolved_edges {
        dependents
            .entry(edge.to.clone())
            .or_default()
            .push(edge.from.clone());
    }
    for list in dependents.values_mut() {
        list.sort();
        list.dedup();
    }
    let mut visited = BTreeSet::<String>::new();
    let mut queue = VecDeque::new();
    queue.push_back((owner.to_owned(), vec![owner.to_owned()]));
    let mut results = Vec::new();
    while let Some((id, path)) = queue.pop_front() {
        if !visited.insert(id.clone()) {
            continue;
        }
        if path.len() > crate::MAX_PACKAGES {
            return Err(Error::Limit("dependency traversal depth"));
        }
        if graph.workspace_members.contains(&id) {
            results.push((id.clone(), path.clone()));
        }
        if let Some(others) = dependents.get(&id) {
            for other in others {
                if !visited.contains(other) {
                    let mut next = path.clone();
                    next.push(other.clone());
                    queue.push_back((other.clone(), next));
                }
            }
        }
    }
    Ok(results)
}
