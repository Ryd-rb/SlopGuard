//! Cargo metadata format-version 1; resolved edges are variant-specific.

use crate::{
    Binding, Error, MAX_EDGES, MAX_LOG_BYTES, MAX_MANIFEST_BYTES, MAX_METADATA_BYTES, MAX_PACKAGES,
    MAX_SOURCE_TOTAL_BYTES, MAX_TARGETS, WorkerCapture, WorkerExit, check_member_path, hash_field,
    json_array, json_object, metadata_argv, sha256_hex, string,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Caller-captured exact file bytes from the same revision as Cargo execution.
/// An expected digest is mandatory: the adapter recomputes it before accepting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub expected_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestDigest {
    pub path: String,
    pub sha256: String,
}

pub struct MetadataInput {
    pub binding: Binding,
    pub manifest_path: String,
    /// Root workspace manifest plus all evaluated workspace-member manifests.
    pub manifests: Vec<SnapshotFile>,
    pub lock: SnapshotFile,
    pub capture: WorkerCapture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// Cargo's target/feature-specific graph is evaluated and inputs are bound.
    /// Does NOT assert Rust semantic, cfg-expanded or test-discovery completeness.
    EvaluatedCargoGraph,
    /// A file/evaluation frontier is unresolved; never use as complete evidence.
    PartialCargoGraph,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoTarget {
    pub name: String,
    pub kinds: Vec<String>,
    pub source_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredDependency {
    pub name: String,
    pub alias: String,
    pub requirement: String,
    pub kind: Option<String>,
    pub target_expression: Option<String>,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoPackage {
    pub id: String,
    pub name: String,
    pub version: String,
    pub edition: String,
    pub manifest_path: String,
    pub workspace_member: bool,
    /// Only declared target roots; Cargo metadata does not enumerate every `mod`.
    pub targets: Vec<CargoTarget>,
    /// Declarations may be inactive in this feature or target variant.
    pub declared_dependencies: Vec<DeclaredDependency>,
    /// Features actually selected by Cargo's resolver for this node.
    pub activated_features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepKind {
    /// `None` is Cargo's normal dependency kind.
    pub kind: Option<String>,
    /// Raw Cargo cfg/target expression, NOT evaluated again by the adapter.
    pub target_expression: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoEdge {
    pub from: String,
    pub to: String,
    pub alias: String,
    pub kinds: Vec<DepKind>,
    /// If not unambiguously matched to a declaration, no optionality is inferred.
    pub optional: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoGraph {
    pub binding: Binding,
    pub root_manifest: String,
    pub workspace_members: BTreeSet<String>,
    pub packages: BTreeMap<String, CargoPackage>,
    pub resolved_edges: Vec<CargoEdge>,
    pub manifest_digests: Vec<ManifestDigest>,
    pub lock_sha256: String,
    pub metadata_sha256: String,
    /// Domain-separated identity for context + manifests + lock + output.
    pub context_sha256: String,
    pub coverage: Coverage,
    pub unresolved: Vec<String>,
}

impl CargoGraph {
    pub fn identity_matches(&self, other: &Self) -> bool {
        let a = &self.binding;
        let b = &other.binding;
        self.root_manifest == other.root_manifest
            && a.workspace_root == b.workspace_root
            && a.rustc_version == b.rustc_version
            && a.target_triple == b.target_triple
            && a.rustc_vv_sha256 == b.rustc_vv_sha256
            && a.cargo_executable_sha256 == b.cargo_executable_sha256
            && a.environment_sha256 == b.environment_sha256
            && a.features == b.features
    }

    pub fn test_package_spec(&self, id: &str) -> Result<String, Error> {
        if !self.workspace_members.contains(id) {
            return Err(Error::Invalid(
                "requested test package is not a workspace member",
            ));
        }
        let p = self
            .packages
            .get(id)
            .ok_or(Error::Malformed("missing package"))?;
        Ok(format!("{}@{}", p.name, p.version))
    }
}

pub fn interpret_metadata(input: MetadataInput) -> Result<CargoGraph, Error> {
    let MetadataInput {
        binding,
        manifest_path,
        manifests,
        lock,
        capture,
    } = input;
    let argv = metadata_argv(&binding, &manifest_path)?;
    capture.check(&binding, &argv)?;
    if capture.stdout.len() > MAX_METADATA_BYTES || capture.stderr.len() > MAX_LOG_BYTES {
        return Err(Error::Limit("Cargo metadata stdout/stderr limit"));
    }
    if capture.truncated {
        return Err(Error::Blocked("truncated Cargo metadata cannot be used"));
    }
    if capture.attempt_id.is_some()
        || capture.fence_sequence.is_some()
        || capture.graph_sha256.is_some()
    {
        return Err(Error::Invalid(
            "metadata capture includes test attempt identity",
        ));
    }
    if capture.exit != WorkerExit::Exited(0) {
        return Err(Error::Blocked(
            "offline locked Cargo metadata did not succeed",
        ));
    }
    let mut snapshot = BTreeMap::<String, String>::new();
    let mut source_total = 0_usize;
    for file in &manifests {
        check_member_path(&binding.workspace_root, &file.path)?;
        if !file.path.ends_with("/Cargo.toml") {
            return Err(Error::Invalid("snapshot contains a nonmanifest"));
        }
        source_total = source_total.saturating_add(file.bytes.len());
        if file.bytes.len() > MAX_MANIFEST_BYTES || source_total > MAX_SOURCE_TOTAL_BYTES {
            return Err(Error::Limit("manifest snapshot byte budget"));
        }
        let actual = sha256_hex(&file.bytes);
        if actual != file.expected_sha256 {
            return Err(Error::Stale(
                "manifest SHA-256 differs from revision snapshot",
            ));
        }
        if snapshot.insert(file.path.clone(), actual).is_some() {
            return Err(Error::Invalid("duplicate manifest in snapshot"));
        }
    }
    if snapshot.len() > MAX_PACKAGES + 1 || !snapshot.contains_key(&manifest_path) {
        return Err(Error::Invalid(
            "root manifest snapshot is absent or too large",
        ));
    }
    if lock.path != format!("{}/Cargo.lock", binding.workspace_root) {
        return Err(Error::Invalid(
            "Cargo.lock does not belong to the workspace root",
        ));
    }
    if lock.bytes.is_empty() || lock.bytes.len() > MAX_MANIFEST_BYTES {
        return Err(Error::Limit("Cargo.lock size"));
    }
    let lock_sha256 = sha256_hex(&lock.bytes);
    if lock.expected_sha256 != lock_sha256 {
        return Err(Error::Stale(
            "Cargo.lock SHA-256 differs from revision snapshot",
        ));
    }

    let document: Value = serde_json::from_slice(&capture.stdout)
        .map_err(|_| Error::Malformed("Cargo metadata JSON cannot be decoded"))?;
    let top = json_object(&document)?;
    if top.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(Error::Unsupported("Cargo metadata format-version is not 1"));
    }
    if string(field(top, "workspace_root")?, 4096)? != binding.workspace_root {
        return Err(Error::Stale(
            "Cargo metadata workspace root differs from admitted root",
        ));
    }
    let raw_members = json_array(field(top, "workspace_members")?)?;
    let raw_packages = json_array(field(top, "packages")?)?;
    if raw_members.len() > MAX_PACKAGES || raw_packages.len() > MAX_PACKAGES {
        return Err(Error::Limit("Cargo package count"));
    }
    let mut workspace_members = BTreeSet::new();
    for item in raw_members {
        let id = string(item, 2048)?;
        if !workspace_members.insert(id) {
            return Err(Error::Malformed("duplicate workspace member id"));
        }
    }
    let mut packages = BTreeMap::new();
    let mut total_targets = 0_usize;
    let mut total_declarations = 0_usize;
    for item in raw_packages {
        let obj = json_object(item)?;
        let id = string(field(obj, "id")?, 2048)?;
        let workspace_member = workspace_members.contains(&id);
        let manifest = string(field(obj, "manifest_path")?, 4096)?;
        if workspace_member {
            check_member_path(&binding.workspace_root, &manifest)?;
        }
        let name = string(field(obj, "name")?, 160)?;
        let version = string(field(obj, "version")?, 160)?;
        let edition = string(field(obj, "edition")?, 16)?;
        if !matches!(edition.as_str(), "2015" | "2018" | "2021" | "2024") {
            return Err(Error::Unsupported("Cargo package edition not qualified"));
        }
        let mut targets = Vec::new();
        for t in json_array(field(obj, "targets")?)? {
            total_targets += 1;
            if total_targets > MAX_TARGETS {
                return Err(Error::Limit("too many Cargo target roots"));
            }
            let t = json_object(t)?;
            let kinds = json_array(field(t, "kind")?)?
                .iter()
                .map(|value| string(value, 80))
                .collect::<Result<Vec<_>, _>>()?;
            if kinds.len() > 16 {
                return Err(Error::Limit("Cargo target kind list"));
            }
            let source_path = string(field(t, "src_path")?, 4096)?;
            targets.push(CargoTarget {
                name: string(field(t, "name")?, 160)?,
                kinds,
                source_path,
            });
        }
        let mut declared_dependencies = Vec::new();
        for d in json_array(field(obj, "dependencies")?)? {
            total_declarations += 1;
            if total_declarations > MAX_EDGES {
                return Err(Error::Limit("too many Cargo dependency declarations"));
            }
            let dep = json_object(d)?;
            let dep_name = string(field(dep, "name")?, 160)?;
            let alias = optional_string(dep, "rename", 160)?.unwrap_or_else(|| dep_name.clone());
            declared_dependencies.push(DeclaredDependency {
                name: dep_name,
                alias,
                requirement: string(field(dep, "req")?, 256)?,
                kind: optional_string(dep, "kind", 16)?,
                target_expression: optional_string(dep, "target", 512)?,
                optional: field(dep, "optional")?
                    .as_bool()
                    .ok_or(Error::Malformed("dependency optional is not a bool"))?,
            });
        }
        if packages
            .insert(
                id.clone(),
                CargoPackage {
                    id,
                    name,
                    version,
                    edition,
                    manifest_path: manifest,
                    workspace_member,
                    targets,
                    declared_dependencies,
                    activated_features: Vec::new(),
                },
            )
            .is_some()
        {
            return Err(Error::Malformed("duplicate Cargo package id"));
        }
    }
    if workspace_members
        .iter()
        .any(|id| !packages.contains_key(id))
    {
        return Err(Error::Malformed("workspace member missing from packages"));
    }
    let resolve = json_object(field(top, "resolve")?)?;
    let nodes = json_array(field(resolve, "nodes")?)?;
    if nodes.len() > MAX_PACKAGES {
        return Err(Error::Limit("too many Cargo resolve nodes"));
    }
    let mut edges = Vec::new();
    let mut unresolved = Vec::new();
    let mut observed = BTreeSet::new();
    for node in nodes {
        let n = json_object(node)?;
        let from = string(field(n, "id")?, 2048)?;
        let package = packages
            .get_mut(&from)
            .ok_or(Error::Malformed("resolve node lacks package"))?;
        if !observed.insert(from.clone()) {
            return Err(Error::Malformed("duplicate Cargo resolve node"));
        }
        package.activated_features = json_array(field(n, "features")?)?
            .iter()
            .map(|v| string(v, 128))
            .collect::<Result<Vec<_>, _>>()?;
        if package.activated_features.len() > 128 {
            return Err(Error::Limit("activated feature count"));
        }
        let declarations = package.declared_dependencies.clone();
        for d in json_array(field(n, "deps")?)? {
            if edges.len() == MAX_EDGES {
                return Err(Error::Limit("resolved dependency count"));
            }
            let dep = json_object(d)?;
            let alias = string(field(dep, "name")?, 160)?;
            let to = string(field(dep, "pkg")?, 2048)?;
            if !packages.contains_key(&to) {
                // Metadata from Cargo must be internally closed. An omission
                // cannot be silently treated as a non-impacting dependency.
                return Err(Error::Malformed("resolved dependency package absent"));
            }
            let mut kinds = Vec::new();
            for kind in json_array(field(dep, "dep_kinds")?)? {
                if kinds.len() >= 16 {
                    return Err(Error::Limit("dependency kind count"));
                }
                let k = json_object(kind)?;
                kinds.push(DepKind {
                    kind: optional_string(k, "kind", 16)?,
                    target_expression: optional_string(k, "target", 512)?,
                });
            }
            if kinds.is_empty() {
                bounded_frontier(&mut unresolved, "unclassified_dep_kind")?;
            }
            let candidates: Vec<_> = declarations.iter().filter(|d| d.alias == alias).collect();
            let optional = if let Some(first) = candidates.first()
                && candidates.iter().all(|d| d.optional == first.optional)
            {
                Some(first.optional)
            } else {
                bounded_frontier(&mut unresolved, "ambiguous_declared_dependency")?;
                None
            };
            edges.push(CargoEdge {
                from: from.clone(),
                to,
                alias,
                kinds,
                optional,
            });
        }
    }
    for id in &workspace_members {
        if !observed.contains(id) {
            bounded_frontier(&mut unresolved, "workspace_member_not_resolved")?;
        }
        let member = &packages[id];
        if !snapshot.contains_key(&member.manifest_path) {
            bounded_frontier(&mut unresolved, "member_manifest_digest_missing")?;
        }
        for target in &member.targets {
            if check_member_path(&binding.workspace_root, &target.source_path).is_err() {
                bounded_frontier(&mut unresolved, "target_source_outside_workspace")?;
            }
        }
    }
    if snapshot.keys().any(|path| {
        path != &manifest_path
            && !workspace_members
                .iter()
                .any(|id| packages[id].manifest_path.as_str() == path.as_str())
    }) {
        return Err(Error::Invalid(
            "snapshot has an unaccounted workspace manifest",
        ));
    }
    let manifests = snapshot
        .into_iter()
        .map(|(path, sha256)| ManifestDigest { path, sha256 })
        .collect::<Vec<_>>();
    let metadata_sha256 = sha256_hex(&capture.stdout);
    let mut h = Sha256::new();
    hash_field(&mut h, b"slophawk.cargo.graph.v1");
    hash_field(&mut h, binding.digest()?.as_bytes());
    hash_field(&mut h, manifest_path.as_bytes());
    for digest in &manifests {
        hash_field(&mut h, digest.path.as_bytes());
        hash_field(&mut h, digest.sha256.as_bytes());
    }
    hash_field(&mut h, lock_sha256.as_bytes());
    hash_field(&mut h, metadata_sha256.as_bytes());
    let context_sha256 = format!("{:x}", h.finalize());
    Ok(CargoGraph {
        binding,
        root_manifest: manifest_path,
        workspace_members,
        packages,
        resolved_edges: edges,
        manifest_digests: manifests,
        lock_sha256,
        metadata_sha256,
        context_sha256,
        coverage: if unresolved.is_empty() {
            Coverage::EvaluatedCargoGraph
        } else {
            Coverage::PartialCargoGraph
        },
        unresolved,
    })
}

fn field<'a>(obj: &'a serde_json::Map<String, Value>, name: &str) -> Result<&'a Value, Error> {
    obj.get(name)
        .ok_or(Error::Malformed("required Cargo metadata field absent"))
}

fn optional_string(
    obj: &serde_json::Map<String, Value>,
    name: &str,
    limit: usize,
) -> Result<Option<String>, Error> {
    match field(obj, name)? {
        Value::Null => Ok(None),
        value => Ok(Some(string(value, limit)?)),
    }
}

fn bounded_frontier(frontiers: &mut Vec<String>, reason: &str) -> Result<(), Error> {
    if frontiers.len() == MAX_EDGES {
        return Err(Error::Limit("unresolved Cargo frontiers"));
    }
    if !frontiers.iter().any(|v| v == reason) {
        frontiers.push(reason.to_owned());
    }
    Ok(())
}
