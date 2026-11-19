//! Opt-in whole-block alignment over the existing v1 typed normalization.
//!
//! Candidate prefixes are a necessary overlap filter, never match evidence.
//! Confirmation uses insertion/deletion distance (a replacement costs two).
//! No persistent state or v1 query budget is changed by this module.

mod layout;

use super::{
    BlockKey, CloneIndexError, CloneIndexSettings, CloneMatchLocation, CloneTokenKind,
    CloneSourceSpan, IndexedBlock, RULE_VERSION, TokenCloneIndex, block_identity, ordinal_cmp,
};
use std::collections::{BTreeMap, BTreeSet};

pub const NEAR_MISS_RULE_VERSION: &str = "sg10.token-near-misses.v1";
pub const PYTHON_NEAR_MISS_RULE_VERSION: &str = "sg10.python-layout-near-misses.v1";

/// Existing provider facts, kept separate from substantive token text/scoring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenLayout {
    pub indent_depth: u32,
    pub starts_logical_line: bool,
}

/// Evidence for exactly one indexed source version and whole-block span.
/// The caller remains responsible for the parser and original-byte provenance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NearMissLayout {
    pub document_version: String,
    pub span: CloneSourceSpan,
    pub tokens: Vec<TokenLayout>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NearMissOptions {
    /// Integer percentage in 1..=100 of 2 * LCS / (left length + right length).
    pub minimum_similarity_percent: u8,
    /// Maximum insertions plus deletions; replacements cost two, not one.
    pub maximum_edits: usize,
}

impl Default for NearMissOptions {
    fn default() -> Self {
        Self {
            minimum_similarity_percent: 80,
            maximum_edits: 64,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NearMissLimits {
    pub maximum_blocks: usize,
    pub maximum_tokens: usize,
    /// Sum of exact UTF-8 token text lengths, including repeated occurrences.
    pub maximum_token_bytes: usize,
    /// Per-block UTF-8 bytes of document ID, version and block ID combined.
    /// Bounds key comparisons and the owned strings copied into each result.
    pub maximum_metadata_bytes_per_block: usize,
    pub maximum_postings_per_token: usize,
    /// Counts every unordered pair visited in a posting, including duplicates.
    pub maximum_posting_visits: usize,
    pub maximum_candidate_pairs: usize,
    /// Includes boundary cells and initialization of each active DP row.
    /// Layout queries share this cap across lexical, logical-line and token DPs.
    pub maximum_alignment_cells: u64,
    pub maximum_output_pairs: usize,
}

impl Default for NearMissLimits {
    fn default() -> Self {
        Self {
            maximum_blocks: 4096,
            maximum_tokens: 262_144,
            maximum_token_bytes: 16 * 1024 * 1024,
            maximum_metadata_bytes_per_block: 16 * 1024,
            maximum_postings_per_token: 64,
            maximum_posting_visits: 100_000,
            maximum_candidate_pairs: 20_000,
            maximum_alignment_cells: 2_000_000,
            maximum_output_pairs: 1000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NearMissGapReason {
    OversizedBlock,
    MetadataByteLimit,
    BlockLimit,
    TokenLimit,
    TokenByteLimit,
    BoilerplatePostingLimit,
    PostingVisitLimit,
    CandidatePairLimit,
    AlignmentCellLimit,
    OutputPairLimit,
    UnsupportedLayout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NearMissGap {
    pub reason: NearMissGapReason,
    /// Exact block/bucket count for OversizedBlock, MetadataByteLimit, BlockLimit,
    /// BoilerplatePostingLimit and UnsupportedLayout;
    /// otherwise one encountered frontier, not the total number of missed pairs.
    pub affected_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NearMissUnsupported {
    SemanticEquivalence,
    ProviderClassificationValidation,
    RenamingInvariantAcrossEdits,
    SubBlockSourceLocations,
    LanguageOrContextPartitioning,
    SuiteInsertionOrDeletion,
    MixedLogicalLineAndTokenEdits,
    HistoricalStatementIdentity,
    InlineOrUnqualifiedStatementLayout,
}

const UNSUPPORTED: &[NearMissUnsupported] = &[
    NearMissUnsupported::SemanticEquivalence,
    NearMissUnsupported::ProviderClassificationValidation,
    NearMissUnsupported::RenamingInvariantAcrossEdits,
    NearMissUnsupported::SubBlockSourceLocations,
    NearMissUnsupported::LanguageOrContextPartitioning,
];

const LAYOUT_UNSUPPORTED: &[NearMissUnsupported] = &[
    NearMissUnsupported::SemanticEquivalence,
    NearMissUnsupported::ProviderClassificationValidation,
    NearMissUnsupported::RenamingInvariantAcrossEdits,
    NearMissUnsupported::SubBlockSourceLocations,
    NearMissUnsupported::LanguageOrContextPartitioning,
    NearMissUnsupported::SuiteInsertionOrDeletion,
    NearMissUnsupported::MixedLogicalLineAndTokenEdits,
    NearMissUnsupported::HistoricalStatementIdentity,
    NearMissUnsupported::InlineOrUnqualifiedStatementLayout,
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NearMissStatistics {
    pub document_count: usize,
    pub indexed_block_count: usize,
    pub generated_documents: usize,
    pub excluded_documents: usize,
    pub below_minimum_blocks: usize,
    pub oversized_blocks: usize,
    pub metadata_limited_blocks: usize,
    pub unsupported_layout_blocks: usize,
    pub admitted_blocks: usize,
    pub preprocessed_tokens: usize,
    pub preprocessed_token_bytes: usize,
    pub suppressed_posting_buckets: usize,
    pub posting_visits: usize,
    pub candidate_pairs: usize,
    pub overlapping_pairs_skipped: usize,
    pub length_rejected_pairs: usize,
    pub distance_rejected_pairs: usize,
    pub layout_rejected_pairs: usize,
    pub v1_pairs_skipped: usize,
    pub verified_pairs: usize,
    pub alignment_cells: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NearMissPair {
    /// Separate rule namespace; endpoint framing still uses UTF-16 lengths.
    pub pair_id: String,
    pub left: CloneMatchLocation,
    pub right: CloneMatchLocation,
    pub edit_distance: usize,
    pub normalized_matched_tokens: usize,
    /// Greatest exact count among admitted minimum-distance normalized alignments.
    /// For the layout query these must also obey its logical-line/suite relation.
    pub exact_matched_tokens: usize,
    pub score: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NearMissQueryResult {
    pub rule_version: &'static str,
    pub normalization_rule_version: &'static str,
    pub pairs: Vec<NearMissPair>,
    pub gaps: Vec<NearMissGap>,
    pub unsupported: &'static [NearMissUnsupported],
    pub statistics: NearMissStatistics,
    pub index_settings: CloneIndexSettings,
    pub applied_options: NearMissOptions,
    pub applied_limits: NearMissLimits,
}

impl NearMissQueryResult {
    #[must_use]
    pub fn is_truncated(&self) -> bool {
        !self.gaps.is_empty()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Atom<'a> {
    Local(u32),
    Text(u8, &'a str),
}

#[derive(Clone, Copy)]
struct TokenIds {
    normalized: usize,
    exact: usize,
}

struct Prepared<'a> {
    key: &'a BlockKey,
    block: &'a IndexedBlock,
    sequence: Vec<TokenIds>,
    prefix: Vec<usize>,
    layout: Option<layout::Layout>,
}

#[derive(Default)]
struct Posting {
    blocks: Vec<usize>,
    suppressed: bool,
}

fn intern<'a>(atoms: &mut BTreeMap<Atom<'a>, usize>, atom: Atom<'a>) -> usize {
    let next = atoms.len();
    *atoms.entry(atom).or_insert(next)
}

fn gap(gaps: &mut Vec<NearMissGap>, reason: NearMissGapReason, affected_count: usize) {
    gaps.push(NearMissGap {
        reason,
        affected_count,
    });
}

impl TokenCloneIndex {
    /// Read-only, separate opt-in query. Distance-zero pairs remain v1's domain,
    /// even if a v1 posting cap would hide them. Same-document overlapping spans
    /// are excluded; disjoint blocks in the same document remain eligible.
    /// Callers must partition by language/context and supply trustworthy kinds.
    pub fn query_near_misses(
        &self,
        options: NearMissOptions,
        limits: NearMissLimits,
    ) -> Result<NearMissQueryResult, CloneIndexError> {
        self.query_near_misses_impl(options, limits, None)
    }

    /// Opt-in Python layout relation over the same candidate and token budgets.
    /// Lookup runs once per size-admitted block, after token/byte admission. Missing,
    /// stale or malformed evidence fails the query; it never falls back to v1.
    /// Whole logical-line gaps require a pure insertion/deletion relation;
    /// otherwise edits must stay within corresponding lines. Suite-opening
    /// ancestry/order is fixed. Layout may not add edits to the lexical optimum.
    /// Inline suites, semicolon statements and unqualified colon-bearing lines
    /// withhold the block with an UnsupportedLayout coverage gap.
    /// An arbitrary caller-supplied lookup must itself be bounded and trustworthy.
    pub fn query_near_misses_with_layout(
        &self,
        options: NearMissOptions,
        limits: NearMissLimits,
        mut layout_for: impl FnMut(&CloneMatchLocation) -> Option<NearMissLayout>,
    ) -> Result<NearMissQueryResult, CloneIndexError> {
        self.query_near_misses_impl(options, limits, Some(&mut layout_for))
    }

    fn query_near_misses_impl(
        &self,
        options: NearMissOptions,
        limits: NearMissLimits,
        mut layout_for: Option<&mut dyn FnMut(&CloneMatchLocation) -> Option<NearMissLayout>>,
    ) -> Result<NearMissQueryResult, CloneIndexError> {
        let layout_enabled = layout_for.is_some();
        let rule_version = if layout_enabled {
            PYTHON_NEAR_MISS_RULE_VERSION
        } else {
            NEAR_MISS_RULE_VERSION
        };
        if !(1..=100).contains(&options.minimum_similarity_percent) || options.maximum_edits == 0 {
            return Err(CloneIndexError::InvalidSettings);
        }
        if limits.maximum_blocks == 0
            || limits.maximum_tokens == 0
            || limits.maximum_token_bytes == 0
            || limits.maximum_metadata_bytes_per_block == 0
            || limits.maximum_postings_per_token == 0
            || limits.maximum_posting_visits == 0
            || limits.maximum_candidate_pairs == 0
            || limits.maximum_alignment_cells == 0
            || limits.maximum_output_pairs == 0
        {
            return Err(CloneIndexError::InvalidLimits);
        }
        let mut statistics = NearMissStatistics {
            document_count: self.documents.len(),
            indexed_block_count: self.blocks.len(),
            ..NearMissStatistics::default()
        };
        let mut gaps = Vec::new();
        for document in self.documents.values() {
            if document.excluded {
                statistics.excluded_documents += 1;
            } else if document.generated {
                statistics.generated_documents += 1;
            } else {
                for block in &document.blocks {
                    if block.token_count < self.settings.minimum_tokens {
                        statistics.below_minimum_blocks += 1;
                    } else if block.token_count > self.settings.maximum_tokens_per_block {
                        statistics.oversized_blocks += 1;
                    }
                }
            }
        }
        if statistics.oversized_blocks > 0 {
            gap(
                &mut gaps,
                NearMissGapReason::OversizedBlock,
                statistics.oversized_blocks,
            );
        }

        // Scan the catalog, retaining at most maximum_blocks + 1 key references.
        // BlockKey supplies .NET UTF-16 ordinal order, independent of HashMap order.
        let mut selected = BTreeSet::new();
        for (key, block) in &self.blocks {
            let location = &block.location;
            let metadata_bytes = location
                .document_id
                .len()
                .saturating_add(location.document_version.len())
                .saturating_add(location.block_id.len());
            if metadata_bytes > limits.maximum_metadata_bytes_per_block {
                statistics.metadata_limited_blocks += 1;
                continue;
            }
            selected.insert(key);
            if selected.len() > limits.maximum_blocks {
                selected.pop_last();
            }
        }
        if statistics.metadata_limited_blocks > 0 {
            gap(
                &mut gaps,
                NearMissGapReason::MetadataByteLimit,
                statistics.metadata_limited_blocks,
            );
        }
        let unselected = self.blocks.len() - statistics.metadata_limited_blocks - selected.len();
        if unselected > 0 {
            gap(&mut gaps, NearMissGapReason::BlockLimit, unselected);
        }
        let mut atoms = BTreeMap::new();
        // Exact interning makes suite-shape comparison constant work per pair.
        // No hashes or absolute logical-line ordinals are used as evidence.
        let mut layout_shapes = BTreeMap::new();
        let mut prepared = Vec::new();
        for key in selected {
            let block = &self.blocks[key];
            if block.exact.len() > limits.maximum_tokens - statistics.preprocessed_tokens {
                gap(&mut gaps, NearMissGapReason::TokenLimit, 1);
                break;
            }
            let remaining_bytes = limits.maximum_token_bytes - statistics.preprocessed_token_bytes;
            let mut bytes = 0;
            let mut fits = true;
            for token in &block.exact {
                let size = block.words[token.word as usize].len();
                if size > remaining_bytes - bytes {
                    fits = false;
                    break;
                }
                bytes += size;
            }
            if !fits {
                gap(&mut gaps, NearMissGapReason::TokenByteLimit, 1);
                break;
            }
            // Even a structurally unavailable block consumes preprocessing
            // budgets. Otherwise repeated unsupported layouts could bypass T.
            statistics.preprocessed_tokens += block.exact.len();
            statistics.preprocessed_token_bytes += bytes;
            let layout = if let Some(lookup) = layout_for.as_mut() {
                let evidence = lookup(&block.location).ok_or_else(|| {
                    CloneIndexError::InvalidLayout("missing admitted block layout".into())
                })?;
                if evidence.document_version != block.location.document_version {
                    return Err(CloneIndexError::InvalidLayout("stale document version".into()));
                }
                if evidence.span != block.location.span || evidence.tokens.len() != block.exact.len() {
                    return Err(CloneIndexError::InvalidLayout("layout span or token count mismatch".into()));
                }
                let Some(layout) = layout::prepare(&evidence.tokens, block, &mut layout_shapes)? else {
                    statistics.unsupported_layout_blocks += 1;
                    continue;
                };
                Some(layout)
            } else {
                None
            };
            // All token-text work follows whole-block token/byte admission.
            let mut sequence = Vec::with_capacity(block.exact.len());
            for (exact, normalized) in block.exact.iter().zip(&block.normalized) {
                let exact_atom = Atom::Text(exact.kind as u8, &block.words[exact.word as usize]);
                let normalized_atom = if normalized.kind == CloneTokenKind::LocalIdentifier {
                    Atom::Local(normalized.word)
                } else {
                    exact_atom
                };
                sequence.push(TokenIds {
                    normalized: intern(&mut atoms, normalized_atom),
                    exact: intern(&mut atoms, exact_atom),
                });
            }
            let mut prefix: Vec<_> = sequence.iter().map(|token| token.normalized).collect();
            prefix.sort_unstable();
            prepared.push(Prepared {
                key,
                block,
                sequence,
                prefix,
                layout,
            });
        }
        statistics.admitted_blocks = prepared.len();
        if statistics.unsupported_layout_blocks > 0 {
            gap(&mut gaps, NearMissGapReason::UnsupportedLayout, statistics.unsupported_layout_blocks);
        }
        // Rank rare atoms first within this admitted snapshot. Frequencies are
        // rebuilt on every query; no old/new token orders can become mixed.
        let mut frequency = vec![0usize; atoms.len()];
        for block in &prepared {
            let mut previous = None;
            for atom in &block.prefix {
                if previous != Some(*atom) {
                    frequency[*atom] += 1;
                    previous = Some(*atom);
                }
            }
        }
        for block in &mut prepared {
            let m = block.sequence.len();
            let theta = u64::from(options.minimum_similarity_percent);
            let required_by_score = ((theta * m as u64).div_ceil(200 - theta)) as usize;
            let required = m.saturating_sub(options.maximum_edits).max(required_by_score);
            // Preserve multiplicity while choosing the prefix, then deduplicate
            // postings. The (frequency, atom ID) order is shared by all blocks.
            block.prefix.sort_unstable_by_key(|atom| (frequency[*atom], *atom));
            block.prefix.truncate(m - required + 1);
            block.prefix.dedup();
        }
        // No token-text comparison or hashing is needed beyond this point.
        drop(atoms);
        drop(layout_shapes);

        let mut postings: BTreeMap<(usize, usize), Posting> = BTreeMap::new();
        for (number, block) in prepared.iter().enumerate() {
            for atom in &block.prefix {
                let posting = postings.entry((frequency[*atom], *atom)).or_default();
                if posting.suppressed {
                    continue;
                }
                if posting.blocks.len() == limits.maximum_postings_per_token {
                    posting.blocks.clear();
                    posting.suppressed = true;
                    statistics.suppressed_posting_buckets += 1;
                } else {
                    posting.blocks.push(number);
                }
            }
        }
        drop(frequency);
        if statistics.suppressed_posting_buckets > 0 {
            gap(
                &mut gaps,
                NearMissGapReason::BoilerplatePostingLimit,
                statistics.suppressed_posting_buckets,
            );
        }

        let mut candidates = BTreeSet::new();
        'postings: for posting in postings.values() {
            for (offset, left) in posting.blocks.iter().enumerate() {
                for right in &posting.blocks[offset + 1..] {
                    if statistics.posting_visits == limits.maximum_posting_visits {
                        gap(&mut gaps, NearMissGapReason::PostingVisitLimit, 1);
                        break 'postings;
                    }
                    statistics.posting_visits += 1;
                    let pair = (*left, *right);
                    if candidates.contains(&pair) {
                        continue;
                    }
                    if candidates.len() == limits.maximum_candidate_pairs {
                        gap(&mut gaps, NearMissGapReason::CandidatePairLimit, 1);
                        break 'postings;
                    }
                    candidates.insert(pair);
                }
            }
        }
        statistics.candidate_pairs = candidates.len();
        drop(postings);

        let mut pairs = Vec::new();
        for (left_index, right_index) in candidates {
            let left = &prepared[left_index];
            let right = &prepared[right_index];
            let a = &left.block.location;
            let b = &right.block.location;
            if left.key.doc == right.key.doc
                && a.span.start_offset < b.span.end_offset
                && b.span.start_offset < a.span.end_offset
            {
                statistics.overlapping_pairs_skipped += 1;
                continue;
            }
            let total = left.sequence.len() + right.sequence.len();
            let cutoff = options.maximum_edits.min(
                (u64::from(100 - options.minimum_similarity_percent) * total as u64 / 100) as usize,
            );
            if left.sequence.len().abs_diff(right.sequence.len()) > cutoff {
                statistics.length_rejected_pairs += 1;
                continue;
            }
            let aligned = align(
                &left.sequence,
                &right.sequence,
                cutoff,
                limits.maximum_alignment_cells,
                &mut statistics.alignment_cells,
            );
            let mut cell = match aligned {
                Alignment::Exhausted => {
                    gap(&mut gaps, NearMissGapReason::AlignmentCellLimit, 1);
                    break;
                }
                Alignment::OverDistance => {
                    statistics.verified_pairs += 1;
                    statistics.distance_rejected_pairs += 1;
                    continue;
                }
                Alignment::Within(cell) => cell,
            };
            if cell.distance == 0 {
                statistics.verified_pairs += 1;
                statistics.v1_pairs_skipped += 1;
                continue;
            }
            if let (Some(left_layout), Some(right_layout)) = (&left.layout, &right.layout) {
                match layout::align_layout(
                    &left.sequence,
                    &right.sequence,
                    left_layout,
                    right_layout,
                    cell.distance,
                    limits.maximum_alignment_cells,
                    &mut statistics.alignment_cells,
                ) {
                    Alignment::Exhausted => {
                        gap(&mut gaps, NearMissGapReason::AlignmentCellLimit, 1);
                        break;
                    }
                    Alignment::OverDistance => {
                        statistics.verified_pairs += 1;
                        statistics.layout_rejected_pairs += 1;
                        continue;
                    }
                    Alignment::Within(qualified) => cell = qualified,
                }
            }
            statistics.verified_pairs += 1;
            if pairs.len() == limits.maximum_output_pairs {
                gap(&mut gaps, NearMissGapReason::OutputPairLimit, 1);
                break;
            }
            let matched = (total - cell.distance) / 2;
            pairs.push(NearMissPair {
                pair_id: format!(
                    "{rule_version}:{}/{}",
                    block_identity(left.key),
                    block_identity(right.key),
                ),
                left: a.clone(),
                right: b.clone(),
                edit_distance: cell.distance,
                normalized_matched_tokens: matched,
                exact_matched_tokens: cell.exact,
                score: 2.0 * matched as f64 / total as f64,
            });
        }
        pairs.sort_by(|a, b| {
            let a_total = a.left.token_count as u128 + a.right.token_count as u128;
            let b_total = b.left.token_count as u128 + b.right.token_count as u128;
            (b.normalized_matched_tokens as u128 * a_total)
                .cmp(&(a.normalized_matched_tokens as u128 * b_total))
                .then_with(|| b_total.cmp(&a_total))
                .then_with(|| ordinal_cmp(&a.pair_id, &b.pair_id))
        });
        gaps.sort_by_key(|item| item.reason);
        Ok(NearMissQueryResult {
            rule_version,
            normalization_rule_version: RULE_VERSION,
            pairs,
            gaps,
            unsupported: if layout_enabled { LAYOUT_UNSUPPORTED } else { UNSUPPORTED },
            statistics,
            index_settings: self.settings,
            applied_options: options,
            applied_limits: limits,
        })
    }
}

#[derive(Clone, Copy)]
struct Cell {
    distance: usize,
    exact: usize,
}

enum Alignment {
    Within(Cell),
    OverDistance,
    Exhausted,
}

fn at(row: &[Cell], start: usize, column: usize, unreachable: Cell) -> Cell {
    column
        .checked_sub(start)
        .and_then(|offset| row.get(offset))
        .copied()
        .unwrap_or(unreachable)
}

fn better(a: Cell, b: Cell) -> Cell {
    if b.distance < a.distance || (b.distance == a.distance && b.exact > a.exact) {
        b
    } else {
        a
    }
}

fn charge(cells: usize, maximum: u64, charged: &mut u64) -> bool {
    if cells as u64 > maximum - *charged {
        false
    } else {
        *charged += cells as u64;
        true
    }
}

fn align(
    left: &[TokenIds],
    right: &[TokenIds],
    cutoff: usize,
    maximum: u64,
    charged: &mut u64,
) -> Alignment {
    let unreachable = Cell {
        distance: cutoff + 1,
        exact: 0,
    };
    let first_width = right.len().min(cutoff) + 1;
    if !charge(first_width, maximum, charged) {
        return Alignment::Exhausted;
    }
    let mut previous: Vec<_> = (0..first_width)
        .map(|j| Cell { distance: j, exact: 0 })
        .collect();
    let mut previous_start = 0;
    let mut current = Vec::new();
    for i in 1..=left.len() {
        let start = i.saturating_sub(cutoff);
        let end = right.len().min(i.saturating_add(cutoff));
        if start > end {
            return Alignment::OverDistance;
        }
        let width = end - start + 1;
        if !charge(width, maximum, charged) {
            return Alignment::Exhausted;
        }
        // Compact active rows: no full-length row clearing outside the band.
        current.clear();
        current.resize(width, unreachable);
        let mut reachable = false;
        for j in start..=end {
            let mut best = unreachable;
            let up = at(&previous, previous_start, j, unreachable);
            if up.distance < cutoff {
                best = better(
                    best,
                    Cell { distance: up.distance + 1, exact: up.exact },
                );
            }
            if j > 0 {
                let across = at(&current, start, j - 1, unreachable);
                if across.distance < cutoff {
                    best = better(
                        best,
                        Cell { distance: across.distance + 1, exact: across.exact },
                    );
                }
                if left[i - 1].normalized == right[j - 1].normalized {
                    let diagonal = at(&previous, previous_start, j - 1, unreachable);
                    if diagonal.distance <= cutoff {
                        best = better(
                            best,
                            Cell {
                                distance: diagonal.distance,
                                exact: diagonal.exact
                                    + usize::from(left[i - 1].exact == right[j - 1].exact),
                            },
                        );
                    }
                }
            }
            current[j - start] = best;
            reachable |= best.distance <= cutoff;
        }
        if !reachable {
            return Alignment::OverDistance;
        }
        std::mem::swap(&mut previous, &mut current);
        previous_start = start;
    }
    let last = at(&previous, previous_start, right.len(), unreachable);
    if last.distance <= cutoff {
        Alignment::Within(last)
    } else {
        Alignment::OverDistance
    }
}
