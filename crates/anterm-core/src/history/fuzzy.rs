//! Fast, allocation-reusing fuzzy subsequence matcher for command history.
//!
//! Optimized for interactive Ctrl+R filtering over 10,000+ entries:
//! 1. **Zero-allocation fast subsequence prefilter** rejects ~95%+ of
//!    non-matching lines before touching DP buffers.
//! 2. **Reusable scratch buffers** inside [`FuzzyMatcher`] eliminate per-line
//!    heap allocations across an entire search pass.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i32,
    pub positions: Vec<usize>,
}

const SCORE_MATCH: i32 = 16;
const BONUS_BOUNDARY: i32 = 12;
const BONUS_CONSECUTIVE: i32 = 10;
const PENALTY_GAP_START: i32 = -3;
const PENALTY_GAP_EXTEND: i32 = -1;

const NEG: i32 = i32::MIN / 2;

/// Standalone helper for one-off matching. Prefer [`FuzzyMatcher`] when
/// scanning many lines in a loop to reuse scratch allocations.
pub fn match_line(line: &str, query: &str) -> Option<FuzzyMatch> {
    let mut matcher = FuzzyMatcher::default();
    matcher.match_line(line, query)
}

/// Reusable fuzzy matcher that keeps scratch buffers across lines so a full
/// Ctrl+R scan performs near-zero heap allocations.
#[derive(Default)]
pub struct FuzzyMatcher {
    hay: Vec<char>,
    hay_lc: Vec<char>,
    bonus: Vec<i32>,
    dp_score: Vec<i32>,
    dp_parent: Vec<usize>,
    pos_buf: Vec<usize>,
    matched_mask: Vec<bool>,
}

/// Prepared query terms (already lowercased) so a search pass does not re-split
/// or re-lowercase the query for every history line.
pub struct PreparedQuery {
    terms: Vec<Vec<char>>,
    ascii_only: bool,
}

impl PreparedQuery {
    pub fn new(query: &str) -> Option<Self> {
        let terms: Vec<Vec<char>> = query
            .split_whitespace()
            .map(|t| t.chars().map(lc).collect())
            .collect();
        if terms.is_empty() {
            return None;
        }
        let ascii_only = terms.iter().all(|t| t.iter().all(char::is_ascii));
        Some(Self { terms, ascii_only })
    }
}

impl FuzzyMatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn match_line(&mut self, line: &str, query: &str) -> Option<FuzzyMatch> {
        let prepared = PreparedQuery::new(query)?;
        self.match_prepared(line, &prepared)
    }

    pub fn match_prepared(&mut self, line: &str, query: &PreparedQuery) -> Option<FuzzyMatch> {
        // Fast zero-allocation prefilter: reject lines that cannot possibly
        // contain every query term as a subsequence before building char arrays
        // or running DP.
        if query.ascii_only && line.is_ascii() {
            let bytes = line.as_bytes();
            for term in &query.terms {
                if !ascii_subsequence(bytes, term) {
                    return None;
                }
            }
        } else {
            for term in &query.terms {
                if !unicode_subsequence(line, term) {
                    return None;
                }
            }
        }

        self.hay.clear();
        self.hay.extend(line.chars());
        let n = self.hay.len();

        self.hay_lc.clear();
        self.hay_lc.extend(self.hay.iter().map(|&c| lc(c)));

        self.bonus.clear();
        for j in 0..n {
            let prev = if j == 0 { None } else { Some(self.hay[j - 1]) };
            self.bonus.push(char_bonus(prev));
        }

        self.matched_mask.clear();
        self.matched_mask.resize(n, false);

        let mut total_score = 0;
        for term in &query.terms {
            let s = self.match_term(term)?;
            total_score += s;
            for &pos in &self.pos_buf {
                self.matched_mask[pos] = true;
            }
        }

        let positions: Vec<usize> = self
            .matched_mask
            .iter()
            .enumerate()
            .filter_map(|(i, &hit)| hit.then_some(i))
            .collect();

        Some(FuzzyMatch {
            score: total_score,
            positions,
        })
    }

    fn match_term(&mut self, term: &[char]) -> Option<i32> {
        let m = term.len();
        let n = self.hay_lc.len();
        if m == 0 || m > n {
            return None;
        }
        let cells = m * n;
        self.dp_score.clear();
        self.dp_score.resize(cells, NEG);
        self.dp_parent.clear();
        self.dp_parent.resize(cells, usize::MAX);

        for j in 0..n {
            if self.hay_lc[j] == term[0] {
                self.dp_score[j] = SCORE_MATCH + self.bonus[j];
            }
        }
        for i in 1..m {
            let mut gap_best = NEG;
            let mut gap_arg = usize::MAX;
            let row = i * n;
            let prev_row = (i - 1) * n;
            for j in 0..n {
                if j >= 2 {
                    let fresh = self.dp_score[prev_row + (j - 2)];
                    let fresh = if fresh > NEG {
                        fresh + PENALTY_GAP_START
                    } else {
                        NEG
                    };
                    let extended = if gap_best > NEG {
                        gap_best + PENALTY_GAP_EXTEND
                    } else {
                        NEG
                    };
                    if fresh >= extended {
                        gap_best = fresh;
                        gap_arg = j - 2;
                    } else {
                        gap_best = extended;
                    }
                }
                if self.hay_lc[j] != term[i] {
                    continue;
                }
                let cons = if j >= 1 && self.dp_score[prev_row + (j - 1)] > NEG {
                    self.dp_score[prev_row + (j - 1)] + BONUS_CONSECUTIVE
                } else {
                    NEG
                };
                let (prev, arg) = if cons >= gap_best {
                    (cons, j.wrapping_sub(1))
                } else {
                    (gap_best, gap_arg)
                };
                if prev > NEG {
                    self.dp_score[row + j] = prev + SCORE_MATCH + self.bonus[j];
                    self.dp_parent[row + j] = arg;
                }
            }
        }

        let last_row = (m - 1) * n;
        let (mut best_j, mut best) = (usize::MAX, NEG);
        for j in 0..n {
            if self.dp_score[last_row + j] > best {
                best = self.dp_score[last_row + j];
                best_j = j;
            }
        }
        if best <= NEG {
            return None;
        }
        self.pos_buf.clear();
        self.pos_buf.resize(m, 0);
        let mut j = best_j;
        for i in (0..m).rev() {
            self.pos_buf[i] = j;
            j = self.dp_parent[i * n + j];
        }
        Some(best)
    }
}

#[inline]
fn ascii_subsequence(hay: &[u8], term: &[char]) -> bool {
    let mut ti = 0;
    let m = term.len();
    if m == 0 || m > hay.len() {
        return m == 0;
    }
    let mut target = term[0] as u8;
    for &b in hay {
        if b.to_ascii_lowercase() == target {
            ti += 1;
            if ti == m {
                return true;
            }
            target = term[ti] as u8;
        }
    }
    false
}

#[inline]
fn unicode_subsequence(hay: &str, term: &[char]) -> bool {
    let mut ti = 0;
    let m = term.len();
    if m == 0 {
        return true;
    }
    let mut target = term[0];
    for c in hay.chars() {
        if lc(c) == target {
            ti += 1;
            if ti == m {
                return true;
            }
            target = term[ti];
        }
    }
    false
}

#[inline]
fn lc(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

#[inline]
fn char_bonus(prev: Option<char>) -> i32 {
    match prev {
        None => BONUS_BOUNDARY,
        Some(c)
            if c.is_whitespace() || matches!(c, '/' | '-' | '_' | '.' | ':' | '=' | ',' | '\\') =>
        {
            BONUS_BOUNDARY
        }
        _ => 0,
    }
}
