//! Shell-integration-aware predictive local echo for high-latency mobile links.
//!
//! Only activates when the shell is confirmed to be at an interactive prompt
//! (`at_prompt && !alt_screen`) and automatically cancels on control keys,
//! submission, or unexpected remote output so password prompts and TUI apps
//! never display stale ghost glyphs.

const MAX_PREDICTED_CHARS: usize = 64;

#[derive(Default, Debug, Clone)]
pub struct PredictiveEcho {
    pending: String,
}

impl PredictiveEcho {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record local printable input bytes when safe to predict.
    pub fn observe_input(&mut self, bytes: &[u8], eligible: bool) {
        if !eligible {
            self.pending.clear();
            return;
        }
        if bytes == b"\x7f" || bytes == b"\x08" {
            self.pending.pop();
            return;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            self.pending.clear();
            return;
        };
        if text.chars().any(char::is_control) {
            self.pending.clear();
            return;
        }
        for ch in text.chars() {
            if self.pending.chars().count() >= MAX_PREDICTED_CHARS {
                self.pending.clear();
                return;
            }
            self.pending.push(ch);
        }
    }

    /// Reconcile incoming remote PTY bytes against pending optimistic text.
    pub fn reconcile_output(&mut self, bytes: &[u8]) {
        if self.pending.is_empty() || bytes.is_empty() {
            return;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            self.pending.clear();
            return;
        };
        if let Some(rest) = self.pending.strip_prefix(text) {
            self.pending = rest.to_string();
        } else {
            self.pending.clear();
        }
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }

    pub fn overlay(&self) -> Option<&str> {
        (!self.pending.is_empty()).then_some(self.pending.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echoes_and_reconciles_prefix_and_clears_on_mismatch() {
        let mut echo = PredictiveEcho::new();
        echo.observe_input(b"git ", true);
        echo.observe_input(b"st", true);
        assert_eq!(echo.overlay(), Some("git st"));

        echo.reconcile_output(b"git ");
        assert_eq!(echo.overlay(), Some("st"));

        echo.observe_input(b"\x7f", true);
        assert_eq!(echo.overlay(), Some("s"));

        echo.observe_input(b"\r", true);
        assert_eq!(echo.overlay(), None);
    }

    #[test]
    fn disabled_when_ineligible_such_as_alt_screen_or_password() {
        let mut echo = PredictiveEcho::new();
        echo.observe_input(b"secret", false);
        assert_eq!(echo.overlay(), None);
    }
}
