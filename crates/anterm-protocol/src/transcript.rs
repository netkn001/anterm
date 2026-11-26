//! Framework-free structured coding-agent transcript types shared between
//! `anterm-core` (host-side tailers), `anterm-gateway` (`TranscriptStream`),
//! and mobile clients.
//!
//! Streaming normalized transcript entries lets mobile clients render native
//! Markdown, collapsible tool calls, and inline diffs without issuing
//! `TIOCSWINSZ` resizes against the desktop PTY.

use serde::{Deserialize, Serialize};

use crate::cli_agent::CLIAgent;

/// One normalized entry extracted from a coding agent's native session
/// transcript (e.g. Claude Code or Codex JSONL files).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptEntry {
    /// Monotonically increasing 1-based sequence number within the stream,
    /// used for resumable delta catch-up (`since_seq`).
    pub seq: u64,
    /// Optional wall-clock timestamp in Unix milliseconds, when recorded by the
    /// agent's transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_ms: Option<u64>,
    pub kind: TranscriptEntryKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptEntryKind {
    /// A prompt or follow-up message submitted by the user.
    UserMessage { text: String },
    /// Assistant prose / Markdown response.
    AssistantText {
        markdown: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Collapsible model reasoning / thinking block.
    Thinking { text: String },
    /// A tool invocation initiated by the agent.
    ToolUse {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        name: String,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_preview: Option<String>,
    },
    /// Output returned from a tool call.
    ToolResult {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        output_preview: String,
        #[serde(default)]
        is_error: bool,
    },
    /// File modification surfaced inline as a unified diff.
    FileDiff {
        path: String,
        unified_diff: String,
    },
    /// Informational event (session compacted, model switched, etc.).
    SystemNotice { text: String },
}

/// Incremental batch of transcript entries pushed over a `TranscriptStream`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptBatch {
    pub pane_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<CLIAgent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Next sequence number the client should request on reconnect (`since_seq`).
    pub next_seq: u64,
    /// True when the underlying transcript was truncated/switched (e.g. `/clear`
    /// or a new session) and the client must replace its local timeline before
    /// appending `entries`.
    #[serde(default)]
    pub reset: bool,
    /// True when the host found a readable structured transcript for this pane;
    /// false signals the client to fall back to client-side reflowed terminal
    /// rendering (`ObserveSince`).
    #[serde(default = "default_true")]
    pub available: bool,
    #[serde(default)]
    pub entries: Vec<TranscriptEntry>,
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_batch_round_trips_all_entry_variants() {
        let batch = TranscriptBatch {
            pane_id: 7,
            agent: Some(CLIAgent::Claude),
            session_id: Some("sess-42".into()),
            next_seq: 8,
            reset: true,
            available: true,
            entries: vec![
                TranscriptEntry {
                    seq: 1,
                    timestamp_ms: Some(1_764_140_000_000),
                    kind: TranscriptEntryKind::UserMessage {
                        text: "Fix the failing clippy lint".into(),
                    },
                },
                TranscriptEntry {
                    seq: 2,
                    timestamp_ms: None,
                    kind: TranscriptEntryKind::Thinking {
                        text: "Inspecting workspace lints...".into(),
                    },
                },
                TranscriptEntry {
                    seq: 3,
                    timestamp_ms: None,
                    kind: TranscriptEntryKind::ToolUse {
                        call_id: Some("toolu_1".into()),
                        name: "Bash".into(),
                        summary: "cargo clippy --workspace".into(),
                        input_preview: Some(r#"{"command":"cargo clippy --workspace"}"#.into()),
                    },
                },
                TranscriptEntry {
                    seq: 4,
                    timestamp_ms: None,
                    kind: TranscriptEntryKind::ToolResult {
                        call_id: Some("toolu_1".into()),
                        name: Some("Bash".into()),
                        output_preview: "0 warnings".into(),
                        is_error: false,
                    },
                },
                TranscriptEntry {
                    seq: 5,
                    timestamp_ms: None,
                    kind: TranscriptEntryKind::FileDiff {
                        path: "crates/anterm-core/src/lib.rs".into(),
                        unified_diff: "@@ -1,1 +1,1 @@\n-old\n+new\n".into(),
                    },
                },
                TranscriptEntry {
                    seq: 6,
                    timestamp_ms: None,
                    kind: TranscriptEntryKind::AssistantText {
                        markdown: "All lints are now clean.".into(),
                        model: Some("claude-opus-4-6".into()),
                    },
                },
                TranscriptEntry {
                    seq: 7,
                    timestamp_ms: None,
                    kind: TranscriptEntryKind::SystemNotice {
                        text: "Context compacted".into(),
                    },
                },
            ],
        };

        let wire = serde_json::to_vec(&batch).unwrap();
        let decoded: TranscriptBatch = serde_json::from_slice(&wire).unwrap();
        assert_eq!(decoded, batch);
    }

    #[test]
    fn unavailable_batch_deserializes_with_minimal_fields() {
        let raw = r#"{"pane_id":3,"next_seq":0,"available":false}"#;
        let decoded: TranscriptBatch = serde_json::from_str(raw).unwrap();
        assert_eq!(decoded.pane_id, 3);
        assert!(!decoded.available);
        assert!(!decoded.reset);
        assert!(decoded.entries.is_empty());
    }
}
