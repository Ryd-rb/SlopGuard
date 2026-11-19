//! A deliberately restricted relation over existing Python layout facts.
//!
//! Suite IDs are preorder IDs of INDENT openings, independent of line ordinals.
//! A line's owner and the child it opens must correspond. Whole-line gaps are
//! permitted only for pure insertions/deletions, and never for an opening line.
//! All other edits remain within paired lines. The accepted distance must equal
//! the lexical optimum: layout cannot be erased by paying extra token edits.

use super::{Alignment, Cell, CloneIndexError, IndexedBlock, TokenIds, TokenLayout, align, at, better, charge};
use std::collections::BTreeMap;

struct Line {
    start: usize,
    end: usize,
    owner: usize,
    opens: Option<usize>,
}

impl Line {
    fn len(&self) -> usize {
        self.end - self.start
    }
}

pub(super) struct Layout {
    lines: Vec<Line>,
    shape: usize,
}

pub(super) fn prepare(
    tokens: &[TokenLayout],
    block: &IndexedBlock,
    shapes: &mut BTreeMap<Vec<usize>, usize>,
) -> Result<Option<Layout>, CloneIndexError> {
    if !tokens.first().is_some_and(|token| token.indent_depth == 0 && token.starts_logical_line) {
        return Err(CloneIndexError::InvalidLayout("block must start at a relative logical line".into()));
    }
    let mut lines: Vec<Line> = Vec::new();
    let mut stack = vec![0usize];
    let mut parents = Vec::new();
    for (offset, token) in tokens.iter().enumerate() {
        let depth = usize::try_from(token.indent_depth)
            .map_err(|_| CloneIndexError::InvalidLayout("indentation depth overflow".into()))?;
        if token.starts_logical_line {
            if depth > stack.len() - 1 {
                // Each INDENT opens exactly one level. Validate before allocating
                // from the supplied depth, so a malformed large depth is cheap.
                if depth != stack.len() {
                    return Err(CloneIndexError::InvalidLayout("indentation skips a level".into()));
                }
                let owner = stack[stack.len() - 1];
                let child = parents.len() + 1;
                parents.push(owner);
                let previous = lines.last_mut().ok_or_else(|| {
                    CloneIndexError::InvalidLayout("suite has no opening line".into())
                })?;
                previous.opens = Some(child);
                stack.push(child);
            } else {
                stack.truncate(depth + 1);
            }
            if let Some(previous) = lines.last_mut() {
                previous.end = offset;
            }
            lines.push(Line {
                start: offset,
                end: tokens.len(),
                owner: stack[stack.len() - 1],
                opens: None,
            });
        } else if depth != stack.len() - 1 {
            return Err(CloneIndexError::InvalidLayout("indentation changes inside a logical line".into()));
        }
    }
    // INDENT facts cannot establish ownership inside an inline compound suite.
    // Nor do they delimit semicolon-separated simple statements. Withhold any
    // block containing such unqualified punctuation instead of guessing syntax.
    // Colons are allowed only in a complete header that demonstrably opens an
    // indentation suite and ends in ':'. This also allows annotated parameters
    // in that header. Other colons (annotations, dicts, slices, lambdas, etc.)
    // are conservative unavailable cases, not claims of invalid Python syntax.
    for line in &lines {
        let text = |offset: usize| block.words[block.exact[offset].word as usize].as_ref();
        if line.opens.is_some() && text(line.end - 1) != ":" {
            return Ok(None);
        }
        for offset in line.start..line.end {
            if text(offset) == ";" || (text(offset) == ":" && line.opens.is_none()) {
                return Ok(None);
            }
        }
    }
    let next_shape = shapes.len();
    let shape = *shapes.entry(parents).or_insert(next_shape);
    Ok(Some(Layout { lines, shape }))
}

fn extend_gap(cell: Cell, line: &Line, cutoff: usize, unreachable: Cell) -> Cell {
    if line.opens.is_none() && cell.distance <= cutoff && line.len() <= cutoff - cell.distance {
        Cell { distance: cell.distance + line.len(), exact: cell.exact }
    } else {
        unreachable
    }
}

pub(super) fn align_layout(
    left: &[TokenIds],
    right: &[TokenIds],
    left_layout: &Layout,
    right_layout: &Layout,
    lexical_distance: usize,
    maximum: u64,
    charged: &mut u64,
) -> Alignment {
    if left_layout.shape != right_layout.shape {
        return Alignment::OverDistance;
    }
    // If both token insertion and deletion are needed, no line may disappear
    // or appear. This deliberately rejects replacement-plus-new-line edits.
    // For a pure insertion/deletion every shorter-side token must survive in
    // an accepting path, so a changed boundary cannot hide in an edited gap.
    let pure = lexical_distance == left.len().abs_diff(right.len());
    let insert_lines = pure && right.len() > left.len();
    let delete_lines = pure && left.len() > right.len();
    let a = &left_layout.lines;
    let b = &right_layout.lines;
    if !pure && a.len() != b.len() {
        return Alignment::OverDistance;
    }
    let unreachable = Cell { distance: lexical_distance + 1, exact: 0 };
    let mut previous = Vec::new();
    let mut previous_start = 0;
    let mut current = Vec::new();
    for i in 0..=a.len() {
        // A skipped line has at least one substantive token. Therefore no
        // accepted path can leave this line-count band. Fixed-line edits use
        // just the diagonal; pure edits use only the permitted gap direction.
        let start = if delete_lines { i.saturating_sub(lexical_distance) } else { i };
        let end = b.len().min(if insert_lines {
            i.saturating_add(lexical_distance)
        } else {
            i
        });
        if start > end {
            return Alignment::OverDistance;
        }
        let width = end - start + 1;
        if !charge(width, maximum, charged) {
            return Alignment::Exhausted;
        }
        current.clear();
        current.resize(width, unreachable);
        let mut reachable = false;
        for j in start..=end {
            let mut best = if i == 0 && j == 0 {
                Cell { distance: 0, exact: 0 }
            } else {
                unreachable
            };
            if delete_lines && i > 0 {
                best = better(best, extend_gap(
                    at(&previous, previous_start, j, unreachable),
                    &a[i - 1], lexical_distance, unreachable,
                ));
            }
            if insert_lines && j > 0 {
                best = better(best, extend_gap(
                    at(&current, start, j - 1, unreachable),
                    &b[j - 1], lexical_distance, unreachable,
                ));
            }
            if i > 0 && j > 0 && a[i - 1].owner == b[j - 1].owner
                && a[i - 1].opens == b[j - 1].opens {
                let diagonal = at(&previous, previous_start, j - 1, unreachable);
                if diagonal.distance <= lexical_distance {
                    let remaining = lexical_distance - diagonal.distance;
                    if a[i - 1].len().abs_diff(b[j - 1].len()) <= remaining {
                        match align(
                            &left[a[i - 1].start..a[i - 1].end],
                            &right[b[j - 1].start..b[j - 1].end],
                            remaining, maximum, charged,
                        ) {
                            Alignment::Exhausted => return Alignment::Exhausted,
                            Alignment::OverDistance => {}
                            Alignment::Within(cell) => {
                                best = better(best, Cell {
                                    distance: diagonal.distance + cell.distance,
                                    exact: diagonal.exact + cell.exact,
                                });
                            }
                        }
                    }
                }
            }
            current[j - start] = best;
            reachable |= best.distance <= lexical_distance;
        }
        if !reachable {
            return Alignment::OverDistance;
        }
        std::mem::swap(&mut previous, &mut current);
        previous_start = start;
    }
    let last = at(&previous, previous_start, b.len(), unreachable);
    if last.distance == lexical_distance {
        Alignment::Within(last)
    } else {
        Alignment::OverDistance
    }
}
