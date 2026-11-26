//! Host-side structured coding-agent transcript tailer (`TranscriptStream`).
//!
//! Reads native JSONL session transcripts written by CLI coding agents
//! (Claude Code, Codex, etc.) on the machine where the agent actually runs,
//! normalizing entries into [`anterm_protocol::transcript::TranscriptBatch`] so
//! mobile clients can render native Markdown, collapsible tool calls, and inline
//! diffs with zero impact on the desktop PTY dimensions.

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anterm_protocol::cli_agent::CLIAgent;
use anterm_protocol::transcript::{TranscriptBatch, TranscriptEntry, TranscriptEntryKind};

/// Cap on preview strings emitted over mobile links to avoid shipping megabytes
/// of raw command stdout inside a single tool result card.
const MAX_PREVIEW_CHARS: usize = 4_096;

/// Incremental reader tracking a single pane's structured transcript file.
#[derive(Debug)]
pub struct TranscriptTailer {
    pane_id: u64,
    agent: Option<CLIAgent>,
    session_id: Option<String>,
    path: Option<PathBuf>,
    last_len: u64,
    entries: Vec<TranscriptEntry>,
}

impl TranscriptTailer {
    pub fn new(
        pane_id: u64,
        agent: Option<CLIAgent>,
        session_id: Option<String>,
        path: Option<PathBuf>,
    ) -> Self {
        Self {
            pane_id,
            agent,
            session_id,
            path,
            last_len: 0,
            entries: Vec::new(),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Update transcript source if the agent started a new session or reported
    /// its transcript path for the first time.
    pub fn update_source(
        &mut self,
        agent: Option<CLIAgent>,
        session_id: Option<String>,
        path: Option<PathBuf>,
    ) {
        self.agent = agent;
        let switched = self.path != path || self.session_id != session_id;
        if switched {
            self.session_id = session_id;
            self.path = path;
            self.last_len = 0;
            self.entries.clear();
        }
    }

    /// Poll the underlying transcript file and return every normalized entry
    /// strictly newer than `since_seq` (0 = full timeline).
    pub fn poll_since(&mut self, since_seq: u64) -> TranscriptBatch {
        let Some(path) = self.path.clone() else {
            return TranscriptBatch {
                pane_id: self.pane_id,
                agent: self.agent,
                session_id: self.session_id.clone(),
                next_seq: 0,
                reset: false,
                available: false,
                entries: Vec::new(),
            };
        };

        let Ok(meta) = fs::metadata(&path) else {
            return TranscriptBatch {
                pane_id: self.pane_id,
                agent: self.agent,
                session_id: self.session_id.clone(),
                next_seq: 0,
                reset: false,
                available: false,
                entries: Vec::new(),
            };
        };

        let len = meta.len();
        let mut reset = false;
        if len < self.last_len {
            // File was truncated or rewritten (e.g. `/clear` or compaction).
            self.last_len = 0;
            self.entries.clear();
            reset = true;
        }

        if len > self.last_len || self.entries.is_empty() {
            if let Ok(parsed) = parse_transcript_file(&path, self.agent) {
                if parsed.len() < self.entries.len() {
                    reset = true;
                }
                self.entries = parsed;
                self.last_len = len;
            }
        }

        let next_seq = self.entries.len() as u64;
        if since_seq > next_seq {
            reset = true;
        }
        let start = if reset {
            0
        } else {
            usize::try_from(since_seq).unwrap_or(self.entries.len())
        };
        let slice = self.entries.get(start..).unwrap_or(&[]).to_vec();

        TranscriptBatch {
            pane_id: self.pane_id,
            agent: self.agent,
            session_id: self.session_id.clone(),
            next_seq,
            reset,
            available: true,
            entries: slice,
        }
    }
}

/// Best-effort discovery of a native transcript path when hooks reported a
/// `session_id` without an explicit `transcript_path`.
pub fn discover_transcript_path(
    agent: CLIAgent,
    session_id: &str,
    explicit: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(p) = explicit
        && p.is_file()
    {
        return Some(p.to_path_buf());
    }
    if session_id.is_empty()
        || session_id.contains('/')
        || session_id.contains('\\')
        || session_id.contains("..")
    {
        return None;
    }
    let home = crate::util::home_dir()?;
    match agent {
        CLIAgent::Claude => {
            let projects_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".claude"))
                .join("projects");
            let target_name = format!("{session_id}.jsonl");
            find_named_file(&projects_dir, &target_name, 3)
        }
        CLIAgent::Codex => {
            let sessions_dir = std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex"))
                .join("sessions");
            find_file_containing(&sessions_dir, session_id, ".jsonl", 4)
        }
        _ => None,
    }
}

fn find_named_file(root: &Path, filename: &str, max_depth: usize) -> Option<PathBuf> {
    if max_depth == 0 {
        return None;
    }
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.file_name().and_then(|n| n.to_str()) == Some(filename) {
            return Some(path);
        }
        if path.is_dir()
            && let Some(found) = find_named_file(&path, filename, max_depth - 1)
        {
            return Some(found);
        }
    }
    None
}

fn find_file_containing(
    root: &Path,
    needle: &str,
    suffix: &str,
    max_depth: usize,
) -> Option<PathBuf> {
    if max_depth == 0 {
        return None;
    }
    let entries = fs::read_dir(root).ok()?;
    let mut best: Option<(u128, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(suffix) && name.contains(needle) {
                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                if best.as_ref().is_none_or(|(prev, _)| mtime >= *prev) {
                    best = Some((mtime, path));
                }
            }
        } else if path.is_dir()
            && let Some(found) = find_file_containing(&path, needle, suffix, max_depth - 1)
        {
            return Some(found);
        }
    }
    best.map(|(_, p)| p)
}

/// Parse an entire JSONL transcript file into monotonically sequenced entries.
pub fn parse_transcript_file(
    path: &Path,
    agent: Option<CLIAgent>,
) -> std::io::Result<Vec<TranscriptEntry>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    let mut seq = 0u64;

    for line in reader.lines() {
        let Ok(line) = line else {
            break;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let ts = parse_timestamp_ms(&value);
        let mut parsed_kinds = match agent {
            Some(CLIAgent::Codex) => parse_codex_line(&value),
            _ => {
                let claude = parse_claude_line(&value);
                if claude.is_empty() {
                    parse_codex_line(&value)
                } else {
                    claude
                }
            }
        };
        for kind in parsed_kinds.drain(..) {
            seq = seq.saturating_add(1);
            out.push(TranscriptEntry {
                seq,
                timestamp_ms: ts,
                kind,
            });
        }
    }
    Ok(out)
}

fn parse_claude_line(v: &serde_json::Value) -> Vec<TranscriptEntryKind> {
    let Some(kind) = v.get("type").and_then(|t| t.as_str()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    match kind {
        "user" => {
            if v.get("isMeta").and_then(|b| b.as_bool()) == Some(true) {
                return out;
            }
            let Some(content) = v.get("message").and_then(|m| m.get("content")) else {
                return out;
            };
            match content {
                serde_json::Value::String(text) => {
                    let t = text.trim();
                    if !t.is_empty() {
                        out.push(TranscriptEntryKind::UserMessage {
                            text: clip_chars(t, MAX_PREVIEW_CHARS),
                        });
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        match item.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                if let Some(text) = item
                                    .get("text")
                                    .and_then(|t| t.as_str())
                                    .map(str::trim)
                                    .filter(|t| !t.is_empty())
                                {
                                    out.push(TranscriptEntryKind::UserMessage {
                                        text: clip_chars(text, MAX_PREVIEW_CHARS),
                                    });
                                }
                            }
                            Some("tool_result") => {
                                let call_id = item
                                    .get("tool_use_id")
                                    .and_then(|s| s.as_str())
                                    .map(str::to_string);
                                let is_error = item
                                    .get("is_error")
                                    .and_then(|b| b.as_bool())
                                    .unwrap_or(false);
                                let preview = extract_tool_result_text(item.get("content"));
                                out.push(TranscriptEntryKind::ToolResult {
                                    call_id,
                                    name: None,
                                    output_preview: clip_chars(&preview, MAX_PREVIEW_CHARS),
                                    is_error,
                                });
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        "assistant" => {
            let msg = v.get("message");
            let model = msg
                .and_then(|m| m.get("model"))
                .and_then(|m| m.as_str())
                .map(str::to_string);
            let Some(items) = msg
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            else {
                return out;
            };
            for item in items {
                match item.get("type").and_then(|t| t.as_str()) {
                    Some("thinking") => {
                        if let Some(text) = item
                            .get("thinking")
                            .and_then(|t| t.as_str())
                            .map(str::trim)
                            .filter(|t| !t.is_empty())
                        {
                            out.push(TranscriptEntryKind::Thinking {
                                text: clip_chars(text, MAX_PREVIEW_CHARS),
                            });
                        }
                    }
                    Some("text") => {
                        if let Some(text) = item
                            .get("text")
                            .and_then(|t| t.as_str())
                            .map(str::trim)
                            .filter(|t| !t.is_empty())
                        {
                            out.push(TranscriptEntryKind::AssistantText {
                                markdown: text.to_string(),
                                model: model.clone(),
                            });
                        }
                    }
                    Some("tool_use") => {
                        let name = item
                            .get("name")
                            .and_then(|s| s.as_str())
                            .unwrap_or("tool")
                            .to_string();
                        let call_id = item
                            .get("id")
                            .and_then(|s| s.as_str())
                            .map(str::to_string);
                        let input = item.get("input");
                        let summary = summarize_tool_input(&name, input);
                        let input_preview = input.map(|val| {
                            clip_chars(&val.to_string(), MAX_PREVIEW_CHARS / 2)
                        });
                        out.push(TranscriptEntryKind::ToolUse {
                            call_id,
                            name: name.clone(),
                            summary,
                            input_preview,
                        });
                        if let Some(diff) = extract_inline_edit_diff(&name, input) {
                            out.push(diff);
                        }
                    }
                    _ => {}
                }
            }
        }
        "system" => {
            if let Some(text) = v
                .get("content")
                .or_else(|| v.get("message"))
                .and_then(|s| s.as_str())
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                out.push(TranscriptEntryKind::SystemNotice {
                    text: clip_chars(text, MAX_PREVIEW_CHARS),
                });
            }
        }
        _ => {}
    }
    out
}

fn parse_codex_line(v: &serde_json::Value) -> Vec<TranscriptEntryKind> {
    let mut out = Vec::new();
    let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if kind == "response_item"
        && let Some(payload) = v.get("payload")
    {
        let ptype = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match ptype {
            "message" => {
                let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                let text = extract_codex_message_text(payload.get("content"));
                if !text.is_empty() {
                    match role {
                        "user" => out.push(TranscriptEntryKind::UserMessage {
                            text: clip_chars(&text, MAX_PREVIEW_CHARS),
                        }),
                        "assistant" => out.push(TranscriptEntryKind::AssistantText {
                            markdown: text,
                            model: None,
                        }),
                        _ => {}
                    }
                }
            }
            "reasoning" => {
                if let Some(summary) = payload
                    .get("summary")
                    .and_then(|s| s.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .filter(|s| !s.trim().is_empty())
                {
                    out.push(TranscriptEntryKind::Thinking {
                        text: clip_chars(summary.trim(), MAX_PREVIEW_CHARS),
                    });
                }
            }
            "function_call" => {
                let name = payload
                    .get("name")
                    .and_then(|s| s.as_str())
                    .unwrap_or("tool")
                    .to_string();
                let call_id = payload
                    .get("call_id")
                    .and_then(|s| s.as_str())
                    .map(str::to_string);
                let args_raw = payload
                    .get("arguments")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                let parsed_args = serde_json::from_str::<serde_json::Value>(args_raw).ok();
                let summary = summarize_tool_input(&name, parsed_args.as_ref());
                out.push(TranscriptEntryKind::ToolUse {
                    call_id,
                    name,
                    summary,
                    input_preview: (!args_raw.is_empty())
                        .then(|| clip_chars(args_raw, MAX_PREVIEW_CHARS / 2)),
                });
            }
            "function_call_output" => {
                let call_id = payload
                    .get("call_id")
                    .and_then(|s| s.as_str())
                    .map(str::to_string);
                let output = payload
                    .get("output")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                out.push(TranscriptEntryKind::ToolResult {
                    call_id,
                    name: None,
                    output_preview: clip_chars(output, MAX_PREVIEW_CHARS),
                    is_error: false,
                });
            }
            _ => {}
        }
    }
    out
}

fn extract_codex_message_text(content: Option<&serde_json::Value>) -> String {
    let Some(content) = content else {
        return String::new();
    };
    if let Some(s) = content.as_str() {
        return s.trim().to_string();
    }
    let Some(arr) = content.as_array() else {
        return String::new();
    };
    arr.iter()
        .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_tool_result_text(content: Option<&serde_json::Value>) -> String {
    let Some(content) = content else {
        return String::new();
    };
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        return arr
            .iter()
            .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
    }
    content.to_string()
}

fn summarize_tool_input(name: &str, input: Option<&serde_json::Value>) -> String {
    let Some(obj) = input.and_then(|v| v.as_object()) else {
        return name.to_string();
    };
    for key in [
        "command",
        "cmd",
        "file_path",
        "path",
        "pattern",
        "query",
        "url",
        "description",
    ] {
        if let Some(s) = obj
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return clip_chars(s, 140);
        }
    }
    name.to_string()
}

fn extract_inline_edit_diff(
    tool_name: &str,
    input: Option<&serde_json::Value>,
) -> Option<TranscriptEntryKind> {
    if !matches!(tool_name, "Edit" | "edit_file" | "MultiEdit") {
        return None;
    }
    let obj = input?.as_object()?;
    let path = obj
        .get("file_path")
        .or_else(|| obj.get("path"))
        .and_then(|v| v.as_str())?
        .to_string();
    let old = obj.get("old_string").and_then(|v| v.as_str())?;
    let new = obj.get("new_string").and_then(|v| v.as_str())?;
    let mut diff = String::new();
    for line in old.lines() {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in new.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    Some(TranscriptEntryKind::FileDiff {
        path,
        unified_diff: clip_chars(&diff, MAX_PREVIEW_CHARS),
    })
}

fn parse_timestamp_ms(v: &serde_json::Value) -> Option<u64> {
    let ts = v.get("timestamp")?;
    if let Some(n) = ts.as_u64() {
        return Some(n);
    }
    None
}

fn clip_chars(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let clipped: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{clipped}…")
    } else {
        clipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_jsonl_parses_user_thinking_tools_diff_and_delta_tails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sess-1.jsonl");
        let initial = concat!(
            r#"{"type":"user","message":{"content":"Fix bug in lib.rs"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"model":"claude-opus-4-6","content":[{"type":"thinking","thinking":"Checking lib.rs"},{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"src/lib.rs","old_string":"let a = 1;","new_string":"let a = 2;"}}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"Updated src/lib.rs"}]}}"#,
            "\n",
        );
        fs::write(&path, initial).unwrap();

        let mut tailer = TranscriptTailer::new(
            7,
            Some(CLIAgent::Claude),
            Some("sess-1".into()),
            Some(path.clone()),
        );
        let first = tailer.poll_since(0);
        assert!(first.available);
        assert!(!first.reset);
        assert_eq!(first.entries.len(), 5);
        assert!(matches!(
            &first.entries[0].kind,
            TranscriptEntryKind::UserMessage { text } if text == "Fix bug in lib.rs"
        ));
        assert!(matches!(
            &first.entries[1].kind,
            TranscriptEntryKind::Thinking { .. }
        ));
        assert!(matches!(
            &first.entries[2].kind,
            TranscriptEntryKind::ToolUse { name, .. } if name == "Edit"
        ));
        assert!(matches!(
            &first.entries[3].kind,
            TranscriptEntryKind::FileDiff { path, unified_diff }
                if path == "src/lib.rs" && unified_diff.contains("-let a = 1;") && unified_diff.contains("+let a = 2;")
        ));
        assert!(matches!(
            &first.entries[4].kind,
            TranscriptEntryKind::ToolResult { output_preview, is_error: false, .. }
                if output_preview == "Updated src/lib.rs"
        ));

        // Append assistant final reply and verify incremental delta fetch.
        let mut appended = initial.to_string();
        appended.push_str(
            r#"{"type":"assistant","message":{"model":"claude-opus-4-6","content":[{"type":"text","text":"Fixed `a` to `2`."}]}}"#,
        );
        appended.push('\n');
        fs::write(&path, appended).unwrap();

        let delta = tailer.poll_since(first.next_seq);
        assert!(delta.available);
        assert!(!delta.reset);
        assert_eq!(delta.entries.len(), 1);
        assert_eq!(delta.entries[0].seq, 6);
        assert!(matches!(
            &delta.entries[0].kind,
            TranscriptEntryKind::AssistantText { markdown, model }
                if markdown == "Fixed `a` to `2`." && model.as_deref() == Some("claude-opus-4-6")
        ));

        // Truncate file to simulate `/clear` and confirm `reset: true`.
        fs::write(
            &path,
            r#"{"type":"user","message":{"content":"Fresh start"}}"#,
        )
        .unwrap();
        let after_clear = tailer.poll_since(delta.next_seq);
        assert!(after_clear.reset);
        assert_eq!(after_clear.entries.len(), 1);
        assert_eq!(after_clear.next_seq, 1);
    }

    #[test]
    fn codex_jsonl_parses_messages_reasoning_and_function_calls() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-abc.jsonl");
        let body = concat!(
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Run tests"}]}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"Will run cargo test"}]}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"function_call","call_id":"c1","name":"shell","arguments":"{\"command\":\"cargo test\"}"}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"ok"}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"All green!"}]}}"#,
            "\n",
        );
        fs::write(&path, body).unwrap();

        let mut tailer = TranscriptTailer::new(
            9,
            Some(CLIAgent::Codex),
            Some("abc".into()),
            Some(path),
        );
        let batch = tailer.poll_since(0);
        assert!(batch.available);
        assert_eq!(batch.entries.len(), 5);
        assert!(matches!(
            &batch.entries[2].kind,
            TranscriptEntryKind::ToolUse { summary, .. } if summary == "cargo test"
        ));
        assert!(matches!(
            &batch.entries[4].kind,
            TranscriptEntryKind::AssistantText { markdown, .. } if markdown == "All green!"
        ));
    }
}
