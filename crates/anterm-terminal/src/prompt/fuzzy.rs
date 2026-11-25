//! Re-exports the allocation-reusing fuzzy matcher from `anterm-core`.

#[allow(unused_imports)]
pub use anterm_core::history::fuzzy::{FuzzyMatch, FuzzyMatcher, PreparedQuery, match_line};

#[cfg(test)]
mod tests {
    use super::*;

    fn score(line: &str, query: &str) -> i32 {
        match_line(line, query).expect("expected a match").score
    }

    fn positions(line: &str, query: &str) -> Vec<usize> {
        match_line(line, query).expect("expected a match").positions
    }

    #[test]
    fn non_subsequence_is_no_match() {
        assert!(match_line("git status", "xyz").is_none());
        assert!(match_line("ls", "lss").is_none());
        assert!(match_line("git status", "tg").is_none());
    }

    #[test]
    fn blank_query_is_no_match() {
        assert!(match_line("git status", "").is_none());
        assert!(match_line("git status", "   ").is_none());
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert_eq!(score("Git Status", "git"), score("git status", "GIT"));
        assert!(match_line("MAKE ALL", "make").is_some());
    }

    #[test]
    fn consecutive_run_beats_scattered_letters() {
        assert!(score("git log", "git") > score("going to lunch", "git"));
    }

    #[test]
    fn word_boundary_beats_mid_word() {
        assert!(score("git status", "st") > score("faster", "st"));
    }

    #[test]
    fn positions_pick_the_best_alignment() {
        assert_eq!(positions("git status", "gs"), vec![0, 4]);
        assert_eq!(positions("cargo build", "build"), vec![6, 7, 8, 9, 10]);
    }

    #[test]
    fn multi_term_queries_must_all_match_and_merge_positions() {
        let m = match_line("git push --force origin", "push git").unwrap();
        assert_eq!(m.positions, vec![0, 1, 2, 4, 5, 6, 7]);
        assert!(match_line("git push", "git nope").is_none());
    }

    #[test]
    fn gaps_are_penalized_by_length() {
        assert!(score("ab", "ab") > score("a-b", "ab"));
        assert!(score("a-b", "ab") > score("a---------b", "ab"));
    }

    #[test]
    fn unicode_haystacks_match_by_char() {
        assert_eq!(positions("构建 ls", "ls"), vec![3, 4]);
    }
}
