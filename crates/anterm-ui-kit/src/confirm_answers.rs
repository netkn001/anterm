//! Standard confirmation dialog button order.

/// The two answers a confirmation dialog gets, arranged the way macOS arranges
/// them: the action on the right, where Return lands, and the safe answer on
/// the left, marked as cancel so it answers Escape, Space, and initial focus.
pub fn confirm_answers(action: &str, keep: &str) -> [gpui::PromptButton; 2] {
    [
        gpui::PromptButton::ok(action),
        gpui::PromptButton::cancel(keep),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_action_answers_return_and_the_safe_one_answers_escape() {
        let [action, keep] = super::confirm_answers("Delete", "Cancel");
        assert_eq!(action.label(), "Delete");
        assert!(!action.is_cancel(), "answer 0 has to keep Return");
        assert_eq!(keep.label(), "Cancel");
        assert!(keep.is_cancel(), "only a cancel answer is given Escape");
    }
}
