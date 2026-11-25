use std::io::{IsTerminal as _, Read as _};
use std::path::{Path, PathBuf};

use crate::cli_agent::AGENT_EVENT_SENTINEL;

use super::target::{MAX_CONFIG_BYTES, home_dir};

pub const ANTERM_ENV_MARKER: &str = "ANTERM";

const GROK_HOOK_ENV: &str = "GROK_HOOK_EVENT";

const MAX_STDIN: u64 = 64 * 1024;

pub fn run_agent_hook(agent: &str, event: &str) {
    detach_console();
    if std::env::var_os(ANTERM_ENV_MARKER).is_none() {
        return;
    }
    let agent = effective_agent(agent, std::env::var_os(GROK_HOOK_ENV).is_some());
    let mut input = String::new();
    if !std::io::stdin().is_terminal() {
        let _ = std::io::stdin().take(MAX_STDIN).read_to_string(&mut input);
    }
    let input = hook_payload(agent, input, |k| std::env::var(k).ok());
    let Some(event) = effective_event(agent, event, &input) else {
        return;
    };
    let body = build_hook_body(agent, event, &input);
    if report_to_daemon(&body) {
        return;
    }
    write_to_controlling_tty(&hook_sequence(&body));
}

/// Hand the report to the daemon that owns this pane, over its socket.
///
/// The tty is the channel of last resort. The agent draws on it too, and a
/// tty write is not atomic — on macOS the kernel parks a writer mid-write
/// whenever the output queue is full, which is exactly when an agent is busy
/// redrawing — so the agent's output can land inside the sequence, break it,
/// and leave the rest of it on screen as text. The socket has no such
/// neighbour. It only works where the daemon runs, though: a hook the
/// environment points at no daemon, at a daemon that predates this message,
/// or at a pane it does not run in (see `DaemonPane::report_agent_event`)
/// falls back to the tty.
fn report_to_daemon(body: &str) -> bool {
    const AGENT_EVENT_REPLY_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

    let Some(pane_id) = std::env::var(crate::daemon::pane::ANTERM_PANE_ENV)
        .ok()
        .and_then(|p| p.trim().parse::<u64>().ok())
    else {
        return false;
    };
    let Ok(mut stream) = crate::daemon::transport::connect() else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(AGENT_EVENT_REPLY_WAIT));
    let _ = stream.set_write_timeout(Some(AGENT_EVENT_REPLY_WAIT));
    let msg = crate::daemon::protocol::ClientMsg::AgentEvent {
        pane_id,
        pid: std::process::id(),
        event: body.to_string(),
    };
    if msg.encode(&mut stream).is_err() {
        return false;
    }
    matches!(
        crate::daemon::protocol::DaemonMsg::read(&mut stream),
        Ok(crate::daemon::protocol::DaemonMsg::InputAck { .. })
    )
}

#[cfg(not(unix))]
fn detach_console() {
    use windows_sys::Win32::System::Console::FreeConsole;
    unsafe {
        FreeConsole();
    }
}

#[cfg(unix)]
fn detach_console() {}

/// The event's payload: what the agent wrote on stdin, or — for jcode, which
/// leaves stdin closed and describes the event in its environment instead —
/// the JSON mirror it exports as `JCODE_HOOK_PAYLOAD` (`session_id`, `cwd`,
/// and the event's own fields).
pub(super) fn hook_payload(agent: &str, stdin: String, env: impl Fn(&str) -> Option<String>) -> String {
    if agent == "jcode"
        && stdin.trim().is_empty()
        && let Some(payload) = env("JCODE_HOOK_PAYLOAD").filter(|p| !p.trim().is_empty())
    {
        return payload;
    }
    stdin
}

pub(super) fn effective_agent(agent: &str, ran_by_grok: bool) -> &str {
    if ran_by_grok { "grok" } else { agent }
}

/// The tools that stop a turn to ask the user a question: Claude's
/// `AskUserQuestion` and Codex's `request_user_input`. Neither goes through a
/// permission prompt, so their `PreToolUse` is the only sign the turn is now
/// waiting on the user.
pub(super) const QUESTION_TOOLS: &[&str] = &["AskUserQuestion", "request_user_input"];

pub(super) fn effective_event<'a>(agent: &str, event: &'a str, stdin_json: &str) -> Option<&'a str> {
    // `tool-start` is not an event of its own: it fires before every tool
    // call, and only the ones that ask the user something change the status.
    if event == "tool-start" {
        let payload = serde_json::from_str::<serde_json::Value>(stdin_json).ok()?;
        let tool = payload.get("tool_name").and_then(|t| t.as_str())?;
        return QUESTION_TOOLS.contains(&tool).then_some("question-asked");
    }
    // Antigravity has no turn-start event, only `PreInvocation` before every
    // model call. The first call of a turn starts it; later ones are the same
    // turn still working, and must not count as new prompts. `Stop` with
    // `fullyIdle: false` ends one execution loop while more are queued.
    if agent == "antigravity"
        && let Ok(payload) = serde_json::from_str::<serde_json::Value>(stdin_json)
    {
        if event == "prompt-submit"
            && payload
                .get("invocationNum")
                .and_then(|n| n.as_u64())
                .unwrap_or(0)
                > 0
        {
            return Some("tool-complete");
        }
        if event == "stop" && payload.get("fullyIdle").and_then(|v| v.as_bool()) == Some(false) {
            return None;
        }
    }
    // Qoder, its China build and CodeBuddy also emit SessionStart after
    // compacting the active turn. Preserve its status until a real turn or
    // session boundary arrives.
    if matches!(agent, "qodercli" | "qoderclicn" | "codebuddy")
        && event == "session-start"
        && let Ok(payload) = serde_json::from_str::<serde_json::Value>(stdin_json)
        && payload.get("source").and_then(|value| value.as_str()) == Some("compact")
    {
        return None;
    }
    if matches!(agent, "copilot" | "grok" | "droid" | "gemini") && event == "notification" {
        let blocks = stdin_json.contains("elicitation_dialog")
            || (matches!(agent, "copilot" | "droid") && stdin_json.contains("permission_prompt"))
            // Gemini's only notification kind so far, but naming it keeps a
            // future non-blocking one from being read as a block.
            || (agent == "gemini" && stdin_json.contains("ToolPermission"));
        return blocks.then_some("permission-request");
    }
    Some(event)
}

#[cfg(test)]
pub(super) fn build_hook_sequence(agent: &str, event: &str, stdin_json: &str) -> Vec<u8> {
    hook_sequence(&build_hook_body(agent, event, stdin_json))
}

/// The report as it goes out on the tty: the body wrapped in the OSC 777
/// notification the daemon's reader looks for.
fn hook_sequence(body: &str) -> Vec<u8> {
    format!("\x1b]777;notify;{AGENT_EVENT_SENTINEL};{body}\x07").into_bytes()
}

/// The report itself, as JSON — see `cli_agent::parse_agent_event_body`.
fn build_hook_body(agent: &str, event: &str, stdin_json: &str) -> String {
    let payload: serde_json::Value =
        serde_json::from_str(stdin_json).unwrap_or(serde_json::json!({}));
    let mut body = serde_json::json!({
        "v": 1,
        "agent": agent,
        "event": event,
    });
    for (key, alias) in [
        ("session_id", "sessionId"),
        // Cursor names it after the conversation; only its `sessionStart`
        // repeats the same id as `session_id`.
        ("session_id", "conversation_id"),
        ("message", "message"),
        ("cwd", "cwd"),
        // Goose spells the working directory its own way.
        ("cwd", "working_dir"),
        // What started a session — see `cli_agent::AgentEvent::source`.
        ("source", "source"),
    ] {
        if let Some(v) = payload
            .get(key)
            .or_else(|| payload.get(alias))
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
        {
            body[key] = serde_json::Value::String(v.to_string());
        }
    }
    if let Some(message) = body.get_mut("message")
        && let Some(text) = message.as_str()
    {
        *message = clip_chars(text, MESSAGE_MAX).into();
    }
    // Antigravity names the session after the conversation, in camelCase.
    // Scoped to it: the alias table is last-write-wins, and another agent's
    // unrelated `conversationId` must not replace its real session id.
    if agent == "antigravity"
        && body.get("session_id").is_none()
        && let Some(id) = payload
            .get("conversationId")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
    {
        body["session_id"] = serde_json::Value::String(id.to_string());
    }
    // Cursor puts `cwd` only on its tool events; the rest carry the workspace
    // roots instead, and the first of those is where the session runs.
    // Antigravity's `workspacePaths` is the same list under its own name.
    if body.get("cwd").is_none()
        && let Some(root) = payload
            .get("workspace_roots")
            .or_else(|| payload.get("workspacePaths"))
            .and_then(|r| r.as_array())
            .and_then(|r| r.first())
            .and_then(|r| r.as_str())
            .filter(|r| !r.is_empty())
    {
        body["cwd"] = serde_json::Value::String(root.to_string());
    }
    if agent == "claude" {
        claude_readout(event, &payload, &mut body);
    }
    body.to_string()
}

/// How much of the end of a transcript is read for the latest reply. One
/// assistant entry is a few KiB; this is room for a long tool result after it.
const TRANSCRIPT_TAIL: u64 = 512 * 1024;

/// How long `Stop` waits for its turn's reply to reach the transcript, and
/// how often it looks meanwhile.
const TRANSCRIPT_CATCH_UP: std::time::Duration = std::time::Duration::from_millis(600);
const TRANSCRIPT_POLL: std::time::Duration = std::time::Duration::from_millis(40);

/// What Claude Code says about its settings, for the composer's toolbar:
/// permission mode, model, the effort level.
///
/// Worked out here, in the hook, because the hook runs where the agent runs —
/// on a remote host its transcript and settings are only readable there.
fn claude_readout(event: &str, payload: &serde_json::Value, body: &mut serde_json::Value) {
    let str_of = |v: Option<&serde_json::Value>| {
        v.and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    if let Some(mode) = str_of(payload.get("permission_mode")) {
        body["permission_mode"] = mode.into();
    }
    // A new session names its model. After that only a finished turn says
    // which it is: the model that wrote the reply `Stop` carries. Any other
    // reply in the transcript may be from before a `/model`, or from before
    // a resume under another `--model` — and saying nothing beats that.
    let model = str_of(payload.get("model")).or_else(|| {
        if event != "stop" {
            return None;
        }
        let reply = str_of(payload.get("last_assistant_message"))?;
        let path = str_of(payload.get("transcript_path"))?;
        let prompt_id = str_of(payload.get("prompt_id"));
        turn_reply_model(Path::new(&path), &reply, prompt_id.as_deref())
    });
    if let Some(model) = &model {
        body["model"] = model.clone().into();
    }
    // Claude says the level it is running at, when it says it — and at the
    // end of a turn it says it whenever the model takes one, so a finished
    // turn without it ran on a model that does not: that is reported as an
    // empty level. So is one for a model Claude's own model catalog lists
    // without effort levels. Otherwise the environment wins over the settings
    // file, as it does for Claude — read only for a model this event names,
    // since the file may set it per model.
    let effort = str_of(payload.get("effort").and_then(|e| e.get("level"))).or_else(|| {
        let model = model.as_deref()?;
        if event == "stop" || catalog_says_no_effort(&claude_model_catalogs(), model) {
            return Some(String::new());
        }
        std::env::var("CLAUDE_CODE_EFFORT_LEVEL")
            .ok()
            .filter(|e| !e.is_empty())
            .or_else(|| configured_effort(&claude_settings(), Some(model)))
    });
    if let Some(effort) = effort {
        body["effort"] = effort.into();
    }
}

/// The effort level Claude's settings give a model: the one set for that
/// model if there is one, the general one otherwise.
pub(super) fn configured_effort(settings: &serde_json::Value, model: Option<&str>) -> Option<String> {
    let level = |v: Option<&serde_json::Value>| {
        v.and_then(|v| v.get("effortLevel"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let per_model = settings.get("modelSettings");
    model
        .and_then(|m| {
            let per_model = per_model?;
            level(
                per_model
                    .get(m)
                    .or_else(|| per_model.get(m.trim_end_matches("[1m]"))),
            )
        })
        .or_else(|| level(Some(settings)))
}

fn claude_config_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".claude")))
}

fn claude_settings() -> serde_json::Value {
    claude_config_dir()
        .and_then(|d| std::fs::read(d.join("settings.json")).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::json!({}))
}

/// The model catalogs Claude Code keeps in its cache, one per account it
/// has signed in with, as parsed JSON. None, if it has not fetched any.
fn claude_model_catalogs() -> Vec<serde_json::Value> {
    let Some(dir) = claude_config_dir().map(|d| d.join("cache").join("model-catalog")) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter(|e| e.metadata().is_ok_and(|m| m.len() <= MAX_CONFIG_BYTES))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice(&b).ok())
        .collect()
}

/// Whether Claude's model catalogs list `model` as taking no effort level —
/// Haiku, say. Only a model they list counts, and only when every catalog
/// that lists it agrees: a model they do not know may well take one.
pub(super) fn catalog_says_no_effort(catalogs: &[serde_json::Value], model: &str) -> bool {
    let model = model.trim_end_matches("[1m]");
    let mut listed = catalogs
        .iter()
        .filter_map(|c| c.pointer("/catalog/config/models")?.as_array())
        .flatten()
        .filter(|m| m.get("id").and_then(|id| id.as_str()) == Some(model))
        .peekable();
    listed.peek().is_some()
        && listed.all(|m| {
            m.pointer("/thinking/effort_options")
                .and_then(|o| o.as_array())
                .is_none_or(|o| o.is_empty())
        })
}

/// The model that wrote `reply` to the prompt `prompt_id`, once the
/// transcript has it as its latest words. Claude Code runs `Stop` before
/// that write is always on disk, so this waits a little for it — reading
/// the tail again only once the transcript has grown.
fn turn_reply_model(path: &Path, reply: &str, prompt_id: Option<&str>) -> Option<String> {
    let deadline = std::time::Instant::now() + TRANSCRIPT_CATCH_UP;
    let mut read = None;
    loop {
        if let Some(model) =
            transcript_tail(path, &mut read).and_then(|t| reply_model_in(&t, reply, prompt_id))
        {
            return Some(model);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(TRANSCRIPT_POLL);
    }
}

/// The transcript's last [`TRANSCRIPT_TAIL`] bytes, unless it is still the
/// length `read` last saw — then nothing new is in it.
fn transcript_tail(path: &Path, read: &mut Option<u64>) -> Option<String> {
    use std::io::{Seek as _, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if read.replace(len) == Some(len) {
        return None;
    }
    file.seek(SeekFrom::Start(len.saturating_sub(TRANSCRIPT_TAIL)))
        .ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    Some(String::from_utf8_lossy(&tail).into_owned())
}

/// The model of the transcript's latest main-thread words, when those words
/// are `reply` and come after the prompt `prompt_id` names — the same words
/// can close an earlier turn too. Subagents' entries are skipped — they may
/// run on a model of their own — and so are the notices Claude Code writes
/// as replies.
pub(super) fn reply_model_in(jsonl: &str, reply: &str, prompt_id: Option<&str>) -> Option<String> {
    let mut words = None;
    let mut prompted = prompt_id.is_none();
    for line in jsonl.lines() {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = entry.get("type").and_then(|t| t.as_str());
        if kind == Some("user") {
            if prompt_id.is_some() && entry.get("promptId").and_then(|p| p.as_str()) == prompt_id {
                prompted = true;
                words = None;
            }
            continue;
        }
        if kind != Some("assistant")
            || entry.get("isSidechain").and_then(|v| v.as_bool()) == Some(true)
        {
            continue;
        }
        let Some(message) = entry.get("message") else {
            continue;
        };
        let model = message.get("model").and_then(|m| m.as_str());
        let text: String = message
            .get("content")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect();
        if let Some(model) = model.filter(|m| *m != "<synthetic>")
            && !text.trim().is_empty()
        {
            words = Some((model.to_string(), text));
        }
    }
    let (model, text) = words.filter(|_| prompted)?;
    (text.trim() == reply.trim()).then_some(model)
}

/// How much of an agent's message rides back to the terminal. It only ever
/// fills a status line, and every byte of the sequence is a byte another
/// writer on the same tty can cut in on.
pub(super) const MESSAGE_MAX: usize = 200;

pub(super) fn clip_chars(text: &str, max: usize) -> &str {
    text.char_indices()
        .nth(max)
        .map_or(text, |(i, _)| &text[..i])
}

#[cfg(unix)]
fn write_to_controlling_tty(bytes: &[u8]) -> bool {
    if write_dev(std::path::Path::new("/dev/tty"), bytes) {
        return true;
    }
    if let Some(dev) = ancestor_tty_device() {
        return write_dev(&dev, bytes);
    }
    false
}

/// Write `bytes` to the terminal device at `path`.
///
/// The device is opened without waiting. A blocking open of a terminal whose
/// other end has gone — the pane closed while the agent was still running its
/// session-end hook — waits for a carrier that never comes, and on macOS it
/// waits holding the lock every lookup under `/dev` needs: the hook hangs, the
/// agent it belongs to cannot finish exiting, and every new terminal, `ps`
/// and `tty` on the machine hangs behind them. Once open, the device goes back
/// to blocking writes, so a full output queue delays the sequence rather than
/// cutting it short; a terminal with no other end fails the write instead.
#[cfg(unix)]
pub(super) fn write_dev(path: &std::path::Path, bytes: &[u8]) -> bool {
    use std::io::Write as _;
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let Ok(mut tty) = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
    else {
        return false;
    };
    let fd = tty.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return false;
    }
    tty.write_all(bytes).and_then(|_| tty.flush()).is_ok()
}

#[cfg(unix)]
pub(super) fn ancestor_tty_device() -> Option<std::path::PathBuf> {
    use std::process::Command;
    let mut pid = unsafe { libc::getppid() };
    for _ in 0..8 {
        if pid <= 1 {
            break;
        }
        let out = Command::new("ps")
            .args(["-o", "tty=", "-o", "ppid=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let line = String::from_utf8_lossy(&out.stdout);
        let mut fields = line.split_whitespace();
        let tty = fields.next().unwrap_or("");
        let ppid: i32 = fields.next().and_then(|s| s.parse().ok()).unwrap_or(1);
        if !tty.is_empty() && tty != "??" && tty != "?" {
            return Some(std::path::PathBuf::from(format!("/dev/{tty}")));
        }
        pid = ppid;
    }
    None
}

#[cfg(not(unix))]
fn write_to_controlling_tty(bytes: &[u8]) -> bool {
    let procs = crate::daemon::winproc::snapshot();
    let ancestors = ancestor_pids(&procs);

    let name_of = |pid: u32| {
        procs
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| p.name.to_ascii_lowercase())
    };
    let shell = ancestors.iter().copied().find(|&pid| {
        procs
            .iter()
            .find(|p| p.pid == pid)
            .and_then(|p| name_of(p.parent))
            .is_some_and(|n| is_anterm_host_exe(&n))
    });

    if let Some(pid) = shell {
        if attach_and_write(pid, bytes) {
            return true;
        }
    }

    let mut any = false;
    for pid in ancestors {
        any |= attach_and_write(pid, bytes);
    }
    any
}

#[cfg(any(not(unix), test))]
pub(super) fn is_anterm_host_exe(name: &str) -> bool {
    matches!(name, "anterm-app.exe" | "anterm-server.exe" | "anterm.exe")
}

#[cfg(not(unix))]
fn attach_and_write(pid: u32, bytes: &[u8]) -> bool {
    use windows_sys::Win32::System::Console::{AttachConsole, FreeConsole};
    unsafe {
        FreeConsole();
        if AttachConsole(pid) == 0 {
            return false;
        }
    }
    let ok = write_conout(bytes);
    unsafe {
        FreeConsole();
    }
    ok
}

#[cfg(not(unix))]
fn write_conout(bytes: &[u8]) -> bool {
    use std::io::Write as _;
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")
    {
        Ok(mut out) => out.write_all(bytes).and_then(|_| out.flush()).is_ok(),
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn ancestor_pids(procs: &[crate::daemon::winproc::Proc]) -> Vec<u32> {
    let parent_of = |pid: u32| procs.iter().find(|p| p.pid == pid).map(|p| p.parent);
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cur = std::process::id();
    seen.insert(cur);
    for _ in 0..16 {
        match parent_of(cur) {
            Some(parent) if parent != 0 && seen.insert(parent) => {
                out.push(parent);
                cur = parent;
            }
            _ => break,
        }
    }
    out
}
