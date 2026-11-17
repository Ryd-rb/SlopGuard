//! A typed, in-memory SG-10 pretokenized whole-block index.
//!
//! This crate intentionally does not tokenize source, discover near clones, persist indexes,
//! or start language providers. Writers and readers must be serialized by the caller.
//! Locations retain the provider's zero-based half-open offset convention.

mod sha256;

use sha256::Sha256;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

pub const RULE_VERSION: &str = "sg10.token-clones.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CloneTokenKind {
    Other = 0,
    LocalIdentifier = 1,
    BoundIdentifier = 2,
    Literal = 3,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneToken {
    pub kind: CloneTokenKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloneSourceSpan {
    pub start_offset: i32,
    pub end_offset: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneTokenBlock {
    pub block_id: String,
    pub span: CloneSourceSpan,
    pub tokens: Vec<CloneToken>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneTokenDocument {
    pub document_id: String,
    pub version: String,
    pub is_generated: bool,
    pub is_excluded: bool,
    pub blocks: Vec<CloneTokenBlock>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloneIndexSettings {
    pub minimum_tokens: usize,
    pub maximum_tokens_per_block: usize,
    pub maximum_fingerprint_postings: usize,
}

impl Default for CloneIndexSettings {
    fn default() -> Self {
        Self {
            minimum_tokens: 40,
            maximum_tokens_per_block: 4096,
            maximum_fingerprint_postings: 64,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloneQueryLimits {
    pub maximum_candidate_pairs: usize,
    pub maximum_output_pairs: usize,
    pub maximum_compared_tokens: i64,
}

impl Default for CloneQueryLimits {
    fn default() -> Self {
        Self {
            maximum_candidate_pairs: 20_000,
            maximum_output_pairs: 10_000,
            maximum_compared_tokens: 2_000_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CloneMatchClass {
    Exact,
    RenamedLocalIdentifiers,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneMatchLocation {
    pub document_id: String,
    pub document_version: String,
    pub block_id: String,
    pub span: CloneSourceSpan,
    pub token_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CloneTokenPair {
    pub pair_id: String,
    pub match_class: CloneMatchClass,
    pub normalization_rule_version: &'static str,
    pub left: CloneMatchLocation,
    pub right: CloneMatchLocation,
    pub exact_matched_tokens: usize,
    pub normalized_matched_tokens: usize,
    pub score: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CloneScopeGapReason {
    OversizedBlock,
    BoilerplatePostingLimit,
    CandidatePairLimit,
    ComparedTokenLimit,
    OutputPairLimit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneScopeGap {
    pub reason: CloneScopeGapReason,
    pub scope_key: String,
    pub affected_count: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneQueryStatistics {
    pub document_count: usize,
    pub indexed_block_count: usize,
    pub generated_documents: usize,
    pub excluded_documents: usize,
    pub below_minimum_blocks: usize,
    pub oversized_blocks: usize,
    pub candidate_pairs_examined: usize,
    pub compared_tokens_charged: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CloneQueryResult {
    pub pairs: Vec<CloneTokenPair>,
    pub statistics: CloneQueryStatistics,
    pub gaps: Vec<CloneScopeGap>,
    pub index_settings: CloneIndexSettings,
    pub applied_limits: CloneQueryLimits,
}

impl CloneQueryResult {
    #[must_use]
    pub fn is_truncated(&self) -> bool {
        !self.gaps.is_empty()
    }
    #[must_use]
    pub fn normalization_rule_version(&self) -> &'static str {
        RULE_VERSION
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloneIndexError {
    InvalidSettings,
    InvalidLimits,
    EmptyDocumentId,
    EmptyVersion,
    EmptyBlockId,
    DuplicateBlockId(String),
    InvalidSpan(String),
    InvalidToken(String),
}

impl fmt::Display for CloneIndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CloneIndexError {}

// The BTreeSet order MUST be .NET StringComparer.Ordinal (UTF-16 code units), not
// Rust's native string/UTF-8 ordering. In particular U+10000 sorts before U+E000.
fn ordinal_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct BlockKey {
    doc: Arc<str>,
    block: Arc<str>,
}
impl Ord for BlockKey {
    fn cmp(&self, other: &Self) -> Ordering {
        ordinal_cmp(&self.doc, &other.doc).then_with(|| ordinal_cmp(&self.block, &other.block))
    }
}
impl PartialOrd for BlockKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone)]
struct BlockMeta {
    key: BlockKey,
    token_count: usize,
}
struct StoredDocument {
    generated: bool,
    excluded: bool,
    blocks: Vec<BlockMeta>,
}

// 8-byte (kind + per-document interned word index) instead of per-token Strings.
// For normalized local names, 'word' is the 0-based renaming ordinal, not a table slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PackedToken {
    kind: CloneTokenKind,
    word: u32,
}
struct IndexedBlock {
    location: CloneMatchLocation,
    words: Arc<Vec<Box<str>>>,
    exact: Vec<PackedToken>,
    normalized: Vec<PackedToken>,
    exact_fp: [u8; 32],
    normalized_fp: [u8; 32],
}
struct PendingIndexed {
    key: BlockKey,
    location: CloneMatchLocation,
    exact: Vec<PackedToken>,
    normalized: Vec<PackedToken>,
    exact_fp: [u8; 32],
    normalized_fp: [u8; 32],
}

/// Query visits all exact fingerprint buckets before normalized buckets and
/// bounds the number of examined candidates, returned pairs and charged tokens.
/// `upsert` snapshots input and is atomic with respect to input validation.
pub struct TokenCloneIndex {
    settings: CloneIndexSettings,
    documents: HashMap<Arc<str>, StoredDocument>,
    blocks: HashMap<BlockKey, IndexedBlock>,
    exact: HashMap<[u8; 32], BTreeSet<BlockKey>>,
    normalized: HashMap<[u8; 32], BTreeSet<BlockKey>>,
}

impl TokenCloneIndex {
    pub fn new(settings: CloneIndexSettings) -> Result<Self, CloneIndexError> {
        if settings.minimum_tokens == 0
            || settings.maximum_tokens_per_block < settings.minimum_tokens
            || settings.maximum_fingerprint_postings < 2
            || settings.maximum_tokens_per_block > i32::MAX as usize
        {
            return Err(CloneIndexError::InvalidSettings);
        }
        Ok(Self {
            settings,
            documents: HashMap::new(),
            blocks: HashMap::new(),
            exact: HashMap::new(),
            normalized: HashMap::new(),
        })
    }

    #[must_use]
    pub fn document_count(&self) -> usize {
        self.documents.len()
    }

    pub fn upsert(&mut self, document: CloneTokenDocument) -> Result<(), CloneIndexError> {
        if invalid_id(&document.document_id) {
            return Err(CloneIndexError::EmptyDocumentId);
        }
        if invalid_id(&document.version) {
            return Err(CloneIndexError::EmptyVersion);
        }
        let doc: Arc<str> = Arc::from(document.document_id.as_str());
        let mut ids = HashSet::with_capacity(document.blocks.len());
        let mut metadata = Vec::with_capacity(document.blocks.len());
        let mut pending = Vec::new();
        // Intern strings within one document, shared by all indexed blocks in that snapshot.
        let mut word_ids: HashMap<String, u32> = HashMap::new();
        let mut words: Vec<Box<str>> = Vec::new();
        for block in document.blocks {
            if invalid_id(&block.block_id) {
                return Err(CloneIndexError::EmptyBlockId);
            }
            if !ids.insert(block.block_id.clone()) {
                return Err(CloneIndexError::DuplicateBlockId(block.block_id));
            }
            if block.span.start_offset < 0 || block.span.end_offset <= block.span.start_offset {
                return Err(CloneIndexError::InvalidSpan(block.block_id));
            }
            for token in &block.tokens {
                if token.text.is_empty() || token.text.len() > i32::MAX as usize {
                    return Err(CloneIndexError::InvalidToken(block.block_id));
                }
            }
            let count = block.tokens.len();
            let key = BlockKey {
                doc: doc.clone(),
                block: Arc::from(block.block_id.as_str()),
            };
            metadata.push(BlockMeta {
                key: key.clone(),
                token_count: count,
            });
            if document.is_generated
                || document.is_excluded
                || count < self.settings.minimum_tokens
                || count > self.settings.maximum_tokens_per_block
            {
                continue;
            }
            let mut exact = Vec::with_capacity(count);
            for token in &block.tokens {
                let id = if let Some(&id) = word_ids.get(token.text.as_str()) {
                    id
                } else {
                    let id = u32::try_from(words.len())
                        .map_err(|_| CloneIndexError::InvalidToken(block.block_id.clone()))?;
                    words.push(token.text.clone().into_boxed_str());
                    word_ids.insert(token.text.clone(), id);
                    id
                };
                exact.push(PackedToken {
                    kind: token.kind,
                    word: id,
                });
            }
            let normalized = normalize(&exact);
            let exact_fp = fingerprint(&exact, &words, false);
            let normalized_fp = fingerprint(&normalized, &words, true);
            pending.push(PendingIndexed {
                key,
                location: CloneMatchLocation {
                    document_id: document.document_id.clone(),
                    document_version: document.version.clone(),
                    block_id: block.block_id,
                    span: block.span,
                    token_count: count,
                },
                exact,
                normalized,
                exact_fp,
                normalized_fp,
            });
        }
        // All validation and hashing precedes removal of the old snapshot.
        self.remove_document(&document.document_id);
        self.documents.insert(
            doc,
            StoredDocument {
                generated: document.is_generated,
                excluded: document.is_excluded,
                blocks: metadata,
            },
        );
        let words = Arc::new(words);
        for block in pending {
            self.exact
                .entry(block.exact_fp)
                .or_default()
                .insert(block.key.clone());
            self.normalized
                .entry(block.normalized_fp)
                .or_default()
                .insert(block.key.clone());
            self.blocks.insert(
                block.key,
                IndexedBlock {
                    location: block.location,
                    words: Arc::clone(&words),
                    exact: block.exact,
                    normalized: block.normalized,
                    exact_fp: block.exact_fp,
                    normalized_fp: block.normalized_fp,
                },
            );
        }
        Ok(())
    }

    pub fn delete(&mut self, document_id: &str) -> Result<bool, CloneIndexError> {
        if invalid_id(document_id) {
            return Err(CloneIndexError::EmptyDocumentId);
        }
        Ok(self.remove_document(document_id))
    }

    fn remove_document(&mut self, document_id: &str) -> bool {
        let Some(old) = self.documents.remove(document_id) else {
            return false;
        };
        for meta in old.blocks {
            if let Some(block) = self.blocks.remove(&meta.key) {
                remove_posting(&mut self.exact, &block.exact_fp, &meta.key);
                remove_posting(&mut self.normalized, &block.normalized_fp, &meta.key);
            }
        }
        true
    }

    pub fn query(&self, limits: CloneQueryLimits) -> Result<CloneQueryResult, CloneIndexError> {
        if limits.maximum_candidate_pairs == 0
            || limits.maximum_output_pairs == 0
            || limits.maximum_compared_tokens < 1
        {
            return Err(CloneIndexError::InvalidLimits);
        }
        let mut gaps = Vec::new();
        let mut pairs = Vec::new();
        let mut ordered: Vec<&BlockKey> = self.blocks.keys().collect();
        ordered.sort_unstable();
        let mut seen = HashSet::<String>::new();
        let mut suppressed = HashSet::<String>::new();
        let mut examined = 0usize;
        let mut charged = 0i64;
        let mut below_minimum = 0;
        let mut oversized = 0;
        let mut generated = 0;
        let mut excluded = 0;
        for doc in self.documents.values() {
            if doc.excluded {
                excluded += 1;
            } else if doc.generated {
                generated += 1;
            } else {
                for block in &doc.blocks {
                    if block.token_count < self.settings.minimum_tokens {
                        below_minimum += 1;
                    } else if block.token_count > self.settings.maximum_tokens_per_block {
                        oversized += 1;
                        gaps.push(CloneScopeGap {
                            reason: CloneScopeGapReason::OversizedBlock,
                            scope_key: block_identity(&block.key),
                            affected_count: block.token_count as i64,
                        });
                    }
                }
            }
        }
        for exact_pass in [true, false] {
            let postings = if exact_pass {
                &self.exact
            } else {
                &self.normalized
            };
            for left_key in &ordered {
                let left = &self.blocks[*left_key];
                let fp = if exact_pass {
                    &left.exact_fp
                } else {
                    &left.normalized_fp
                };
                let bucket = &postings[fp];
                if bucket.len() > self.settings.maximum_fingerprint_postings {
                    let bucket_id = format!(
                        "{}:{}",
                        if exact_pass { "exact" } else { "normalized" },
                        hex(fp)
                    );
                    if suppressed.insert(bucket_id.clone()) {
                        gaps.push(CloneScopeGap {
                            reason: CloneScopeGapReason::BoilerplatePostingLimit,
                            scope_key: bucket_id,
                            affected_count: (bucket.len() as i64 * (bucket.len() as i64 - 1)) / 2,
                        });
                    }
                    continue;
                }
                for right_key in bucket {
                    if left_key.cmp(&right_key) != Ordering::Less {
                        continue;
                    }
                    let pair_id = pair_identity(left_key, right_key);
                    if seen.contains(&pair_id) {
                        continue;
                    }
                    if examined >= limits.maximum_candidate_pairs {
                        gaps.push(CloneScopeGap {
                            reason: CloneScopeGapReason::CandidatePairLimit,
                            scope_key: pair_id,
                            affected_count: 1,
                        });
                        return Ok(finish(
                            self.settings,
                            limits,
                            pairs,
                            gaps,
                            ordered.len(),
                            self.documents.len(),
                            generated,
                            excluded,
                            below_minimum,
                            oversized,
                            examined,
                            charged,
                        ));
                    }
                    let right = &self.blocks[right_key];
                    let cost = (left.location.token_count + right.location.token_count) as i64;
                    if charged > limits.maximum_compared_tokens - cost {
                        gaps.push(CloneScopeGap {
                            reason: CloneScopeGapReason::ComparedTokenLimit,
                            scope_key: pair_id,
                            affected_count: cost,
                        });
                        return Ok(finish(
                            self.settings,
                            limits,
                            pairs,
                            gaps,
                            ordered.len(),
                            self.documents.len(),
                            generated,
                            excluded,
                            below_minimum,
                            oversized,
                            examined,
                            charged,
                        ));
                    }
                    examined += 1;
                    charged += cost;
                    if left.location.token_count != right.location.token_count
                        || !equal(left, right, true)
                    {
                        continue;
                    }
                    let equal_exact = equal(left, right, false);
                    if exact_pass && !equal_exact {
                        continue;
                    }
                    if pairs.len() >= limits.maximum_output_pairs {
                        gaps.push(CloneScopeGap {
                            reason: CloneScopeGapReason::OutputPairLimit,
                            scope_key: pair_id,
                            affected_count: 1,
                        });
                        return Ok(finish(
                            self.settings,
                            limits,
                            pairs,
                            gaps,
                            ordered.len(),
                            self.documents.len(),
                            generated,
                            excluded,
                            below_minimum,
                            oversized,
                            examined,
                            charged,
                        ));
                    }
                    seen.insert(pair_id.clone());
                    let count = left.location.token_count;
                    pairs.push(CloneTokenPair {
                        pair_id,
                        match_class: if equal_exact {
                            CloneMatchClass::Exact
                        } else {
                            CloneMatchClass::RenamedLocalIdentifiers
                        },
                        normalization_rule_version: RULE_VERSION,
                        left: left.location.clone(),
                        right: right.location.clone(),
                        exact_matched_tokens: if equal_exact {
                            count
                        } else {
                            count_equal(left, right)
                        },
                        normalized_matched_tokens: count,
                        score: 1.0,
                    });
                }
            }
        }
        Ok(finish(
            self.settings,
            limits,
            pairs,
            gaps,
            ordered.len(),
            self.documents.len(),
            generated,
            excluded,
            below_minimum,
            oversized,
            examined,
            charged,
        ))
    }
}

fn invalid_id(id: &str) -> bool {
    id.is_empty() || id.chars().all(char::is_whitespace)
}

fn remove_posting(map: &mut HashMap<[u8; 32], BTreeSet<BlockKey>>, fp: &[u8; 32], key: &BlockKey) {
    if let Some(bucket) = map.get_mut(fp) {
        bucket.remove(key);
        if bucket.is_empty() {
            map.remove(fp);
        }
    }
}

fn normalize(tokens: &[PackedToken]) -> Vec<PackedToken> {
    let mut locals: HashMap<u32, u32> = HashMap::new();
    tokens
        .iter()
        .map(|token| {
            if token.kind != CloneTokenKind::LocalIdentifier {
                *token
            } else {
                let next = locals.len() as u32;
                let word = *locals.entry(token.word).or_insert(next);
                PackedToken {
                    kind: token.kind,
                    word,
                }
            }
        })
        .collect()
}

fn fingerprint(tokens: &[PackedToken], words: &[Box<str>], normalized: bool) -> [u8; 32] {
    let mut hash = Sha256::new();
    for token in tokens {
        let name;
        let text: &str = if normalized && token.kind == CloneTokenKind::LocalIdentifier {
            name = token.word.to_string();
            name.as_str()
        } else {
            words[token.word as usize].as_ref()
        };
        let bytes = text.as_bytes();
        hash.update(&(token.kind as i32).to_le_bytes());
        hash.update(&(bytes.len() as i32).to_le_bytes());
        hash.update(bytes);
    }
    hash.finish()
}

fn equal(a: &IndexedBlock, b: &IndexedBlock, normalized: bool) -> bool {
    let left = if normalized { &a.normalized } else { &a.exact };
    let right = if normalized { &b.normalized } else { &b.exact };
    left.iter().zip(right).all(|(a_token, b_token)| {
        a_token.kind == b_token.kind
            && (normalized
                && a_token.kind == CloneTokenKind::LocalIdentifier
                && a_token.word == b_token.word
                || !(normalized && a_token.kind == CloneTokenKind::LocalIdentifier)
                    && a.words[a_token.word as usize] == b.words[b_token.word as usize])
    })
}

fn count_equal(a: &IndexedBlock, b: &IndexedBlock) -> usize {
    a.exact
        .iter()
        .zip(&b.exact)
        .filter(|(left, right)| {
            left.kind == right.kind && a.words[left.word as usize] == b.words[right.word as usize]
        })
        .count()
}

fn block_identity(key: &BlockKey) -> String {
    // C# string.Length counts UTF-16 code units, including a surrogate pair as 2.
    format!(
        "{}:{}{}:{}",
        key.doc.encode_utf16().count(),
        key.doc,
        key.block.encode_utf16().count(),
        key.block
    )
}
fn pair_identity(a: &BlockKey, b: &BlockKey) -> String {
    format!("{RULE_VERSION}:{}/{}", block_identity(a), block_identity(b))
}
fn hex(fp: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(64);
    for byte in fp {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xF) as usize] as char);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn finish(
    settings: CloneIndexSettings,
    limits: CloneQueryLimits,
    mut pairs: Vec<CloneTokenPair>,
    mut gaps: Vec<CloneScopeGap>,
    indexed: usize,
    documents: usize,
    generated: usize,
    excluded: usize,
    below_minimum: usize,
    oversized: usize,
    examined: usize,
    charged: i64,
) -> CloneQueryResult {
    pairs.sort_by(|a, b| {
        a.match_class
            .cmp(&b.match_class)
            .then_with(|| {
                b.normalized_matched_tokens
                    .cmp(&a.normalized_matched_tokens)
            })
            .then_with(|| ordinal_cmp(&a.pair_id, &b.pair_id))
    });
    gaps.sort_by(|a, b| {
        a.reason
            .cmp(&b.reason)
            .then_with(|| ordinal_cmp(&a.scope_key, &b.scope_key))
    });
    CloneQueryResult {
        pairs,
        gaps,
        statistics: CloneQueryStatistics {
            document_count: documents,
            indexed_block_count: indexed,
            generated_documents: generated,
            excluded_documents: excluded,
            below_minimum_blocks: below_minimum,
            oversized_blocks: oversized,
            candidate_pairs_examined: examined,
            compared_tokens_charged: charged,
        },
        index_settings: settings,
        applied_limits: limits,
    }
}

impl Default for TokenCloneIndex {
    fn default() -> Self {
        Self::new(CloneIndexSettings::default()).expect("valid defaults")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_token_representation_is_eight_bytes() {
        assert_eq!(std::mem::size_of::<PackedToken>(), 8);
    }
    #[test]
    fn dotnet_ordinal_and_utf16_pair_identity() {
        assert_eq!(ordinal_cmp("𐐀", "\u{E000}"), Ordering::Less);
        assert_eq!(ordinal_cmp("a", "a\0"), Ordering::Less);
        let a = BlockKey {
            doc: Arc::from("𐐀"),
            block: Arc::from("𐐀"),
        };
        assert_eq!(block_identity(&a), "2:𐐀2:𐐀");
    }
    #[test]
    fn fingerprint_collision_never_accepts_different_sequences() {
        let settings = CloneIndexSettings {
            minimum_tokens: 1,
            maximum_tokens_per_block: 32,
            maximum_fingerprint_postings: 8,
        };
        let mut index = TokenCloneIndex::new(settings).unwrap();
        for (id, literal) in [("a", "one"), ("b", "two")] {
            index
                .upsert(CloneTokenDocument {
                    document_id: id.into(),
                    version: "1".into(),
                    is_generated: false,
                    is_excluded: false,
                    blocks: vec![CloneTokenBlock {
                        block_id: "m".into(),
                        span: CloneSourceSpan {
                            start_offset: 0,
                            end_offset: 2,
                        },
                        tokens: vec![CloneToken {
                            kind: CloneTokenKind::Literal,
                            text: literal.into(),
                        }],
                    }],
                })
                .unwrap();
        }
        let a = BlockKey {
            doc: Arc::from("a"),
            block: Arc::from("m"),
        };
        let b = BlockKey {
            doc: Arc::from("b"),
            block: Arc::from("m"),
        };
        let target_exact = index.blocks[&a].exact_fp;
        let target_normalized = index.blocks[&a].normalized_fp;
        let old_exact = index.blocks[&b].exact_fp;
        let old_normalized = index.blocks[&b].normalized_fp;
        remove_posting(&mut index.exact, &old_exact, &b);
        remove_posting(&mut index.normalized, &old_normalized, &b);
        index
            .exact
            .entry(target_exact)
            .or_default()
            .insert(b.clone());
        index
            .normalized
            .entry(target_normalized)
            .or_default()
            .insert(b.clone());
        index.blocks.get_mut(&b).unwrap().exact_fp = target_exact;
        index.blocks.get_mut(&b).unwrap().normalized_fp = target_normalized;
        let result = index.query(CloneQueryLimits::default()).unwrap();
        assert!(
            result.pairs.is_empty(),
            "a digest collision must not create a clone"
        );
        assert_eq!(result.statistics.candidate_pairs_examined, 2);
    }
    #[test]
    fn fingerprints_match_dotnet_kind_length_framing() {
        let words = vec!["é".into(), "𐐀".into()];
        let tokens = vec![
            PackedToken {
                kind: CloneTokenKind::LocalIdentifier,
                word: 0,
            },
            PackedToken {
                kind: CloneTokenKind::Literal,
                word: 1,
            },
        ];
        // Compare the exact result to SHA256 over little-endian kinds/UTF-8 byte lengths,
        // not a hash of the display strings or a UTF-16 length.
        assert_eq!(
            hex(&fingerprint(&tokens, &words, false)),
            "6F0721966D2EA0C00F31D0FBBF8ACDBE4BBF1C9C2A4CFC863AF768CED406FE80"
        );
        assert_eq!(
            hex(&fingerprint(&normalize(&tokens), &words, true)),
            "A726EE1874CFAE891E32355E2EBA8C688A121F3F9D63DFF61D7722B900770BBD"
        );
    }
}
