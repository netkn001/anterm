//! How well a row answers a query. One scorer for every tab, so the All tab
//! can put two tabs' best rows side by side and have the order mean something.

use super::command::Item;

pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    let needle: Vec<char> = query
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !c.is_whitespace())
        .collect();
    if needle.is_empty() {
        return Some(0);
    }
    let (score, _) = best_alignment(&needle, text)?;
    Some(score)
}

/// Byte ranges in `text` corresponding to the best fuzzy alignment of `query`,
/// with adjacent matched characters merged into contiguous ranges for crisp
/// `StyledText` highlighting.
pub fn fuzzy_match_ranges(query: &str, text: &str) -> Vec<std::ops::Range<usize>> {
    let needle: Vec<char> = query
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !c.is_whitespace())
        .collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let Some((_, char_positions)) = best_alignment(&needle, text) else {
        return Vec::new();
    };
    let byte_spans: Vec<(usize, usize)> = text
        .char_indices()
        .map(|(start, ch)| (start, start + ch.len_utf8()))
        .collect();
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for pos in char_positions {
        let Some(&(start, end)) = byte_spans.get(pos) else {
            continue;
        };
        if let Some(last) = ranges.last_mut()
            && last.end == start
        {
            last.end = end;
        } else {
            ranges.push(start..end);
        }
    }
    ranges
}

/// Finds the highest-scoring subsequence alignment of `needle` inside `text`,
/// returning `(score, original_char_indices)`.
///
/// Uses dynamic programming over `(needle_pos, hay_pos)` so acronyms like `"st"`
/// against `"Switch Tab…"` prefer word-initial `'T'` over a mid-word `'t'`,
/// while keeping the exact per-character scoring weights.
fn best_alignment(needle: &[char], text: &str) -> Option<(i32, Vec<usize>)> {
    let mut hay: Vec<char> = Vec::new();
    let mut orig_idx: Vec<usize> = Vec::new();
    for (ci, ch) in text.chars().enumerate() {
        for lower in ch.to_lowercase() {
            hay.push(lower);
            orig_idx.push(ci);
        }
    }
    let m = needle.len();
    let n = hay.len();
    if m == 0 || m > n {
        return None;
    }

    let word_start: Vec<bool> = (0..n)
        .map(|i| i == 0 || !hay[i - 1].is_alphanumeric())
        .collect();
    let base_bonus = |i: usize| -> i32 {
        1 + if word_start[i] { 12 } else { 0 } + if i == 0 { 10 } else { 0 }
    };

    // dp[k][j] = Some((best_score, consecutive_run, prev_j)) when needle[k] matches hay[j].
    let mut dp: Vec<Vec<Option<(i32, i32, Option<usize>)>>> = vec![vec![None; n]; m];
    for j in 0..n {
        if hay[j] == needle[0] {
            dp[0][j] = Some((base_bonus(j), 0, None));
        }
    }

    for k in 1..m {
        // Track best non-adjacent match strictly before j-1.
        let mut best_prior: Option<(i32, usize)> = None;
        for j in k..n {
            if j >= 2
                && let Some((score, _, _)) = dp[k - 1][j - 2]
                && best_prior.is_none_or(|(b, _)| score > b)
            {
                best_prior = Some((score, j - 2));
            }
            if hay[j] != needle[k] {
                continue;
            }
            let base = base_bonus(j);
            let mut best_here: Option<(i32, i32, Option<usize>)> = None;

            // Adjacent continuation from j - 1.
            if let Some((prev_score, prev_run, _)) = dp[k - 1][j - 1] {
                let run = prev_run + 1;
                let cand = prev_score + base + 6 + run.min(8);
                best_here = Some((cand, run, Some(j - 1)));
            }

            // Non-adjacent jump from best prior index <= j - 2.
            if let Some((prior_score, prior_j)) = best_prior {
                let cand = prior_score + base;
                if best_here.is_none_or(|(b, _, _)| cand > b) {
                    best_here = Some((cand, 0, Some(prior_j)));
                }
            }

            dp[k][j] = best_here;
        }
    }

    let (mut cur_j, (best, _, _)) = dp[m - 1]
        .iter()
        .enumerate()
        .filter_map(|(j, entry)| entry.map(|e| (j, e)))
        .max_by_key(|(_, (score, _, _))| *score)?;

    let mut positions = vec![0usize; m];
    for k in (0..m).rev() {
        positions[k] = cur_j;
        if let Some((_, _, Some(prev))) = dp[k][cur_j] {
            cur_j = prev;
        }
    }

    let mut score = best;
    if hay == needle {
        score += 120;
    } else if hay.starts_with(needle) {
        score += 50;
    }
    score -= (n as i32) / 6;

    let mut mapped: Vec<usize> = Vec::with_capacity(positions.len());
    for pos in positions {
        let orig = orig_idx[pos];
        if mapped.last().copied() != Some(orig) {
            mapped.push(orig);
        }
    }
    Some((score, mapped))
}

/// Whether every whitespace-separated word of `query` occurs in `text`,
/// ignoring case.
fn words_appear_in(query: &str, text: &str) -> bool {
    let text = text.to_lowercase();
    query
        .split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
}

/// How far behind the visible label an alias hit lands. An alias *is* the
/// command's own name, only in another language, so the gap is small — but two
/// commands that both match must still be ordered by the text on screen.
const ALIAS_PENALTY: i32 = 10;

pub fn item_score(query: &str, cmd: &Item) -> Option<i32> {
    let title = fuzzy_score(query, &cmd.title);
    // A subtitle is prose — a description, a host — and a query's letters
    // strewn across a sentence are no match for it: "theme" found "Git:
    // Discard All Changes" through "Throws away every uncommitted change". The
    // words typed have to be in it as typed.
    let subtitle = cmd
        .subtitle
        .as_deref()
        .filter(|s| words_appear_in(query, s))
        .and_then(|s| fuzzy_score(query, s))
        .map(|s| s / 2 - 25);
    // Aliases only ever decide whether a row is in the list and where it sits.
    // The row keeps rendering `title` untouched, so a hit on wording the user
    // cannot see can never end up underlined against the wrong characters.
    let alias = cmd
        .aliases
        .iter()
        .filter_map(|a| fuzzy_score(query, a))
        .max()
        .map(|s| s - ALIAS_PENALTY);
    [title, subtitle, alias].into_iter().flatten().max()
}

/// A bounded nudge, not a re-ranking. Two commands that match the query about
/// equally well should come out in the order they actually get run, but a
/// command used daily must not outrank a plainly better match — a single extra
/// matched character is worth 16 before bonuses.
pub fn frecency_bonus(used: f64) -> i32 {
    const CEILING: f64 = 24.0;
    if used <= 0.0 {
        return 0;
    }
    (used.ln_1p() * 8.0).min(CEILING).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use anterm_i18n::L10nKey;
    use crate::search::PaletteAction;

    #[test]
    fn frecency_nudges_without_overruling_the_match() {
        assert_eq!(frecency_bonus(0.0), 0, "an unused command gets nothing");
        assert!(frecency_bonus(1.0) > 0, "one use is worth something");
        // Monotone in usage.
        assert!(frecency_bonus(20.0) > frecency_bonus(1.0));
        // Bounded: never worth more than one and a half matched characters,
        // so a command run a thousand times still loses to a better match.
        assert!(
            frecency_bonus(1_000.0) <= 24,
            "frecency is a tiebreak, not a ranking"
        );
        assert_eq!(frecency_bonus(1_000.0), frecency_bonus(10_000.0));
    }

    #[test]
    fn a_whole_word_is_found_past_an_earlier_stray_letter() {
        // The `w` of "New" used to claim the match and scatter the rest.
        let whole = fuzzy_score("worktree", "New Worktree Tab…").expect("matches");
        let scattered = fuzzy_score("worktree", "New work on tree").expect("matches");
        assert!(whole > scattered, "{whole} should beat {scattered}");
        assert_eq!(whole, 88);
    }

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        assert!(fuzzy_match_ranges("", "anything").is_empty());
    }

    #[test]
    fn fuzzy_match_ranges_highlights_acronyms_and_utf8() {
        let title = "Switch Tab…";
        let ranges = fuzzy_match_ranges("st", title);
        let slices: Vec<&str> = ranges.iter().map(|r| &title[r.clone()]).collect();
        assert_eq!(slices, vec!["S", "T"]);

        let title = "Split Right";
        let ranges = fuzzy_match_ranges("spli", title);
        assert_eq!(ranges, vec![0..4]);

        let zh = "切换标签…";
        let ranges = fuzzy_match_ranges("切标", zh);
        let slices: Vec<&str> = ranges.iter().map(|r| &zh[r.clone()]).collect();
        assert_eq!(slices, vec!["切", "标"]);
    }

    #[test]
    fn non_subsequence_does_not_match() {
        assert_eq!(fuzzy_score("zzz", "Split Right"), None);
        assert_eq!(fuzzy_score("thgir", "Split Right"), None);
    }

    #[test]
    fn word_initials_outrank_scattered_letters() {
        let target = fuzzy_score("sr", "Split Right").expect("matches");
        let scattered = fuzzy_score("sr", "SSH: Manage Profiles…").expect("matches");
        assert!(
            target > scattered,
            "expected 'Split Right' ({target}) to outrank 'SSH: Manage Profiles…' ({scattered})"
        );
    }

    #[test]
    fn exact_and_prefix_beat_mid_string() {
        let exact = fuzzy_score("copy", "Copy").expect("matches");
        let longer = fuzzy_score("copy", "Copy Working Directory").expect("matches");
        assert!(
            exact > longer,
            "expected exact 'Copy' ({exact}) above 'Copy Working Directory' ({longer})"
        );
    }

    #[test]
    fn subtitle_matches_are_found_but_discounted() {
        let cmd = Item::new("prod-web", PaletteAction::NewTab)
            .with_subtitle("deploy@10.0.0.5".to_string());
        assert!(item_score("10.0.0", &cmd).is_some());
        let title_hit = item_score("prod", &cmd).expect("title matches");
        let subtitle_hit = item_score("deploy", &cmd).expect("subtitle matches");
        assert!(title_hit > subtitle_hit);
    }

    #[test]
    fn the_remote_files_panel_is_found_as_sftp() {
        let cmd =
            Item::localized(L10nKey::CmdSshRemoteFiles, PaletteAction::ToggleSftp).with_alias("SFTP");
        assert!(item_score("sftp", &cmd).is_some());
    }

    #[test]
    fn letters_strewn_across_a_description_do_not_match_it() {
        let cmd = Item::new("Git: Discard All Changes", PaletteAction::NewTab)
            .with_subtitle("Throws away every uncommitted change in the working tree.".to_string());
        assert!(item_score("theme", &cmd).is_none());
        assert!(item_score("uncommitted", &cmd).is_some());
        assert!(item_score("working tree", &cmd).is_some());
    }

    #[test]
    fn a_translated_command_is_still_found_by_its_english_name() {
        anterm_i18n::set_locale("zh-CN");
        let cmd = Item::localized(L10nKey::CmdChangeTheme, PaletteAction::OpenThemePicker);
        assert!(
            !cmd.title.to_lowercase().contains("theme"),
            "the row shows the Chinese label: {:?}",
            cmd.title
        );
        assert!(
            item_score("theme", &cmd).is_some(),
            "an English query must find the Chinese-labelled row"
        );
        // The displayed label still works, and still wins: the same query
        // against the locale that shows it scores higher.
        let zh = item_score("更改主题", &cmd).expect("the shown label matches");
        anterm_i18n::set_locale("en");
        let en = Item::localized(L10nKey::CmdChangeTheme, PaletteAction::OpenThemePicker);
        assert!(zh > 0 && en.title.contains("Theme"));
        assert!(
            item_score("theme", &en).unwrap() > item_score("theme", &cmd).unwrap(),
            "a hit on the visible label must outrank the same hit on an alias"
        );
    }
}
