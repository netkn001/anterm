//! Shared UniFFI value types and error definitions exported to Swift/Kotlin.

use anterm_client_core::composer::QuickApprovalAction;
use anterm_protocol::gateway::{MobileRole, PairingBundle};
use anterm_protocol::transcript::{TranscriptBatch, TranscriptEntry, TranscriptEntryKind};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MobileError {
    #[error("{message}")]
    Message { message: String },
}

impl MobileError {
    pub(crate) fn msg(message: impl Into<String>) -> Self {
        Self::Message {
            message: message.into(),
        }
    }
}

impl From<anyhow::Error> for MobileError {
    fn from(err: anyhow::Error) -> Self {
        Self::msg(err.to_string())
    }
}

impl From<std::io::Error> for MobileError {
    fn from(err: std::io::Error) -> Self {
        Self::msg(err.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum FfiMobileRole {
    ObserveOnly,
    Interactive,
    Full,
}

impl From<MobileRole> for FfiMobileRole {
    fn from(role: MobileRole) -> Self {
        match role {
            MobileRole::ObserveOnly => Self::ObserveOnly,
            MobileRole::Interactive => Self::Interactive,
            MobileRole::Full => Self::Full,
        }
    }
}

impl From<FfiMobileRole> for MobileRole {
    fn from(role: FfiMobileRole) -> Self {
        match role {
            FfiMobileRole::ObserveOnly => Self::ObserveOnly,
            FfiMobileRole::Interactive => Self::Interactive,
            FfiMobileRole::Full => Self::Full,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FfiAgentStatus {
    Idle,
    Working,
    Waiting,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FfiQuickApproval {
    ApproveOnce,
    ApproveAlways,
    Deny,
    Interrupt,
    Choice { index: u8 },
}

pub(crate) fn approval_bytes(
    agent: Option<anterm_protocol::cli_agent::CLIAgent>,
    action: FfiQuickApproval,
) -> Vec<u8> {
    match action {
        FfiQuickApproval::ApproveOnce => {
            anterm_client_core::composer::quick_approval_bytes(agent, QuickApprovalAction::ApproveOnce)
        }
        FfiQuickApproval::ApproveAlways => anterm_client_core::composer::quick_approval_bytes(
            agent,
            QuickApprovalAction::ApproveAlways,
        ),
        FfiQuickApproval::Deny => {
            anterm_client_core::composer::quick_approval_bytes(agent, QuickApprovalAction::Deny)
        }
        FfiQuickApproval::Choice { index } => anterm_client_core::composer::quick_approval_bytes(
            agent,
            QuickApprovalAction::Choice(index),
        ),
        FfiQuickApproval::Interrupt => anterm_client_core::composer::interrupt_keys(agent)
            .into_iter()
            .flat_map(|s| s.bytes)
            .collect(),
    }
}

pub(crate) fn parse_cli_agent(name: &str) -> Option<anterm_protocol::cli_agent::CLIAgent> {
    use anterm_protocol::cli_agent::CLIAgent;
    match name.trim().to_ascii_lowercase().as_str() {
        "claude" | "claude-code" => Some(CLIAgent::Claude),
        "codex" => Some(CLIAgent::Codex),
        "gemini" => Some(CLIAgent::Gemini),
        "opencode" => Some(CLIAgent::OpenCode),
        "copilot" => Some(CLIAgent::Copilot),
        "cursor" => Some(CLIAgent::Cursor),
        "amp" => Some(CLIAgent::Amp),
        "goose" => Some(CLIAgent::Goose),
        "droid" => Some(CLIAgent::Droid),
        "grok" => Some(CLIAgent::Grok),
        "pi" => Some(CLIAgent::Pi),
        "qwen" => Some(CLIAgent::Qwen),
        "kimi" => Some(CLIAgent::Kimi),
        "crush" => Some(CLIAgent::Crush),
        _ => None,
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiPairingInfo {
    pub host_node_id: String,
    pub host_name: String,
    pub relay_url: Option<String>,
    pub direct_addrs: Vec<String>,
    pub expires_at: u64,
    pub is_expired: bool,
}

impl From<&PairingBundle> for FfiPairingInfo {
    fn from(bundle: &PairingBundle) -> Self {
        Self {
            host_node_id: bundle.node_id.clone(),
            host_name: bundle.host_name.clone(),
            relay_url: bundle.relay_url.clone(),
            direct_addrs: bundle.lan_addrs.clone(),
            expires_at: bundle.expires_at,
            is_expired: bundle.is_expired(anterm_protocol::util::unix_now()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, uniffi::Record)]
pub struct FfiPairedHost {
    pub host_node_id: String,
    pub host_name: String,
    pub relay_url: Option<String>,
    pub direct_addrs: Vec<String>,
    pub role: FfiMobileRole,
    pub paired_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FfiTranscriptEntryKind {
    UserPrompt,
    AssistantMarkdown,
    Thinking,
    ToolCall,
    ToolResult,
    EditDiff,
    SystemNotice,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiTranscriptEntry {
    pub seq: u64,
    pub timestamp_ms: Option<u64>,
    pub kind: FfiTranscriptEntryKind,
    pub title: String,
    pub body: String,
    pub tool_name: Option<String>,
    pub file_path: Option<String>,
    pub diff_old: Option<String>,
    pub diff_new: Option<String>,
    pub is_error: bool,
    pub in_progress: bool,
}

impl From<TranscriptEntry> for FfiTranscriptEntry {
    fn from(entry: TranscriptEntry) -> Self {
        let mut out = Self {
            seq: entry.seq,
            timestamp_ms: entry.timestamp_ms,
            kind: FfiTranscriptEntryKind::SystemNotice,
            title: String::new(),
            body: String::new(),
            tool_name: None,
            file_path: None,
            diff_old: None,
            diff_new: None,
            is_error: false,
            in_progress: false,
        };
        match entry.kind {
            TranscriptEntryKind::UserMessage { text } => {
                out.kind = FfiTranscriptEntryKind::UserPrompt;
                out.body = text;
            }
            TranscriptEntryKind::AssistantText { markdown, model } => {
                out.kind = FfiTranscriptEntryKind::AssistantMarkdown;
                out.title = model.unwrap_or_default();
                out.body = markdown;
            }
            TranscriptEntryKind::Thinking { text } => {
                out.kind = FfiTranscriptEntryKind::Thinking;
                out.title = "Thinking".into();
                out.body = text;
            }
            TranscriptEntryKind::ToolUse {
                name,
                summary,
                input_preview,
                ..
            } => {
                out.kind = FfiTranscriptEntryKind::ToolCall;
                out.title = if summary.is_empty() { name.clone() } else { summary };
                out.tool_name = Some(name);
                out.body = input_preview.unwrap_or_default();
            }
            TranscriptEntryKind::ToolResult {
                name,
                output_preview,
                is_error,
                ..
            } => {
                out.kind = FfiTranscriptEntryKind::ToolResult;
                out.title = name.clone().unwrap_or_else(|| "Result".into());
                out.tool_name = name;
                out.body = output_preview;
                out.is_error = is_error;
            }
            TranscriptEntryKind::FileDiff { path, unified_diff } => {
                out.kind = FfiTranscriptEntryKind::EditDiff;
                out.title = path.clone();
                out.file_path = Some(path);
                out.body = unified_diff;
            }
            TranscriptEntryKind::SystemNotice { text } => {
                out.kind = FfiTranscriptEntryKind::SystemNotice;
                out.body = text;
            }
        }
        out
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiTranscriptBatch {
    pub pane_id: u64,
    pub session_id: Option<String>,
    pub entries: Vec<FfiTranscriptEntry>,
    pub next_seq: u64,
    pub reset: bool,
}

impl From<TranscriptBatch> for FfiTranscriptBatch {
    fn from(batch: TranscriptBatch) -> Self {
        Self {
            pane_id: batch.pane_id,
            session_id: batch.session_id,
            entries: batch.entries.into_iter().map(Into::into).collect(),
            next_seq: batch.next_seq,
            reset: batch.reset,
        }
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiReflowLine {
    pub text: String,
    pub is_wrapped_continuation: bool,
    pub row_index: u32,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiPaneSummary {
    pub pane_id: u64,
    pub title: String,
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
    pub agent: Option<String>,
    pub agent_status: FfiAgentStatus,
    pub agent_detail: Option<String>,
    pub has_transcript: bool,
    pub listening_ports: Vec<u16>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiTabSummary {
    pub tab_id: String,
    pub title: String,
    pub tag: Option<String>,
    pub desktop_focused: bool,
    pub hibernated: bool,
    pub active_pane_id: Option<u64>,
    pub panes: Vec<FfiPaneSummary>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiRemoteRoute {
    pub key: String,
    pub kind: String,
    pub connected: bool,
    pub host_label: String,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiWorkspaceSnapshot {
    pub host_name: String,
    pub desktop_active_tab_id: Option<String>,
    pub tabs: Vec<FfiTabSummary>,
    pub waiting_panes: Vec<FfiPaneSummary>,
    pub routes: Vec<FfiRemoteRoute>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FfiWorkspaceEventKind {
    LayoutChanged,
    AgentStatusChanged,
    PaneExited,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiWorkspaceEvent {
    pub kind: FfiWorkspaceEventKind,
    pub pane_id: Option<u64>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiFileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub ignored: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiGitChangedFile {
    pub path: String,
    pub status: String,
    pub staged: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiGitDiffSnapshot {
    pub repo_root: String,
    pub branch: String,
    pub files: Vec<FfiGitChangedFile>,
    pub unified_diff: String,
}
