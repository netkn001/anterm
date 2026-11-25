use std::io;
use std::path::{Path, PathBuf};

use crate::cli_agent::CLIAgent;
use crate::host::Host;

#[cfg(unix)]
use super::runner::{ancestor_tty_device, write_dev};
use super::runner::{
    MESSAGE_MAX, build_hook_sequence, catalog_says_no_effort, clip_chars, configured_effort,
    effective_agent, effective_event, hook_payload, is_anterm_host_exe, reply_model_in,
};
use super::strategies::{
    ANTIGRAVITY_HOOK_SET, CLAUDE_HOOK_EVENTS, CODEBUDDY_HOOK_EVENTS, CODEX_HOOK_EVENTS,
    CRUSH_HOOK_EVENTS, CURSOR_HOOK_EVENTS, DROID_HOOK_EVENTS, GEMINI_HOOK_EVENTS,
    GOOSE_HOOK_EVENTS, GROK_HOOK_EVENTS, KIMI_HOOK_EVENTS, MUSE_HOOK_EVENTS, MUSE_PLUGIN_ID,
    QODER_HOOK_EVENTS, QWEN_HOOK_EVENTS, TRAE_CODE_HOOK_EVENTS, copilot_hooks_json,
    goose_hooks_json, grok_hooks_json, jcode_commands, marker_command, muse_inspected_state,
    muse_install_with, muse_plugin_manifest, opencode_plugin_js, owned_file_install,
    owned_file_state, owned_file_uninstall, pi_extension_ts, toml_command_is_marked,
};
use super::*;

/// A hook whose pane closed under it gives up instead of waiting on the
/// terminal's other end for good.
#[cfg(unix)]
#[test]
fn a_terminal_with_its_other_end_gone_fails_the_write_at_once() {
    // Both ends are opened close-on-exec from the start: the other tests
    // spawn processes on threads of their own, and a child that inherited
    // the pane's end would keep it open after it is closed here, so the
    // write would go through.
    let flags = libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC;
    let master = unsafe { libc::posix_openpt(flags) };
    assert!(master >= 0);
    assert_eq!(unsafe { libc::grantpt(master) }, 0);
    assert_eq!(unsafe { libc::unlockpt(master) }, 0);
    let name = unsafe { std::ffi::CStr::from_ptr(libc::ptsname(master)) }
        .to_string_lossy()
        .into_owned();
    let path = std::ffi::CString::new(name.clone()).unwrap();
    let slave = unsafe { libc::open(path.as_ptr(), flags) };
    assert!(slave >= 0);
    // The agent still holds its end; the pane's is gone.
    unsafe { libc::close(master) };

    // A child another test forked a moment ago still holds a copy of the
    // pane's end until it gets to exec, and a write in that moment goes
    // through; one made once it has let go fails. None may wait.
    let mut wrote = Ok(true);
    for _ in 0..50 {
        let (tx, rx) = std::sync::mpsc::channel();
        let name = name.clone();
        std::thread::spawn(move || {
            let _ = tx.send(write_dev(std::path::Path::new(&name), b"x"));
        });
        wrote = rx.recv_timeout(std::time::Duration::from_secs(5));
        if wrote != Ok(true) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    unsafe { libc::close(slave) };
    assert_eq!(wrote, Ok(false));
}

/// The turn's reply names the model: a subagent's entry after it and
/// Claude Code's own synthetic notices are not the conversation's, and
/// the thinking entry before the words carries none.
#[test]
fn the_readout_takes_the_model_that_wrote_the_turns_reply() {
    let jsonl = [
        r#"{"type":"assistant","message":{"model":"claude-haiku-4-5","content":[{"type":"text","text":"Earlier."}]}}"#,
        r#"{"type":"user","promptId":"p2","message":{"content":"hi"}}"#,
        r#"{"type":"assistant","message":{"model":"claude-opus-5-5","content":[{"type":"thinking","thinking":""}]}}"#,
        r#"{"type":"assistant","message":{"model":"claude-opus-5-5","content":[{"type":"text","text":"Hi there!\n"}]}}"#,
        r#"{"type":"assistant","isSidechain":true,"message":{"model":"claude-haiku-4-5","content":[{"type":"text","text":"Hi there!"}]}}"#,
        r#"{"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"No response requested."}]}}"#,
        "not json",
    ]
    .join("\n");
    assert_eq!(
        reply_model_in(&jsonl, "Hi there!", Some("p2")).as_deref(),
        Some("claude-opus-5-5")
    );
    assert_eq!(
        reply_model_in(&jsonl, "Hi there!", None).as_deref(),
        Some("claude-opus-5-5")
    );
    assert_eq!(reply_model_in("", "Hi there!", Some("p2")), None);
}

/// Until the reply is written, the transcript's latest words are the
/// previous turn's — possibly under another model, possibly the very
/// same words — and say nothing.
#[test]
fn a_transcript_behind_the_turn_names_no_model() {
    let jsonl = [
        r#"{"type":"user","promptId":"p1","message":{"content":"say hi"}}"#,
        r#"{"type":"assistant","message":{"model":"claude-haiku-4-5","content":[{"type":"text","text":"Hi!"}]}}"#,
    ]
    .join("\n");
    assert_eq!(reply_model_in(&jsonl, "Hello.", Some("p1")), None);
    assert_eq!(reply_model_in(&jsonl, "Hi!", Some("p2")), None);
    let asked =
        jsonl + "\n" + r#"{"type":"user","promptId":"p2","message":{"content":"say hi"}}"#;
    assert_eq!(reply_model_in(&asked, "Hi!", Some("p2")), None);
}

#[test]
fn the_readout_rides_the_hook_sequence() {
    let dir = std::env::temp_dir().join(format!("anterm-readout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let transcript = dir.join("t.jsonl");
    std::fs::write(
        &transcript,
        r#"{"type":"assistant","message":{"model":"claude-opus-5-5","content":[{"type":"text","text":"Done."}]}}"#,
    )
    .unwrap();
    let stdin = |extra: serde_json::Value| {
        let mut payload = serde_json::json!({
            "session_id": "s",
            "permission_mode": "plan",
            "transcript_path": transcript,
        });
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        payload.to_string()
    };
    let readout = |event: &str, stdin: &str| {
        let seq = build_hook_sequence("claude", event, stdin);
        crate::cli_agent::parse_agent_event(
            seq.strip_prefix(b"\x1b]")
                .unwrap()
                .strip_suffix(b"\x07")
                .unwrap(),
        )
        .unwrap()
        .readout
    };
    let stop = readout(
        "stop",
        &stdin(serde_json::json!({
            "last_assistant_message": "Done.",
            "effort": { "level": "xhigh" },
        })),
    );
    let submit = readout("prompt-submit", &stdin(serde_json::json!({})));
    let no_effort = readout(
        "stop",
        &stdin(serde_json::json!({ "last_assistant_message": "Done." })),
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(stop.permission_mode.as_deref(), Some("plan"));
    assert_eq!(stop.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(
        stop.effort.as_deref(),
        Some("xhigh"),
        "the level Claude reports is the one it runs at"
    );
    assert_eq!(
        (submit.model, submit.effort),
        (None, None),
        "before its reply a turn says nothing of the model it runs on"
    );
    assert_eq!(
        no_effort.effort.as_deref(),
        Some(""),
        "a turn that ends without a level ran on a model that takes none"
    );
}

#[test]
fn a_model_the_catalog_lists_without_effort_levels_takes_none() {
    let catalog = serde_json::json!({
        "version": 2,
        "catalog": { "config": { "models": [
            {
                "id": "claude-opus-5-5",
                "thinking": {
                    "type": "effort",
                    "effort_options": [{ "id": "low" }, { "id": "high" }],
                },
            },
            { "id": "claude-haiku-4-5-20251001", "thinking": { "type": "none" } },
        ] } },
    });
    let catalogs = [catalog];
    assert!(catalog_says_no_effort(
        &catalogs,
        "claude-haiku-4-5-20251001"
    ));
    assert!(!catalog_says_no_effort(&catalogs, "claude-opus-5-5"));
    assert!(!catalog_says_no_effort(&catalogs, "claude-opus-5-5[1m]"));
    assert!(
        !catalog_says_no_effort(&catalogs, "claude-new-model"),
        "a model the catalog does not know may take a level"
    );
    assert!(!catalog_says_no_effort(&[], "claude-haiku-4-5-20251001"));
}

#[test]
fn an_effort_set_for_the_model_wins_over_the_general_one() {
    let settings = serde_json::json!({
        "effortLevel": "high",
        "modelSettings": { "claude-opus-5-5": { "effortLevel": "medium" } },
    });
    assert_eq!(
        configured_effort(&settings, Some("claude-opus-5-5")).as_deref(),
        Some("medium")
    );
    assert_eq!(
        configured_effort(&settings, Some("claude-opus-5-5[1m]")).as_deref(),
        Some("medium")
    );
    assert_eq!(
        configured_effort(&settings, Some("claude-sonnet-5-5")).as_deref(),
        Some("high")
    );
    assert_eq!(configured_effort(&settings, None).as_deref(), Some("high"));
    assert_eq!(configured_effort(&serde_json::json!({}), None), None);
}

#[test]
fn other_agents_carry_no_readout() {
    let stdin = r#"{"session_id":"s","permission_mode":"plan"}"#;
    let seq = build_hook_sequence("codex", "stop", stdin);
    let ev = crate::cli_agent::parse_agent_event(
        seq.strip_prefix(b"\x1b]")
            .unwrap()
            .strip_suffix(b"\x07")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(ev.readout, Default::default());
}

/// The exhaustive match keeps every detected agent mapped; this keeps the
/// other direction honest, so a hooked agent cannot become unreachable
/// from detection and silently stop being diagnosed.
#[test]
fn every_hooked_agent_is_reachable_from_detection() {
    for hooked in HookAgent::ALL {
        assert!(
            CLIAgent::ALL
                .into_iter()
                .any(|detected| HookAgent::of_detected(detected) == Some(hooked)),
            "{hooked:?} has hooks but no detected agent maps to it"
        );
    }
}

#[test]
fn every_anterm_daemon_host_takes_the_console_fast_path() {
    for name in ["anterm-app.exe", "anterm-server.exe", "anterm.exe"] {
        assert!(is_anterm_host_exe(name), "{name} hosts anterm shells");
    }
    for name in ["explorer.exe", "cmd.exe", "anterm", "anterm-app", "wt.exe"] {
        assert!(!is_anterm_host_exe(name), "{name} is not a anterm host process");
    }
}

#[test]
fn hook_sequence_round_trips_through_the_daemon_parser() {
    use crate::cli_agent::{AgentEventKind, CLIAgent, parse_agent_event};

    let seq = build_hook_sequence(
        "claude",
        "notification",
        r#"{"session_id":"abc-123","message":"Claude needs your permission","cwd":"/w"}"#,
    );
    let payload = &seq[2..seq.len() - 1];
    let ev = parse_agent_event(payload).expect("daemon parses the emitted event");
    assert_eq!(ev.agent, Some(CLIAgent::Claude));
    assert_eq!(ev.kind, AgentEventKind::Notification);
    assert_eq!(ev.session_id.as_deref(), Some("abc-123"));
    assert!(ev.message.as_deref().unwrap().contains("permission"));
    assert_eq!(ev.cwd.as_deref(), Some(std::path::Path::new("/w")));

    let seq = build_hook_sequence("claude", "stop", "not json at all");
    let ev = parse_agent_event(&seq[2..seq.len() - 1]).expect("bare event still parses");
    assert_eq!(ev.kind, AgentEventKind::Stop);
    assert_eq!(ev.session_id, None);

    let seq = build_hook_sequence(
        "grok",
        "session-start",
        r#"{"hookEventName":"session_start","sessionId":"g-42","cwd":"/w"}"#,
    );
    let ev = parse_agent_event(&seq[2..seq.len() - 1]).expect("daemon parses the grok event");
    assert_eq!(ev.agent, Some(CLIAgent::Grok));
    assert_eq!(ev.session_id.as_deref(), Some("g-42"));
    assert_eq!(ev.cwd.as_deref(), Some(std::path::Path::new("/w")));
}

/// Parses a built sequence back the way the terminal's scanner does.
fn round_trip(
    agent: &str,
    event: &str,
    stdin_json: &str,
) -> crate::cli_agent::AgentEvent {
    let seq = build_hook_sequence(agent, event, stdin_json);
    crate::cli_agent::parse_agent_event(&seq[2..seq.len() - 1]).expect("parses")
}

/// What the user typed never rides along: nothing downstream reads it,
/// and a pasted prompt made the sequence long enough for the agent's own
/// output to land in the middle of it.
#[test]
fn a_submitted_prompt_stays_out_of_the_sequence() {
    let prompt = "x".repeat(64 * 1024);
    let seq = build_hook_sequence(
        "claude",
        "prompt-submit",
        &serde_json::json!({ "prompt": prompt, "session_id": "s-1" }).to_string(),
    );
    assert!(!String::from_utf8_lossy(&seq).contains("xxxx"));
    assert!(seq.len() < 200, "{} bytes", seq.len());
    let ev = round_trip(
        "claude",
        "prompt-submit",
        r#"{"prompt":"hi","session_id":"s-1"}"#,
    );
    assert_eq!(
        ev.kind,
        crate::cli_agent::AgentEventKind::PromptSubmit
    );
    assert_eq!(ev.message, None, "a prompt is not a message");
}

#[test]
fn a_long_message_is_cut_on_a_character_boundary() {
    let ev = round_trip(
        "gemini",
        "permission-request",
        &serde_json::json!({ "message": "允许".repeat(1000) }).to_string(),
    );
    assert_eq!(ev.message.map(|m| m.chars().count()), Some(MESSAGE_MAX));
    assert_eq!(clip_chars("short", MESSAGE_MAX), "short");
}

#[test]
fn grok_run_hooks_are_relabeled_to_grok() {
    assert_eq!(effective_agent("claude", true), "grok");
    assert_eq!(effective_agent("grok", true), "grok");
    assert_eq!(effective_agent("claude", false), "claude");
    assert_eq!(effective_agent("grok", false), "grok");
}

#[test]
fn tool_start_only_speaks_for_question_tools() {
    for (agent, tool) in [
        ("claude", "AskUserQuestion"),
        ("codex", "request_user_input"),
    ] {
        let input = format!(r#"{{"tool_name":"{tool}","tool_input":{{}}}}"#);
        assert_eq!(
            effective_event(agent, "tool-start", &input),
            Some("question-asked"),
            "{agent} {tool}"
        );
    }
    for input in [
        r#"{"tool_name":"Bash"}"#,
        r#"{"tool_name":"shell"}"#,
        "{}",
        "not json",
    ] {
        assert_eq!(
            effective_event("claude", "tool-start", input),
            None,
            "{input}"
        );
    }
}

#[test]
fn claude_and_codex_report_permission_prompts_first_hand() {
    for events in [CLAUDE_HOOK_EVENTS, CODEX_HOOK_EVENTS] {
        for (hook, sentinel) in [
            ("PermissionRequest", "permission-request"),
            ("PreToolUse", "tool-start"),
            ("PostToolUse", "tool-complete"),
        ] {
            assert!(events.contains(&(hook, sentinel)), "{hook}");
        }
    }
}

#[test]
fn every_installed_event_parses_as_a_sentinel_kind() {
    use crate::cli_agent::parse_agent_event;

    let mut events: Vec<&str> = CLAUDE_HOOK_EVENTS
        .iter()
        .chain(CODEX_HOOK_EVENTS)
        .chain(TRAE_CODE_HOOK_EVENTS)
        .chain(GEMINI_HOOK_EVENTS)
        .chain(DROID_HOOK_EVENTS)
        .chain(QODER_HOOK_EVENTS)
        .chain(QWEN_HOOK_EVENTS)
        .chain(GOOSE_HOOK_EVENTS)
        .chain(KIMI_HOOK_EVENTS)
        .chain(CRUSH_HOOK_EVENTS)
        .chain(CODEBUDDY_HOOK_EVENTS)
        .chain(CURSOR_HOOK_EVENTS)
        .map(|(_, e)| *e)
        .chain(GROK_HOOK_EVENTS.iter().map(|(_, e, _)| *e))
        // Never sent as itself: `effective_event` turns it into a real
        // kind or drops it (`tool_start_only_speaks_for_question_tools`).
        .filter(|e| *e != "tool-start")
        .collect();
    events.extend([
        "prompt-submit",
        "permission-request",
        "stop",
        "session-end",
        "session-start",
    ]);
    for event in events {
        let seq = build_hook_sequence("codex", event, "{}");
        let ev = parse_agent_event(&seq[2..seq.len() - 1])
            .unwrap_or_else(|| panic!("event {event:?} must parse"));
        let kind_json = serde_json::to_value(ev.kind).unwrap();
        assert_eq!(kind_json, serde_json::Value::String(event.to_string()));
    }
}

#[test]
fn a_remote_hook_names_the_server_its_own_installer_placed() {
    let host = FakeRemote::shared();
    let dialect = crate::daemon::install::RemoteProtocol::of_this_build();
    let name = crate::daemon::install::asset::binary_name(dialect.control, dialect.protocol);

    let unix = HookTarget::remote(&*host, PathBuf::from("/home/me"));
    assert_eq!(
        unix.exe,
        PathBuf::from(format!("/home/me/.local/share/anterm/bin/{name}"))
    );

    let windows = HookTarget::remote(&*host, PathBuf::from(r"C:\Users\me"));
    assert_eq!(
        windows.exe,
        PathBuf::from(format!(r"C:\Users\me\AppData\Local\anterm\bin\{name}.exe"))
    );
}

#[test]
fn the_new_hook_agents_target_the_paths_their_clis_read() {
    let host = FakeRemote::shared();
    let t = HookTarget::remote(&*host, PathBuf::from("/home/me"));

    for (agent, want) in [
        (HookAgent::Gemini, "/home/me/.gemini/settings.json"),
        (HookAgent::Droid, "/home/me/.factory/settings.json"),
        (HookAgent::Qwen, "/home/me/.qwen/settings.json"),
        (HookAgent::TraeCode, "/home/me/.trae/cli/hooks.json"),
        (
            HookAgent::Goose,
            "/home/me/.agents/plugins/anterm/hooks/hooks.json",
        ),
        (HookAgent::Kimi, "/home/me/.kimi-code/config.toml"),
        (HookAgent::QoderCLI, "/home/me/.qoder/settings.json"),
        (HookAgent::QoderCLICn, "/home/me/.qoder-cn/settings.json"),
        (HookAgent::Crush, "/home/me/.config/crush/crush.json"),
        (HookAgent::CodeBuddy, "/home/me/.codebuddy/settings.json"),
        (HookAgent::Cursor, "/home/me/.cursor/hooks.json"),
        (HookAgent::Empryo, "/home/me/.empryo/hooks.json"),
        (HookAgent::Jcode, "/home/me/.jcode/config.toml"),
        (
            HookAgent::PrimeAgent,
            "/home/me/.prime/agent/extensions/anterm/index.ts",
        ),
    ] {
        assert_eq!(
            agent.target_path(&t),
            PathBuf::from(want),
            "{} writes somewhere its CLI does not read",
            agent.slug()
        );
    }

    let dir = std::env::temp_dir().join(format!("anterm-new-hooks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let real = HookTarget::remote(&*host, dir.clone());

    for agent in [
        HookAgent::Gemini,
        HookAgent::Droid,
        HookAgent::Qwen,
        HookAgent::Goose,
        HookAgent::Kimi,
        HookAgent::QoderCLI,
        HookAgent::QoderCLICn,
        HookAgent::Crush,
        HookAgent::CodeBuddy,
        HookAgent::Cursor,
    ] {
        assert_eq!(hooks_state(&real, agent), HooksState::NotInstalled);
        install_hooks(&real, agent).unwrap_or_else(|e| panic!("{}: {e}", agent.slug()));
        assert_eq!(
            hooks_state(&real, agent),
            HooksState::Installed,
            "{} does not read back what it wrote",
            agent.slug()
        );
        let written = std::fs::read_to_string(agent.target_path(&real)).unwrap();
        assert!(
            written.contains(&format!("agent-hook {}", agent.slug())),
            "{} wrote a config without its own emitter",
            agent.slug()
        );
        uninstall_hooks(&real, agent).unwrap_or_else(|e| panic!("{}: {e}", agent.slug()));
        assert_eq!(hooks_state(&real, agent), HooksState::NotInstalled);
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn qoder_compaction_preserves_the_active_turn() {
    use crate::cli_agent::{AgentSessionState, AgentStatus};

    let mut state = AgentSessionState::default();
    state.apply_event(&round_trip(
        "qodercli",
        "prompt-submit",
        r#"{"session_id":"q-1","cwd":"/repo","prompt":"Continue the task"}"#,
    ));
    let before = state.clone();
    let compact = r#"{"session_id":"q-1","cwd":"/repo","source":"compact"}"#;
    if let Some(event) = effective_event("qodercli", "session-start", compact) {
        state.apply_event(&round_trip("qodercli", event, compact));
    }
    assert_eq!(state, before, "compaction must preserve the active turn");

    state.apply_event(&round_trip("qodercli", "tool-complete", "{}"));
    assert_eq!(state.status, AgentStatus::Working);
    state.apply_event(&round_trip("qodercli", "stop", "{}"));
    assert_eq!(state.status, AgentStatus::Done);

    for input in [
        r#"{"source":"startup","message":"compact"}"#,
        r#"{"source":"resume"}"#,
        r#"{"source":"clear"}"#,
        "{}",
        "not JSON",
    ] {
        let event = effective_event("qodercli", "session-start", input)
            .expect("ordinary session starts still reach the state machine");
        let mut session = before.clone();
        session.apply_event(&round_trip("qodercli", event, input));
        assert_eq!(session.status, AgentStatus::Idle, "{input}");
    }
    assert_eq!(
        effective_event("claude", "session-start", compact),
        Some("session-start"),
        "the filter is specific to Qoder"
    );
    assert_eq!(
        effective_event("qodercli", "prompt-submit", compact),
        Some("prompt-submit")
    );
}

#[test]
fn qoder_mcp_elicitation_waits_for_user_input() {
    use crate::cli_agent::{AgentSessionState, AgentStatus};

    let apply_hook = |state: &mut AgentSessionState, hook: &str, input: &str| {
        let event = HookAgent::QoderCLI
            .hook_map_events()
            .unwrap()
            .iter()
            .find_map(|(name, event)| (*name == hook).then_some(*event))
            .and_then(|event| effective_event("qodercli", event, input));
        if let Some(event) = event {
            state.apply_event(&round_trip("qodercli", event, input));
        }
    };
    let mut state = AgentSessionState::default();
    apply_hook(
        &mut state,
        "UserPromptSubmit",
        r#"{"session_id":"q-1","prompt":"Look up my tickets"}"#,
    );
    assert_eq!(state.status, AgentStatus::Working);
    // Qoder says outright when it is blocked — `PermissionRequest` and
    // `Elicitation` — so it must not also carry `Notification`, which
    // fires for non-blocking alerts and would strand the pane on
    // "waiting". Asserting the map has no seat for it is the check; a
    // `Notification` payload put through `apply_hook` would be dropped
    // for want of one and prove nothing.
    assert!(
        !HookAgent::QoderCLI
            .hook_map_events()
            .unwrap()
            .iter()
            .any(|(hook, _)| *hook == "Notification"),
        "an unblocking alert must not read as a question"
    );

    // The MCP tool is already authorized, so no PermissionRequest precedes
    // its request for more information from the user.
    apply_hook(
        &mut state,
        "Elicitation",
        r#"{
            "session_id":"q-1",
            "hook_event_name":"Elicitation",
            "mcp_server_name":"tickets",
            "message":"Choose a project",
            "mode":"form"
        }"#,
    );
    assert_eq!(state.status, AgentStatus::Waiting);
    assert_eq!(state.message.as_deref(), Some("Choose a project"));
    assert_eq!(state.session_id.as_deref(), Some("q-1"));

    apply_hook(&mut state, "PostToolUse", r#"{"session_id":"q-1"}"#);
    assert_eq!(state.status, AgentStatus::Working);
    assert_eq!(state.message, None);
    apply_hook(&mut state, "Stop", r#"{"session_id":"q-1"}"#);
    assert_eq!(state.status, AgentStatus::Done);
}

/// CodeBuddy reuses Claude Code's hook shape and payload, so a turn walks
/// the same way — but it fires `SessionStart` with `source: "compact"`
/// in the middle of a turn, which must not reset the pane to idle.
#[test]
fn codebuddy_turns_survive_compaction_and_record_the_session() {
    use crate::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

    let apply_hook = |state: &mut AgentSessionState, hook: &str, input: &str| {
        let event = HookAgent::CodeBuddy
            .hook_map_events()
            .unwrap()
            .iter()
            .find_map(|(name, event)| (*name == hook).then_some(*event))
            .unwrap_or_else(|| panic!("CodeBuddy installs no {hook} hook"));
        if let Some(event) = effective_event("codebuddy", event, input) {
            let ev = round_trip("codebuddy", event, input);
            assert_eq!(ev.agent, Some(CLIAgent::CodeBuddy));
            state.apply_event(&ev);
        }
    };
    let mut state = AgentSessionState::default();
    apply_hook(
        &mut state,
        "SessionStart",
        r#"{"session_id":"cb-1","cwd":"/repo","source":"startup"}"#,
    );
    assert_eq!(state.status, AgentStatus::Idle);
    apply_hook(
        &mut state,
        "UserPromptSubmit",
        r#"{"session_id":"cb-1","cwd":"/repo","prompt":"Fix the build"}"#,
    );
    assert_eq!(state.status, AgentStatus::Working);

    let before = state.clone();
    apply_hook(
        &mut state,
        "SessionStart",
        r#"{"session_id":"cb-1","cwd":"/repo","source":"compact"}"#,
    );
    assert_eq!(state, before, "compaction must preserve the active turn");

    apply_hook(
        &mut state,
        "PermissionRequest",
        r#"{"session_id":"cb-1","tool_name":"Bash"}"#,
    );
    assert_eq!(state.status, AgentStatus::Waiting);
    apply_hook(&mut state, "PostToolUse", r#"{"session_id":"cb-1"}"#);
    assert_eq!(state.status, AgentStatus::Working);
    apply_hook(&mut state, "StopFailure", r#"{"session_id":"cb-1"}"#);
    assert_eq!(state.status, AgentStatus::Done, "a failed turn still ends");
    assert_eq!(state.session_id.as_deref(), Some("cb-1"));
    assert_eq!(state.cwd.as_deref(), Some(Path::new("/repo")));

    assert!(
        !HookAgent::CodeBuddy
            .hook_map_events()
            .unwrap()
            .iter()
            .any(|(hook, _)| *hook == "Notification"),
        "PermissionRequest reports the block; Notification would only muddy it"
    );
}

/// Cursor calls the session a conversation, and only its tool events say
/// where they ran. Both still have to reach the pane, or Copy Session ID
/// and resume stay dark.
#[test]
fn cursor_payloads_name_the_session_and_its_workspace() {
    let ev = round_trip(
        "cursor",
        "stop",
        r#"{"conversation_id":"cur-1","generation_id":"g-1","status":"completed",
            "workspace_roots":["/repo","/other"],"hook_event_name":"stop"}"#,
    );
    assert_eq!(ev.agent, Some(CLIAgent::Cursor));
    assert_eq!(ev.session_id.as_deref(), Some("cur-1"));
    assert_eq!(ev.cwd.as_deref(), Some(Path::new("/repo")));

    let ev = round_trip(
        "cursor",
        "prompt-submit",
        r#"{"conversation_id":"cur-1","cwd":"/repo/sub","workspace_roots":["/repo"],
            "tool_name":"Shell"}"#,
    );
    assert_eq!(
        ev.cwd.as_deref(),
        Some(Path::new("/repo/sub")),
        "a real cwd beats the workspace root"
    );

    let ev = round_trip(
        "cursor",
        "session-start",
        r#"{"session_id":"cur-2","conversation_id":"cur-2","workspace_roots":[]}"#,
    );
    assert_eq!(ev.session_id.as_deref(), Some("cur-2"));
    assert_eq!(ev.cwd, None, "an empty root list names no directory");
}

/// A `cursor-agent` turn opens on `beforeSubmitPrompt` and closes on
/// `stop`; tool calls in between only move the activity counter.
#[test]
fn cursor_cli_turns_open_on_submit_and_close_on_stop() {
    use crate::cli_agent::{AgentSessionState, AgentStatus};

    let apply_hook = |state: &mut AgentSessionState, hook: &str, input: &str| {
        let event = HookAgent::Cursor
            .hook_map_events()
            .unwrap()
            .iter()
            .find_map(|(name, event)| (*name == hook).then_some(*event))
            .unwrap_or_else(|| panic!("Cursor installs no {hook} hook"));
        let event = effective_event("cursor", event, input).unwrap();
        state.apply_event(&round_trip("cursor", event, input));
    };
    let mut state = AgentSessionState::default();
    apply_hook(
        &mut state,
        "sessionStart",
        r#"{"session_id":"cur-1","conversation_id":"cur-1","workspace_roots":["/repo"]}"#,
    );
    assert_eq!(state.status, AgentStatus::Idle);
    assert_eq!(state.session_id.as_deref(), Some("cur-1"));

    for turn in 1..=2 {
        apply_hook(
            &mut state,
            "beforeSubmitPrompt",
            r#"{"conversation_id":"cur-1","prompt":"Fix the build"}"#,
        );
        assert_eq!(state.status, AgentStatus::Working, "turn {turn}");
        let before = state.activity;
        apply_hook(
            &mut state,
            "postToolUse",
            r#"{"conversation_id":"cur-1","tool_name":"Shell","cwd":"/repo"}"#,
        );
        assert_eq!(state.status, AgentStatus::Working, "turn {turn}");
        assert_eq!(state.activity, before + 1, "a tool call is activity");
        apply_hook(
            &mut state,
            "stop",
            r#"{"conversation_id":"cur-1","status":"completed"}"#,
        );
        assert_eq!(state.status, AgentStatus::Done, "turn {turn}");
        assert_eq!(state.turns, turn, "each turn is counted once");
    }

    // A turn that answers without a tool still shows as working.
    apply_hook(
        &mut state,
        "beforeSubmitPrompt",
        r#"{"conversation_id":"cur-1","prompt":"Explain this"}"#,
    );
    assert_eq!(state.status, AgentStatus::Working);
    apply_hook(
        &mut state,
        "stop",
        r#"{"conversation_id":"cur-1","status":"aborted"}"#,
    );
    assert_eq!(state.status, AgentStatus::Done);

    apply_hook(
        &mut state,
        "sessionEnd",
        r#"{"session_id":"cur-1","conversation_id":"cur-1","reason":"user_close"}"#,
    );
    assert_eq!(state.status, AgentStatus::Idle);
    assert_eq!(state.session_id.as_deref(), Some("cur-1"));
}

/// A Pi bridge written before Pi changed its package name is ours but
/// stale, so launch's refresh rewrites it with the current import.
#[test]
fn a_pi_bridge_importing_the_old_package_is_refreshed() {
    let host = FakeRemote::shared();
    let base = std::env::temp_dir().join(format!("anterm-pi-bridge-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let target = HookTarget::remote(&*host, base.clone());
    let path = HookAgent::Pi.target_path(&target);
    let current = pi_extension_ts(&target, HookAgent::Pi).expect("pi bridge builds");
    let old = current.replace(
        "@earendil-works/pi-coding-agent",
        "@mariozechner/pi-coding-agent",
    );
    assert_ne!(old, current);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &old).unwrap();

    assert_eq!(hooks_state(&target, HookAgent::Pi), HooksState::Outdated);
    assert_eq!(refresh_hooks(&target), 1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), current);
    assert_eq!(hooks_state(&target, HookAgent::Pi), HooksState::Installed);

    let _ = std::fs::remove_dir_all(&base);
}

/// Cursor's `hooks.json` is flat like Crush's, but it is also ignored
/// outright without `"version": 1` at the root — so a file anterm creates
/// has to carry one, and a user's own file keeps whatever it declares.
#[test]
fn cursor_hooks_file_is_versioned_flat_and_preserves_user_entries() {
    let host = FakeRemote::shared();
    let base = std::env::temp_dir().join(format!("anterm-cursor-hooks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let target = HookTarget::remote(&*host, base.clone());
    let config = HookAgent::Cursor.target_path(&target);
    let read = || -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap()
    };

    // From nothing: anterm creates the file, so it declares the version.
    install_hooks(&target, HookAgent::Cursor).expect("install succeeds");
    assert_eq!(
        hooks_state(&target, HookAgent::Cursor),
        HooksState::Installed
    );
    let created = read();
    assert_eq!(created["version"], 1);
    for (hook, event) in CURSOR_HOOK_EVENTS {
        let entries = created["hooks"][*hook].as_array().unwrap();
        assert_eq!(entries.len(), 1, "{hook}");
        assert_eq!(
            entries[0],
            serde_json::json!({ "command": target.hook_command(HookAgent::Cursor, event) }),
            "{hook} is one flat entry naming its emitter"
        );
    }
    uninstall_hooks(&target, HookAgent::Cursor).unwrap();
    assert_eq!(
        hooks_state(&target, HookAgent::Cursor),
        HooksState::NotInstalled
    );

    // A user's file with hooks of their own and a version already set.
    let user_config = serde_json::json!({
        "version": 1,
        "hooks": {
            "stop": [{ "command": "./audit.sh", "timeout": 5 }],
            "beforeShellExecution": [{ "command": "./guard.sh", "matcher": "rm" }]
        }
    });
    std::fs::write(&config, user_config.to_string()).unwrap();
    install_hooks(&target, HookAgent::Cursor).unwrap();
    install_hooks(&target, HookAgent::Cursor).expect("re-install succeeds");
    let merged = read();
    let stop = merged["hooks"]["stop"].as_array().unwrap();
    assert_eq!(
        stop.len(),
        2,
        "one anterm entry beside the user's after two installs"
    );
    assert_eq!(stop[0], user_config["hooks"]["stop"][0]);
    assert_eq!(
        merged["hooks"]["beforeShellExecution"], user_config["hooks"]["beforeShellExecution"],
        "a permission hook anterm does not install is left alone"
    );
    assert_eq!(
        uninstall_hooks(&target, HookAgent::Cursor).unwrap(),
        HookOutcome::Removed
    );
    assert_eq!(read(), user_config, "uninstall restores the user's file");

    // Hooks written into a file Cursor would ignore are not installed yet.
    let mut unversioned = serde_json::json!({ "hooks": {} });
    for (hook, event) in CURSOR_HOOK_EVENTS {
        unversioned["hooks"][*hook] = serde_json::json!([
            { "command": target.hook_command(HookAgent::Cursor, event) }
        ]);
    }
    std::fs::write(&config, unversioned.to_string()).unwrap();
    assert_eq!(
        hooks_state(&target, HookAgent::Cursor),
        HooksState::Outdated
    );
    assert_eq!(refresh_hooks(&target), 1);
    assert_eq!(read()["version"], 1);
    assert_eq!(
        hooks_state(&target, HookAgent::Cursor),
        HooksState::Installed
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// Gemini shows a running hook by its `name`, else by its whole command,
/// so anterm's entries carry a short one — and an install from before they
/// did reads as outdated, for the launch-time refresh to name it.
#[test]
fn gemini_hooks_carry_a_short_name_and_unnamed_ones_are_refreshed() {
    let host = FakeRemote::shared();
    let base = std::env::temp_dir().join(format!("anterm-gemini-names-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let target = HookTarget::remote(&*host, base.clone());
    let config = HookAgent::Gemini.target_path(&target);
    let read = || -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap()
    };

    install_hooks(&target, HookAgent::Gemini).expect("install succeeds");
    for (hook, event) in GEMINI_HOOK_EVENTS {
        assert_eq!(
            read()["hooks"][*hook][0]["hooks"][0],
            serde_json::json!({
                "type": "command",
                "command": target.hook_command(HookAgent::Gemini, event),
                "name": "anterm",
            }),
            "{hook}"
        );
    }

    // What an earlier anterm wrote: the same commands, no name.
    let mut unnamed = serde_json::json!({ "ui": { "theme": "Default" }, "hooks": {} });
    for (hook, event) in GEMINI_HOOK_EVENTS {
        unnamed["hooks"][*hook] = serde_json::json!([{
            "hooks": [{
                "type": "command",
                "command": target.hook_command(HookAgent::Gemini, event),
            }]
        }]);
    }
    std::fs::write(&config, unnamed.to_string()).unwrap();
    assert_eq!(
        hooks_state(&target, HookAgent::Gemini),
        HooksState::Outdated
    );
    assert_eq!(refresh_hooks(&target), 1);
    let refreshed = read();
    assert_eq!(refreshed["ui"], unnamed["ui"]);
    for (hook, _) in GEMINI_HOOK_EVENTS {
        let entries = refreshed["hooks"][*hook].as_array().unwrap();
        assert_eq!(entries.len(), 1, "{hook}: replaced, not doubled");
        assert_eq!(entries[0]["hooks"][0]["name"], "anterm", "{hook}");
    }
    assert_eq!(
        hooks_state(&target, HookAgent::Gemini),
        HooksState::Installed
    );

    // Nobody else's entries get a name, and a named one of theirs is no
    // reason to call Claude's hooks outdated.
    install_hooks(&target, HookAgent::Claude).unwrap();
    let claude = HookAgent::Claude.target_path(&target);
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&claude).unwrap()).unwrap();
    assert!(
        written["hooks"]["Stop"][0]["hooks"][0]
            .get("name")
            .is_none()
    );
    assert_eq!(
        hooks_state(&target, HookAgent::Claude),
        HooksState::Installed
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// Crush's lone `PreToolUse` has no `Stop` to close a turn behind it, so it
/// must not open one: the pane would read as busy until Crush exits.
#[test]
fn crush_tool_calls_record_the_session_without_starting_a_turn() {
    use crate::cli_agent::{AgentSessionState, AgentStatus};

    let mut state = AgentSessionState::default();
    for (hook, event) in HookAgent::Crush.hook_map_events().unwrap() {
        assert_eq!(*hook, "PreToolUse");
        let input =
            r#"{"event":"PreToolUse","session_id":"c-1","cwd":"/repo","tool_name":"bash"}"#;
        let event = effective_event("crush", event, input).unwrap();
        state.apply_event(&round_trip("crush", event, input));
    }
    assert_eq!(state.status, AgentStatus::Idle);
    assert_eq!(state.session_id.as_deref(), Some("c-1"));
    assert_eq!(state.cwd.as_deref(), Some(Path::new("/repo")));
    assert_eq!(state.activity, 1);
}

/// Qwen is the one agent that reports a blocked turn outright, so it must
/// not also carry the `Notification` hook the others need — that event fires
/// for non-blocking alerts too and would strand the pane on "waiting".
#[test]
fn qwen_reports_permission_requests_natively() {
    assert!(
        QWEN_HOOK_EVENTS
            .iter()
            .any(|(hook, anterm)| *hook == "PermissionRequest" && *anterm == "permission-request")
    );
    assert!(
        !QWEN_HOOK_EVENTS
            .iter()
            .any(|(hook, _)| *hook == "Notification")
    );
    assert_eq!(
        effective_event("qwen", "permission-request", "{}"),
        Some("permission-request")
    );
}

#[test]
fn gemini_and_droid_notifications_filter_to_permission_requests() {
    assert_eq!(
        effective_event(
            "gemini",
            "notification",
            r#"{"notification_type":"ToolPermission"}"#
        ),
        Some("permission-request")
    );
    assert_eq!(
        effective_event(
            "droid",
            "notification",
            r#"{"notification_type":"permission_prompt"}"#
        ),
        Some("permission-request")
    );
    // A non-blocking alert must not strand the pane on "waiting".
    for agent in ["gemini", "droid"] {
        assert_eq!(
            effective_event(
                agent,
                "notification",
                r#"{"notification_type":"auth_success"}"#
            ),
            None,
            "{agent} reported an idle notification as a block"
        );
    }
}

#[test]
fn uninstalling_goose_takes_its_generated_plugin_dirs_with_it() {
    let root = std::env::temp_dir().join(format!("anterm-goose-test-{}", std::process::id()));
    let plugins = root.join("plugins");
    let plugin = plugins.join("anterm");
    let hooks_dir = plugin.join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let path = hooks_dir.join("hooks.json");

    let host = local_host();
    let t = HookTarget::local(&*host).expect("home resolves in tests");
    let content = goose_hooks_json(&t).expect("goose content builds");
    assert!(content.contains("agent-hook goose"));
    let marker = "agent-hook goose";

    owned_file_install(&t, &path, &content, marker).expect("install");
    owned_file_uninstall(&t, &path, marker).expect("uninstall");

    assert!(!path.exists());
    assert!(!hooks_dir.exists(), "the generated hooks/ dir goes too");
    assert!(!plugin.exists(), "and the anterm plugin dir above it");
    assert!(plugins.exists(), "but never the shared plugins/ dir");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn copilot_notifications_filter_to_permission_requests() {
    assert_eq!(
        effective_event("copilot", "notification", r#"{"type":"permission_prompt"}"#),
        Some("permission-request")
    );
    assert_eq!(
        effective_event(
            "copilot",
            "notification",
            r#"{"type":"elicitation_dialog"}"#
        ),
        Some("permission-request")
    );
    assert_eq!(
        effective_event("copilot", "notification", r#"{"type":"turn_summary"}"#),
        None
    );
    assert_eq!(
        effective_event(
            "grok",
            "notification",
            r#"{"notificationType":"elicitation_dialog","message":"User question requested"}"#
        ),
        Some("permission-request")
    );
    for noisy in ["permission_prompt", "task_complete", "agent_error"] {
        assert_eq!(
            effective_event(
                "grok",
                "notification",
                &format!(r#"{{"notificationType":"{noisy}"}}"#)
            ),
            None,
            "grok {noisy} is not a block"
        );
    }
    assert_eq!(
        effective_event("claude", "notification", "{}"),
        Some("notification")
    );
    assert_eq!(effective_event("copilot", "stop", "{}"), Some("stop"));
    assert_eq!(effective_event("grok", "stop", "{}"), Some("stop"));
}

#[cfg(unix)]
#[test]
fn ancestor_tty_device_is_none_or_a_dev_path() {
    match ancestor_tty_device() {
        None => {}
        Some(dev) => assert!(
            dev.starts_with("/dev/"),
            "a resolved tty must be an openable device path, got {dev:?}"
        ),
    }
}

#[test]
fn marker_detection_matches_our_entries_only() {
    let ours = serde_json::json!({
        "hooks": [{ "type": "command", "command": "\"/x/anterm\" agent-hook claude stop" }]
    });
    assert!(marker_command(&ours, "agent-hook claude").is_some());
    assert!(marker_command(&ours, "agent-hook codex").is_none());
    let theirs = serde_json::json!({
        "hooks": [{ "type": "command", "command": "afplay /System/Library/Sounds/Glass.aiff" }]
    });
    assert!(marker_command(&theirs, "agent-hook claude").is_none());
    assert!(marker_command(&serde_json::json!({}), "agent-hook claude").is_none());

    // Crush flattens the same entry: `command` sits beside `matcher` rather
    // than inside a nested `hooks` array, and the reader has to see it.
    let flat = serde_json::json!({
        "matcher": "^bash$",
        "command": "\"/x/anterm\" agent-hook crush prompt-submit"
    });
    assert_eq!(
        marker_command(&flat, "agent-hook crush"),
        Some("\"/x/anterm\" agent-hook crush prompt-submit")
    );
    assert!(marker_command(&flat, "agent-hook claude").is_none());
}

fn local_host() -> crate::host::SharedHost {
    crate::host::local::LocalHost::new()
}

struct FakeRemote(crate::host::SharedHost);

impl FakeRemote {
    fn shared() -> crate::host::SharedHost {
        std::sync::Arc::new(FakeRemote(local_host()))
    }
}

impl Host for FakeRemote {
    fn id(&self) -> crate::host::HostId {
        crate::host::HostId::from_connection_key("ssh-direct:me@box:22")
    }
    fn separator(&self) -> char {
        '/'
    }
    fn is_absolute(&self, p: &Path) -> bool {
        p.to_string_lossy().starts_with('/')
    }
    fn read_dir(&self, dir: &Path, root: Option<&Path>) -> io::Result<Vec<crate::host::Entry>> {
        self.0.read_dir(dir, root)
    }
    fn stat(&self, p: &Path) -> io::Result<crate::host::Meta> {
        self.0.stat(p)
    }
    fn read_file(&self, p: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
        self.0.read_file(p, max_bytes)
    }
    fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
        self.0.canonicalize(p)
    }
    fn search(
        &self,
        roots: &[PathBuf],
        query: &str,
        limit: usize,
        max_dirs: usize,
        show_hidden: bool,
    ) -> io::Result<Vec<crate::host::SearchHit>> {
        self.0.search(roots, query, limit, max_dirs, show_hidden)
    }
    fn search_content(
        &self,
        roots: &[PathBuf],
        query: &crate::host::ContentQuery,
        limits: &crate::host::ContentLimits,
    ) -> io::Result<crate::host::ContentResults> {
        self.0.search_content(roots, query, limits)
    }
    fn write_file(&self, p: &Path, bytes: &[u8]) -> io::Result<crate::host::Meta> {
        self.0.write_file(p, bytes)
    }
    fn create_file_new(&self, p: &Path) -> io::Result<()> {
        self.0.create_file_new(p)
    }
    fn create_dir(&self, p: &Path, recursive: bool) -> io::Result<()> {
        self.0.create_dir(p, recursive)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.0.rename(from, to)
    }
    fn remove(&self, p: &Path, recursive: bool) -> io::Result<()> {
        self.0.remove(p, recursive)
    }
    fn repo_root(&self, p: &Path) -> io::Result<Option<PathBuf>> {
        self.0.repo_root(p)
    }
    fn git(&self, cwd: &Path, args: &[&str]) -> io::Result<crate::host::Output> {
        self.0.git(cwd, args)
    }
    fn shells(&self) -> io::Result<crate::host::ShellInventory> {
        self.0.shells()
    }
    fn watch(&self, dirs: &[PathBuf]) -> io::Result<crate::host::WatchSub> {
        self.0.watch(dirs)
    }
}

#[test]
fn hook_command_quotes_the_exe_path() {
    let host = local_host();
    let target = HookTarget::local(&*host).expect("home resolves in tests");
    let cmd = target.hook_command(HookAgent::Claude, "stop");
    // On Windows the executable may be emitted as a bare PATH-resolvable
    // name: PowerShell cannot invoke a quoted path without the `&` call
    // operator, so quoting is avoided whenever possible.
    #[cfg(not(windows))]
    assert!(cmd.starts_with('"'));
    assert!(cmd.ends_with("agent-hook claude stop"));
}

#[test]
fn remote_paths_are_built_in_the_remote_machine_s_spelling() {
    let host = FakeRemote::shared();
    let target = HookTarget::remote(&*host, PathBuf::from("/home/me"));

    for (agent, expected) in [
        (HookAgent::Claude, "/home/me/.claude/settings.json"),
        (HookAgent::Codex, "/home/me/.codex/hooks.json"),
        (HookAgent::TraeCode, "/home/me/.trae/cli/hooks.json"),
        (HookAgent::Copilot, "/home/me/.copilot/hooks/anterm.json"),
        (
            HookAgent::OpenCode,
            "/home/me/.config/opencode/plugins/anterm.js",
        ),
        (HookAgent::Pi, "/home/me/.pi/agent/extensions/anterm/index.ts"),
        (HookAgent::Grok, "/home/me/.grok/hooks/anterm.json"),
        (
            HookAgent::OhMyPi,
            "/home/me/.omp/agent/extensions/anterm/index.ts",
        ),
        (HookAgent::Kimi, "/home/me/.kimi-code/config.toml"),
        (HookAgent::QoderCLI, "/home/me/.qoder/settings.json"),
        (HookAgent::QoderCLICn, "/home/me/.qoder-cn/settings.json"),
        (HookAgent::Crush, "/home/me/.config/crush/crush.json"),
        (HookAgent::CodeBuddy, "/home/me/.codebuddy/settings.json"),
        (HookAgent::Cursor, "/home/me/.cursor/hooks.json"),
        (HookAgent::Empryo, "/home/me/.empryo/hooks.json"),
        (HookAgent::Jcode, "/home/me/.jcode/config.toml"),
    ] {
        assert_eq!(
            agent.target_path(&target),
            PathBuf::from(expected),
            "{agent:?} target path"
        );
        assert_eq!(
            agent.target_display(&target),
            expected.replacen("/home/me/", "~/", 1),
            "{agent:?} display path"
        );
    }
}

#[test]
fn the_hook_command_names_the_binary_on_that_machine() {
    let host = FakeRemote::shared();
    let target = HookTarget::remote(&*host, PathBuf::from("/home/me"));
    let dialect = crate::daemon::install::RemoteProtocol::of_this_build();
    let name = format!("anterm-server-c{}p{}", dialect.control, dialect.protocol);
    assert_eq!(
        target.hook_command(HookAgent::Claude, "stop"),
        format!("\"/home/me/.local/share/anterm/bin/{name}\" agent-hook claude stop")
    );

    let local = local_host();
    let here = HookTarget::local(&*local).expect("home resolves in tests");
    let exe = std::env::current_exe().unwrap();
    let command_exe = here
        .hook_command_exe()
        .unwrap_or_else(|| format!("\"{}\"", exe.display()));
    assert_eq!(
        here.hook_command(HookAgent::Claude, "stop"),
        format!("{command_exe} agent-hook claude stop")
    );
}

fn installed_muse_fixture(target: &HookTarget) -> serde_json::Value {
    let mut hooks = muse_plugin_manifest(target)["capabilities"]["hooks"].clone();
    for hook in hooks.as_array_mut().unwrap() {
        let timeout = hook.as_object_mut().unwrap().remove("timeoutMs").unwrap();
        hook["timeout_ms"] = timeout;
    }
    let runtime: Vec<_> = MUSE_HOOK_EVENTS
        .iter()
        .map(|(id, _, _)| {
            serde_json::json!({
                "candidate": {"kind": "hook", "capability_id": id}, "status": "trusted_enabled"
            })
        })
        .collect();
    serde_json::json!({"active": true, "valid": true, "plugin": {"id": MUSE_PLUGIN_ID,
        "capabilities": {"hooks": hooks}}, "runtime_capabilities": runtime})
}

#[test]
fn muse_install_uses_native_cli_and_requires_approved_hooks() {
    let host = local_host();
    let dir = std::env::temp_dir().join(format!("anterm-muse-install-{}", std::process::id()));
    let target = HookTarget {
        host: &*host,
        home: dir.clone(),
        exe: PathBuf::from("/anterm bin/anterm-app"),
    };
    let path = HookAgent::Muse.target_path(&target);
    let fixture = installed_muse_fixture(&target);
    let mut calls = Vec::new();
    let outcome = muse_install_with(&target, &path, |args| {
        calls.push(args.to_vec());
        Ok(if args[0] == "inspect" {
            fixture.clone()
        } else {
            serde_json::json!({})
        })
    })
    .unwrap();
    assert_eq!(outcome, HookOutcome::Installed);
    let bundle = path
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        calls,
        vec![
            vec![
                "install".to_owned(),
                bundle,
                "--scope".into(),
                "user".into()
            ],
            vec!["approve".into(), "anterm-presence".into()],
            vec!["enable".into(), "anterm-presence".into()],
            vec!["inspect".into(), "anterm-presence".into()],
        ]
    );
    assert_eq!(
        muse_inspected_state(&target, &fixture),
        HooksState::Installed
    );
    let mut unapproved = fixture.clone();
    unapproved["runtime_capabilities"][0]["status"] = "needs_review".into();
    assert_eq!(
        muse_inspected_state(&target, &unapproved),
        HooksState::Outdated
    );
    assert!(
        muse_install_with(&target, &path, |args| {
            if args[0] == "approve" {
                anyhow::bail!("approval failed");
            }
            Ok(serde_json::json!({}))
        })
        .is_err(),
        "a failed approval is not reported as installed"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn muse_remote_bundle_requires_manual_install_and_preserves_session_ids() {
    let host = FakeRemote::shared();
    let dir = std::env::temp_dir().join(format!("anterm-muse-remote-{}", std::process::id()));
    let target = HookTarget::remote(&*host, dir.clone());
    let path = HookAgent::Muse.target_path(&target);
    let outcome = muse_install_with(&target, &path, |_| {
        panic!("must not run local Muse for a remote")
    })
    .unwrap();
    let HookOutcome::MuseInstallManually(command) = outcome else {
        panic!("manual installation required");
    };
    assert!(command.contains("muse plugins install"));
    assert!(command.contains("muse plugins approve anterm-presence"));
    assert_eq!(
        hooks_state(&target, HookAgent::Muse),
        HooksState::NotInstalled
    );
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let hooks = manifest["capabilities"]["hooks"].as_array().unwrap();
    assert_eq!(hooks.len(), 8);
    let interrupt = hooks
        .iter()
        .find(|hook| hook["event"] == "Interrupt")
        .unwrap();
    assert_eq!(
        interrupt["async"], true,
        "Muse admits only asynchronous cancellation hooks"
    );
    let permission = hooks
        .iter()
        .find(|hook| hook["event"] == "PermissionRequest")
        .unwrap();
    assert_eq!(permission["command"][3], "permission-request");
    let event = round_trip(
        "muse",
        "session-start",
        r#"{"session_id":"muse-session-42","cwd":"/workspace"}"#,
    );
    assert_eq!(event.agent, Some(CLIAgent::Muse));
    assert_eq!(event.session_id.as_deref(), Some("muse-session-42"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empryo_hooks_merge_with_existing_lifecycle_hooks() {
    let dir = std::env::temp_dir().join(format!("anterm-empryo-hooks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let host = FakeRemote::shared();
    let target = HookTarget::remote(&*host, dir.clone());
    let path = HookAgent::Empryo.target_path(&target);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"hooks":{"SessionStart":[]}}"#).unwrap();
    assert_eq!(
        hooks_state(&target, HookAgent::Empryo),
        HooksState::NotInstalled
    );
    install_hooks(&target, HookAgent::Empryo).expect("install succeeds");
    assert_eq!(
        hooks_state(&target, HookAgent::Empryo),
        HooksState::Installed
    );
    let root: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(root["hooks"]["SessionStart"].as_array().unwrap().len() >= 1);
    assert!(
        root["hooks"]["SessionEnd"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("agent-hook empryo session-end")
    );
    install_hooks(&target, HookAgent::Empryo).expect("reinstall succeeds");
    assert_eq!(
        hooks_state(&target, HookAgent::Empryo),
        HooksState::Installed
    );
    assert_eq!(
        uninstall_hooks(&target, HookAgent::Empryo).unwrap(),
        HookOutcome::Removed
    );
    assert_eq!(
        uninstall_hooks(&target, HookAgent::Empryo).unwrap(),
        HookOutcome::NoAntermHooks
    );
    assert!(path.exists(), "uninstall preserves the shared config file");
    let root: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(root, serde_json::json!({"hooks": {}}));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_jcode_report_reads_its_session_from_the_environment() {
    let env = |k: &str| {
        (k == "JCODE_HOOK_PAYLOAD").then(|| {
            r#"{"event":"session_start","session_id":"j-7","cwd":"/work"}"#.to_string()
        })
    };
    let input = hook_payload("jcode", String::new(), env);
    let event = round_trip("jcode", "session-start", &input);
    assert_eq!(event.agent, Some(CLIAgent::Jcode));
    assert_eq!(event.session_id.as_deref(), Some("j-7"));
    assert_eq!(event.cwd.as_deref(), Some(Path::new("/work")));
    // Only jcode is read from there, and stdin still wins when it has
    // something to say.
    assert_eq!(hook_payload("claude", String::new(), env), "");
    assert_eq!(hook_payload("jcode", "{}".into(), env), "{}");
}

#[test]
fn jcode_hooks_use_native_config_lifecycle_keys() {
    let dir = std::env::temp_dir().join(format!("anterm-jcode-hooks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let host = FakeRemote::shared();
    let target = HookTarget::remote(&*host, dir.clone());
    let path = HookAgent::Jcode.target_path(&target);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[hooks]\nturn_start = \"user-hook\"\n").unwrap();
    install_hooks(&target, HookAgent::Jcode).expect("install succeeds");
    assert_eq!(
        hooks_state(&target, HookAgent::Jcode),
        HooksState::Installed
    );
    let text = std::fs::read_to_string(&path).unwrap();
    let doc: toml_edit::DocumentMut = text.parse().unwrap();
    let commands = jcode_commands(&doc["hooks"]["turn_start"]).unwrap();
    assert_eq!(
        commands,
        vec![
            "user-hook".to_owned(),
            target.hook_command(HookAgent::Jcode, "prompt-submit")
        ]
    );
    install_hooks(&target, HookAgent::Jcode).expect("reinstall succeeds");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        text,
        "reinstall adds no duplicates"
    );
    assert!(text.contains("agent-hook jcode prompt-submit"));
    assert_eq!(
        uninstall_hooks(&target, HookAgent::Jcode).unwrap(),
        HookOutcome::Removed
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("user-hook")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn antigravity_owns_a_named_hook_set_beside_the_users() {
    let dir = std::env::temp_dir().join(format!("anterm-agy-hooks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let host = FakeRemote::shared();
    let target = HookTarget::remote(&*host, dir.clone());
    let path = HookAgent::Antigravity.target_path(&target);
    assert_eq!(path, dir.join(".gemini/config/hooks.json"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let theirs = serde_json::json!({
        "my-linter-hook": {
            "PostToolUse": [{ "matcher": "run_command", "hooks": [{ "command": "./lint.sh" }] }]
        }
    });
    std::fs::write(&path, theirs.to_string()).unwrap();

    assert_eq!(
        hooks_state(&target, HookAgent::Antigravity),
        HooksState::NotInstalled
    );
    install_hooks(&target, HookAgent::Antigravity).expect("install succeeds");
    assert_eq!(
        hooks_state(&target, HookAgent::Antigravity),
        HooksState::Installed
    );
    let root: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        root["my-linter-hook"], theirs["my-linter-hook"],
        "the user's set survives"
    );
    let set = &root[ANTIGRAVITY_HOOK_SET];
    assert!(
        set["PreInvocation"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("agent-hook antigravity prompt-submit")
    );
    assert_eq!(set["PostToolUse"][0]["matcher"], "*");
    assert!(
        set["Stop"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("agent-hook antigravity stop")
    );
    assert!(
        set.get("PreToolUse").is_none(),
        "a status hook must not gate tools"
    );

    root_set_to(&path, serde_json::json!({ "Stop": [] }), &root);
    assert_eq!(
        hooks_state(&target, HookAgent::Antigravity),
        HooksState::Outdated
    );
    install_hooks(&target, HookAgent::Antigravity).expect("reinstall succeeds");
    assert_eq!(
        hooks_state(&target, HookAgent::Antigravity),
        HooksState::Installed
    );

    assert_eq!(
        uninstall_hooks(&target, HookAgent::Antigravity).expect("uninstall succeeds"),
        HookOutcome::Removed
    );
    let root: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        root, theirs,
        "uninstall leaves exactly what was there before"
    );
    assert_eq!(
        uninstall_hooks(&target, HookAgent::Antigravity).expect("second uninstall"),
        HookOutcome::NoAntermHooks
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn root_set_to(path: &Path, set: serde_json::Value, root: &serde_json::Value) {
    let mut root = root.clone();
    root[ANTIGRAVITY_HOOK_SET] = set;
    std::fs::write(path, root.to_string()).unwrap();
}

#[test]
fn antigravity_payloads_map_onto_turns() {
    let first = r#"{"conversationId":"c-1","invocationNum":0,"workspacePaths":["/repo"]}"#;
    let later = r#"{"conversationId":"c-1","invocationNum":2,"workspacePaths":["/repo"]}"#;
    assert_eq!(
        effective_event("antigravity", "prompt-submit", first),
        Some("prompt-submit")
    );
    assert_eq!(
        effective_event("antigravity", "prompt-submit", later),
        Some("tool-complete")
    );
    assert_eq!(
        effective_event(
            "antigravity",
            "stop",
            r#"{"conversationId":"c-1","fullyIdle":true}"#
        ),
        Some("stop")
    );
    assert_eq!(
        effective_event(
            "antigravity",
            "stop",
            r#"{"conversationId":"c-1","fullyIdle":false}"#
        ),
        None
    );
    let ev = round_trip("antigravity", "prompt-submit", first);
    assert_eq!(ev.session_id.as_deref(), Some("c-1"));
    assert_eq!(ev.cwd.as_deref(), Some(Path::new("/repo")));
    assert_eq!(
        ev.agent,
        Some(crate::cli_agent::CLIAgent::Antigravity)
    );
}

#[test]
fn a_remote_install_round_trips_through_the_host() {
    let dir = std::env::temp_dir().join(format!("anterm-remote-hooks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = FakeRemote::shared();
    let target = HookTarget::remote(&*host, dir.clone());

    for agent in [HookAgent::Claude, HookAgent::Grok, HookAgent::OhMyPi] {
        assert_eq!(hooks_state(&target, agent), HooksState::NotInstalled);
        install_hooks(&target, agent).expect("install succeeds");
        assert_eq!(hooks_state(&target, agent), HooksState::Installed);
        let path = agent.target_path(&target);
        let dialect = crate::daemon::install::RemoteProtocol::of_this_build();
        assert!(std::fs::read_to_string(&path).unwrap().contains(&format!(
            "anterm-server-c{}p{}",
            dialect.control, dialect.protocol
        )));
        uninstall_hooks(&target, agent).expect("uninstall succeeds");
        assert_eq!(hooks_state(&target, agent), HooksState::NotInstalled);
    }

    let summary = install_hooks(&target, HookAgent::Codex).expect("codex install succeeds");
    assert_eq!(
        summary,
        HookOutcome::InstalledEnableCodexThere,
        "a remote codex install has to hand the leftover step back to the caller"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn owned_file_contents_carry_marker_and_exe() {
    let exe_raw = std::env::current_exe().unwrap().display().to_string();
    let exe_json = serde_json::to_string(&exe_raw).unwrap();
    let exe = exe_json.trim_matches('"').to_string();
    let host = local_host();
    let target = HookTarget::local(&*host).expect("home resolves in tests");
    let hook_exe_raw = target.hook_command_exe().unwrap_or_else(|| exe_raw.clone());
    let hook_exe_json = serde_json::to_string(&hook_exe_raw).unwrap();
    let hook_exe = hook_exe_json.trim_matches('"');

    let copilot = copilot_hooks_json(&target).expect("copilot content builds");
    let parsed: serde_json::Value = serde_json::from_str(&copilot).expect("valid JSON");
    for event in [
        "sessionStart",
        "userPromptSubmitted",
        "agentStop",
        "sessionEnd",
        "notification",
    ] {
        assert!(
            parsed["hooks"][event][0]["bash"]
                .as_str()
                .is_some_and(|c| c.contains("agent-hook copilot")),
            "copilot {event} carries the emitter"
        );
    }
    assert!(copilot.contains(hook_exe));

    let opencode = opencode_plugin_js(&target).expect("opencode content builds");
    assert!(opencode.contains("agent-hook opencode"));
    assert!(opencode.contains(&format!("const EXE = {exe_json}")));
    assert!(opencode.contains(r#"process.env["ANTERM"]"#));
    for (needle, message) in [
        (
            r#"export default { id: "anterm", server, setup }"#,
            "opencode default-exports one definition both plugin loaders accept (#999)",
        ),
        (
            "export const AntermPresence = server",
            "opencode keeps the named factory for 1.x loaders that call every export",
        ),
        (
            r#"spawn(EXE, ["agent-hook", "opencode", event]"#,
            "opencode spawns the emitter without a shell, which Windows lacks",
        ),
        (
            "event.properties ?? {}",
            "opencode 1.x reads event fields from properties",
        ),
        (
            "ctx.event.subscribe(",
            "opencode 2.x reads events from the subscribe stream",
        ),
        (
            "event.data ?? {}",
            "opencode 2.x reads event fields from data",
        ),
        (
            r#"process.argv.includes("--service")"#,
            "opencode 2.x stays inert in the machine-wide background service",
        ),
        (
            r#"session_id: sessionId"#,
            "opencode forwards the session id to the emitter",
        ),
        (
            r#""session.status.busy": "prompt-submit""#,
            "opencode maps session.status busy to prompt-submit",
        ),
        (
            r#""session.status.idle": "stop""#,
            "opencode maps session.status idle to stop",
        ),
        (
            r#""session.idle": "stop""#,
            "opencode still maps the session.idle event to stop",
        ),
        (
            r#""permission.asked": "permission-request""#,
            "opencode 2.x reports its permission.asked event as a permission request",
        ),
        (
            r#""permission.ask": async"#,
            "opencode 1.x keeps the permission.ask hook",
        ),
        (
            "info.parentID",
            "opencode 1.x tells a subagent's child session apart from the pane's own",
        ),
        (
            "fields.parentID",
            "opencode 2.x tells a subagent's child session apart from the pane's own",
        ),
        (
            "children.has(fields.sessionID)",
            "opencode lets a child session's events pass without touching the pane",
        ),
    ] {
        assert!(opencode.contains(needle), "{message}");
    }
    assert!(
        !opencode.contains("sh -c"),
        "opencode no longer routes the emitter through sh"
    );

    for (agent, slug, package) in [
        (HookAgent::Pi, "pi", "@earendil-works/pi-coding-agent"),
        (HookAgent::OhMyPi, "omp", "@oh-my-pi/pi-coding-agent"),
        (
            HookAgent::PrimeAgent,
            "prime-agent",
            "@earendil-works/pi-coding-agent",
        ),
    ] {
        let bridge =
            pi_extension_ts(&target, agent).unwrap_or_else(|| panic!("{slug} content builds"));
        assert!(bridge.contains(&format!("agent-hook {slug}")));
        assert!(bridge.contains(&format!(r#"["agent-hook", "{slug}", event]"#)));
        assert!(bridge.contains(&format!(r#"from "{package}""#)));
        assert!(bridge.contains(&exe));
        assert!(bridge.contains(r#"process.env["ANTERM"]"#));
        assert!(bridge.contains("getSessionId"));
        assert!(bridge.contains("session_id"));
        assert!(bridge.contains(r#"stdio: ["pipe", "ignore", "ignore"]"#));
        for event in [
            "session_start",
            "agent_start",
            "agent_end",
            "session_shutdown",
            "ui_prompt_start",
            "ui_prompt_end",
            "agent_settled",
            "tool_approval_requested",
            "tool_approval_resolved",
            "auto_retry_start",
        ] {
            assert!(
                bridge.contains(&format!(r#"pi.on("{event}""#)),
                "{slug} bridge subscribes to {event}"
            );
        }
        assert!(
            bridge.contains(r#"pi.on("ui_prompt_end", (_event, ctx) => emit(turn, ctx))"#),
            "{slug} restores the pre-prompt status rather than forcing working"
        );
    }
    assert!(
        pi_extension_ts(&target, HookAgent::Claude).is_none(),
        "only the two Pi-shaped agents get this bridge"
    );

    let grok = grok_hooks_json(&target).expect("grok content builds");
    let parsed: serde_json::Value = serde_json::from_str(&grok).expect("valid JSON");
    for (event, sentinel, matcher) in GROK_HOOK_EVENTS {
        let group = &parsed["hooks"][*event][0];
        let cmd = group["hooks"][0]["command"]
            .as_str()
            .unwrap_or_else(|| panic!("grok {event} carries a command"));
        assert!(
            cmd.ends_with(&format!("agent-hook grok {sentinel}")),
            "grok {event} runs the emitter with {sentinel}, got {cmd}"
        );
        assert_eq!(
            group.get("matcher").and_then(|m| m.as_str()),
            *matcher,
            "grok {event} matcher"
        );
    }
    assert!(grok.contains(hook_exe));
}

#[test]
fn owned_file_round_trip_and_ownership_guard() {
    let dir = std::env::temp_dir().join(format!("anterm-owned-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("anterm.json");
    let marker = "agent-hook copilot";
    let host = local_host();
    let t = HookTarget::local(&*host).expect("home resolves in tests");
    let content = copilot_hooks_json(&t).unwrap();

    assert_eq!(
        owned_file_state(&t, &path, &content, marker),
        HooksState::NotInstalled
    );
    owned_file_install(&t, &path, &content, marker).expect("fresh install succeeds");
    assert_eq!(
        owned_file_state(&t, &path, &content, marker),
        HooksState::Installed
    );

    std::fs::write(&path, content.replace(marker, "agent-hook copilot --old")).unwrap();
    assert_eq!(
        owned_file_state(&t, &path, &content, marker),
        HooksState::Outdated
    );
    owned_file_install(&t, &path, &content, marker).expect("reinstall over our own file");
    assert_eq!(
        owned_file_state(&t, &path, &content, marker),
        HooksState::Installed
    );

    std::fs::write(&path, "// my own hooks, hands off").unwrap();
    assert!(owned_file_install(&t, &path, &content, marker).is_err());
    assert!(owned_file_uninstall(&t, &path, marker).is_err());

    std::fs::write(&path, &content).unwrap();
    owned_file_uninstall(&t, &path, marker).expect("uninstall succeeds");
    assert!(!path.exists());
    owned_file_uninstall(&t, &path, marker).expect("uninstall is idempotent");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn qoder_config_dir_controls_local_hook_lifecycle() {
    const CASE_ENV: &str = "ANTERM_TEST_QODER_CONFIG_CASE";
    const ROOT_ENV: &str = "ANTERM_TEST_QODER_CONFIG_ROOT";
    let Ok(case) = std::env::var(CASE_ENV) else {
        // Each case gets its own environment, without changing the one
        // shared by the other tests or touching the user's settings.
        for case in ["override", "empty", "unset"] {
            let sandbox = tempfile::tempdir().unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "agent_hooks::tests::qoder_config_dir_controls_local_hook_lifecycle",
                    "--nocapture",
                ])
                .env(CASE_ENV, case)
                .env(ROOT_ENV, sandbox.path());
            match case {
                "override" => {
                    child.env("QODER_CONFIG_DIR", sandbox.path().join("custom config"))
                }
                "empty" => child.env("QODER_CONFIG_DIR", ""),
                _ => child.env_remove("QODER_CONFIG_DIR"),
            };
            let output = crate::proc::output_within(
                crate::proc::hide_console(&mut child),
                std::time::Duration::from_secs(30),
            )
            .expect("run the isolated Qoder hook test");
            assert!(
                output.status.success(),
                "{case}:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        return;
    };

    let root = PathBuf::from(std::env::var_os(ROOT_ENV).unwrap());
    let host = local_host();
    let target = HookTarget {
        host: &*host,
        home: root.join("home"),
        exe: std::env::current_exe().unwrap(),
    };
    let default_settings = target.home.join(".qoder").join("settings.json");
    let custom_settings = root.join("custom config").join("settings.json");
    let (settings, untouched) = if case == "override" {
        (&custom_settings, &default_settings)
    } else {
        (&default_settings, &custom_settings)
    };
    let user_config = serde_json::json!({
        "model": "qoder-test",
        "hooks": {
            "Stop": [{ "hooks": [{ "type": "command", "command": "echo user-hook" }] }]
        }
    });
    for path in [settings, untouched] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, user_config.to_string()).unwrap();
    }

    let agent = HookAgent::QoderCLI;
    assert_eq!(agent.target_path(&target), *settings);
    let remote_host = FakeRemote::shared();
    let remote = HookTarget::remote(&*remote_host, PathBuf::from("/home/me"));
    assert_eq!(
        agent.target_path(&remote),
        PathBuf::from("/home/me/.qoder/settings.json"),
        "a local override must not redirect remote hooks"
    );

    assert_eq!(hooks_state(&target, agent), HooksState::NotInstalled);
    assert_eq!(
        install_hooks(&target, agent).unwrap(),
        HookOutcome::Installed
    );
    assert_eq!(hooks_state(&target, agent), HooksState::Installed);
    assert!(
        std::fs::read_to_string(settings)
            .unwrap()
            .contains("agent-hook qodercli")
    );
    assert_eq!(
        uninstall_hooks(&target, agent).unwrap(),
        HookOutcome::Removed
    );
    assert_eq!(hooks_state(&target, agent), HooksState::NotInstalled);
    for path in [settings, untouched] {
        let actual: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(actual, user_config, "{}", path.display());
    }
}

#[test]
fn qoder_cn_config_dir_controls_local_hook_lifecycle() {
    const CASE_ENV: &str = "ANTERM_TEST_QODER_CN_CONFIG_CASE";
    const ROOT_ENV: &str = "ANTERM_TEST_QODER_CN_CONFIG_ROOT";
    let Ok(case) = std::env::var(CASE_ENV) else {
        for case in ["override", "empty", "unset"] {
            let sandbox = tempfile::tempdir().unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "agent_hooks::tests::qoder_cn_config_dir_controls_local_hook_lifecycle",
                    "--nocapture",
                ])
                .env(CASE_ENV, case)
                .env(ROOT_ENV, sandbox.path());
            match case {
                "override" => {
                    child.env("QODERCN_CONFIG_DIR", sandbox.path().join("custom config"))
                }
                "empty" => child.env("QODERCN_CONFIG_DIR", ""),
                _ => child.env_remove("QODERCN_CONFIG_DIR"),
            };
            let output = crate::proc::output_within(
                crate::proc::hide_console(&mut child),
                std::time::Duration::from_secs(30),
            )
            .expect("run the isolated Qoder CN hook test");
            assert!(
                output.status.success(),
                "{case}:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        return;
    };

    let root = PathBuf::from(std::env::var_os(ROOT_ENV).unwrap());
    let host = local_host();
    let target = HookTarget {
        host: &*host,
        home: root.join("home"),
        exe: std::env::current_exe().unwrap(),
    };
    let default_settings = target.home.join(".qoder-cn").join("settings.json");
    let custom_settings = root.join("custom config").join("settings.json");
    let (settings, untouched) = if case == "override" {
        (&custom_settings, &default_settings)
    } else {
        (&default_settings, &custom_settings)
    };
    let user_config = serde_json::json!({
        "model": "qoder-cn-test",
        "hooks": {
            "Stop": [{ "hooks": [{ "type": "command", "command": "echo user-hook" }] }]
        }
    });
    for path in [settings, untouched] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, user_config.to_string()).unwrap();
    }

    let agent = HookAgent::QoderCLICn;
    assert_eq!(agent.target_path(&target), *settings);
    let remote_host = FakeRemote::shared();
    let remote = HookTarget::remote(&*remote_host, PathBuf::from("/home/me"));
    assert_eq!(
        agent.target_path(&remote),
        PathBuf::from("/home/me/.qoder-cn/settings.json"),
        "a local override must not redirect remote hooks"
    );

    assert_eq!(hooks_state(&target, agent), HooksState::NotInstalled);
    assert_eq!(
        install_hooks(&target, agent).unwrap(),
        HookOutcome::Installed
    );
    assert_eq!(hooks_state(&target, agent), HooksState::Installed);
    assert!(
        std::fs::read_to_string(settings)
            .unwrap()
            .contains("agent-hook qoderclicn")
    );
    assert_eq!(
        uninstall_hooks(&target, agent).unwrap(),
        HookOutcome::Removed
    );
    assert_eq!(hooks_state(&target, agent), HooksState::NotInstalled);
    for path in [settings, untouched] {
        let actual: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(actual, user_config, "{}", path.display());
    }
}

#[test]
fn codebuddy_config_dir_controls_local_hook_lifecycle() {
    const CASE_ENV: &str = "ANTERM_TEST_CODEBUDDY_CONFIG_CASE";
    const ROOT_ENV: &str = "ANTERM_TEST_CODEBUDDY_CONFIG_ROOT";
    let Ok(case) = std::env::var(CASE_ENV) else {
        // Each case gets its own environment, without changing the one
        // shared by the other tests or touching the user's settings.
        for case in ["override", "blank", "unset"] {
            let sandbox = tempfile::tempdir().unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "agent_hooks::tests::codebuddy_config_dir_controls_local_hook_lifecycle",
                    "--nocapture",
                ])
                .env(CASE_ENV, case)
                .env(ROOT_ENV, sandbox.path());
            match case {
                "override" => {
                    child.env("CODEBUDDY_CONFIG_DIR", sandbox.path().join("custom config"))
                }
                // CodeBuddy trims the value, so whitespace is as unset as empty.
                "blank" => child.env("CODEBUDDY_CONFIG_DIR", "  "),
                _ => child.env_remove("CODEBUDDY_CONFIG_DIR"),
            };
            let output = crate::proc::output_within(
                crate::proc::hide_console(&mut child),
                std::time::Duration::from_secs(30),
            )
            .expect("run the isolated CodeBuddy hook test");
            assert!(
                output.status.success(),
                "{case}:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        return;
    };

    let root = PathBuf::from(std::env::var_os(ROOT_ENV).unwrap());
    let host = local_host();
    let target = HookTarget {
        host: &*host,
        home: root.join("home"),
        exe: std::env::current_exe().unwrap(),
    };
    let default_settings = target.home.join(".codebuddy").join("settings.json");
    let custom_settings = root.join("custom config").join("settings.json");
    let (settings, untouched) = if case == "override" {
        (&custom_settings, &default_settings)
    } else {
        (&default_settings, &custom_settings)
    };
    let user_config = serde_json::json!({
        "model": "codebuddy-test",
        "hooks": {
            "Stop": [{ "hooks": [{ "type": "command", "command": "echo user-hook" }] }]
        }
    });
    for path in [settings, untouched] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, user_config.to_string()).unwrap();
    }

    let agent = HookAgent::CodeBuddy;
    assert_eq!(agent.target_path(&target), *settings);
    let remote_host = FakeRemote::shared();
    let remote = HookTarget::remote(&*remote_host, PathBuf::from("/home/me"));
    assert_eq!(
        agent.target_path(&remote),
        PathBuf::from("/home/me/.codebuddy/settings.json"),
        "a local override must not redirect remote hooks"
    );

    assert_eq!(hooks_state(&target, agent), HooksState::NotInstalled);
    assert_eq!(
        install_hooks(&target, agent).unwrap(),
        HookOutcome::Installed
    );
    assert_eq!(hooks_state(&target, agent), HooksState::Installed);
    assert!(
        std::fs::read_to_string(settings)
            .unwrap()
            .contains("agent-hook codebuddy")
    );
    assert_eq!(
        uninstall_hooks(&target, agent).unwrap(),
        HookOutcome::Removed
    );
    assert_eq!(hooks_state(&target, agent), HooksState::NotInstalled);
    for path in [settings, untouched] {
        let actual: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(actual, user_config, "{}", path.display());
    }
}

/// Crush's hook map is one list under `hooks.PreToolUse` like Claude's, but
/// each entry carries `command` and `matcher` directly instead of wrapping
/// them in a nested `hooks` array. The merge has to write the flat shape and
/// still leave a user's own hooks and the rest of `crush.json` alone.
#[test]
fn crush_installs_a_flat_hook_and_preserves_user_entries() {
    let host = FakeRemote::shared();
    let base = std::env::temp_dir().join(format!("anterm-crush-hooks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let target = HookTarget::remote(&*host, base.clone());
    let config = HookAgent::Crush.target_path(&target);
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    let user_config = serde_json::json!({
        "model": "crush-test",
        "hooks": {
            "PreToolUse": [
                { "matcher": "^bash$", "command": "echo user-hook", "timeout": 5 }
            ]
        }
    });
    std::fs::write(&config, serde_json::to_string_pretty(&user_config).unwrap()).unwrap();

    assert_eq!(
        hooks_state(&target, HookAgent::Crush),
        HooksState::NotInstalled
    );
    install_hooks(&target, HookAgent::Crush).expect("install succeeds");
    assert_eq!(
        hooks_state(&target, HookAgent::Crush),
        HooksState::Installed
    );
    install_hooks(&target, HookAgent::Crush).expect("re-install succeeds");

    let merged: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(merged["model"], "crush-test");
    let entries = merged["hooks"]["PreToolUse"].as_array().unwrap();
    let ours: Vec<&serde_json::Value> = entries
        .iter()
        .filter(|e| {
            e.get("command")
                .and_then(|c| c.as_str())
                .is_some_and(|c| c.contains("agent-hook crush"))
        })
        .collect();
    assert_eq!(ours.len(), 1, "exactly one anterm entry after two installs");
    assert_eq!(
        ours[0]["command"].as_str(),
        Some(
            target
                .hook_command(HookAgent::Crush, "tool-complete")
                .as_str()
        ),
        "the entry is flat and names the tool-complete emitter"
    );
    assert!(
        ours[0].get("hooks").is_none(),
        "Crush does not nest a hooks array inside the entry"
    );
    assert!(
        entries.iter().any(|e| e
            .get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|c| c.contains("user-hook"))),
        "the user's own PreToolUse hook survives"
    );

    assert_eq!(
        uninstall_hooks(&target, HookAgent::Crush).unwrap(),
        HookOutcome::Removed
    );
    assert_eq!(
        hooks_state(&target, HookAgent::Crush),
        HooksState::NotInstalled
    );
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(
        after, user_config,
        "uninstall restores the user's file exactly"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn crush_global_config_controls_local_hook_lifecycle() {
    const CASE_ENV: &str = "ANTERM_TEST_CRUSH_CONFIG_CASE";
    const ROOT_ENV: &str = "ANTERM_TEST_CRUSH_CONFIG_ROOT";
    let Ok(case) = std::env::var(CASE_ENV) else {
        // Each case gets its own environment, without changing the one
        // shared by the other tests or touching the user's settings.
        for case in ["override", "empty", "unset"] {
            let sandbox = tempfile::tempdir().unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "agent_hooks::tests::crush_global_config_controls_local_hook_lifecycle",
                    "--nocapture",
                ])
                .env(CASE_ENV, case)
                .env(ROOT_ENV, sandbox.path())
                // `crush_settings_path` falls back to `$XDG_CONFIG_HOME`
                // before the sandbox home, and CI exports it. Neutralise it
                // so the default lands under the home this test owns.
                .env_remove("XDG_CONFIG_HOME");
            match case {
                "override" => {
                    child.env("CRUSH_GLOBAL_CONFIG", sandbox.path().join("custom config"))
                }
                "empty" => child.env("CRUSH_GLOBAL_CONFIG", ""),
                _ => child.env_remove("CRUSH_GLOBAL_CONFIG"),
            };
            let output = crate::proc::output_within(
                crate::proc::hide_console(&mut child),
                std::time::Duration::from_secs(30),
            )
            .expect("run the isolated Crush hook test");
            assert!(
                output.status.success(),
                "{case}:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        return;
    };

    let root = PathBuf::from(std::env::var_os(ROOT_ENV).unwrap());
    let host = local_host();
    let target = HookTarget {
        host: &*host,
        home: root.join("home"),
        exe: std::env::current_exe().unwrap(),
    };
    let default_config = target.home.join(".config").join("crush").join("crush.json");
    let custom_config = root.join("custom config").join("crush.json");
    let (config, untouched) = if case == "override" {
        (&custom_config, &default_config)
    } else {
        (&default_config, &custom_config)
    };
    let user_config = serde_json::json!({
        "model": "crush-test",
        "hooks": {
            "PreToolUse": [
                { "command": "echo user-hook" }
            ]
        }
    });
    for path in [config, untouched] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_string(&user_config).unwrap()).unwrap();
    }

    let agent = HookAgent::Crush;
    assert_eq!(agent.target_path(&target), *config);
    let remote_host = FakeRemote::shared();
    let remote = HookTarget::remote(&*remote_host, PathBuf::from("/home/me"));
    assert_eq!(
        agent.target_path(&remote),
        PathBuf::from("/home/me/.config/crush/crush.json"),
        "a local override must not redirect remote hooks"
    );

    assert_eq!(hooks_state(&target, agent), HooksState::NotInstalled);
    assert_eq!(
        install_hooks(&target, agent).unwrap(),
        HookOutcome::Installed
    );
    assert_eq!(hooks_state(&target, agent), HooksState::Installed);
    assert!(
        std::fs::read_to_string(config)
            .unwrap()
            .contains("agent-hook crush")
    );
    assert_eq!(
        uninstall_hooks(&target, agent).unwrap(),
        HookOutcome::Removed
    );
    assert_eq!(hooks_state(&target, agent), HooksState::NotInstalled);
    for path in [config, untouched] {
        let actual: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(actual, user_config, "{}", path.display());
    }
}

#[test]
fn install_is_idempotent_and_preserves_user_hooks() {
    let dir = std::env::temp_dir().join(format!("anterm-hooks-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let settings = dir.join("settings.json");
    std::fs::write(
        &settings,
        serde_json::json!({
            "model": "opus",
            "hooks": {
                "Stop": [{ "hooks": [{ "type": "command", "command": "afplay ding.aiff" }] }]
            }
        })
        .to_string(),
    )
    .unwrap();
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &dir) };

    let host = local_host();
    let t = HookTarget::local(&*host).expect("home resolves in tests");
    let remote_host = FakeRemote::shared();
    let remote = HookTarget::remote(&*remote_host, PathBuf::from("/home/me"));
    assert_eq!(
        HookAgent::Claude.target_path(&remote),
        PathBuf::from("/home/me/.claude/settings.json")
    );

    assert_eq!(hooks_state(&t, HookAgent::Claude), HooksState::NotInstalled);
    install_hooks(&t, HookAgent::Claude).expect("install succeeds");
    assert_eq!(hooks_state(&t, HookAgent::Claude), HooksState::Installed);

    install_hooks(&t, HookAgent::Claude).expect("re-install succeeds");
    let root: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(root["model"], "opus");
    let stop = root["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(
        stop.iter()
            .filter(|m| marker_command(m, "agent-hook claude").is_some())
            .count(),
        1,
        "exactly one anterm entry after two installs"
    );
    assert!(
        stop.iter()
            .any(|m| m.to_string().contains("afplay ding.aiff")),
        "the user's own Stop hook survives"
    );
    for (event, _) in CLAUDE_HOOK_EVENTS {
        assert!(
            root["hooks"][*event]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| marker_command(m, "agent-hook claude").is_some()),
            "{event} carries the anterm hook"
        );
    }

    let healthy = std::fs::read_to_string(&settings).unwrap();
    std::fs::write(
        &settings,
        healthy.replace("agent-hook claude stop", "agent-hook claude stop --stale"),
    )
    .unwrap();
    assert_eq!(hooks_state(&t, HookAgent::Claude), HooksState::Outdated);
    install_hooks(&t, HookAgent::Claude).expect("reinstall over an outdated entry succeeds");
    assert_eq!(hooks_state(&t, HookAgent::Claude), HooksState::Installed);

    uninstall_hooks(&t, HookAgent::Claude).expect("uninstall succeeds");
    assert_eq!(hooks_state(&t, HookAgent::Claude), HooksState::NotInstalled);
    let root: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(root["model"], "opus");
    let stop = root["hooks"]["Stop"].as_array().unwrap();
    assert!(
        stop.iter()
            .any(|m| m.to_string().contains("afplay ding.aiff")),
        "the user's own Stop hook survives uninstall"
    );
    assert!(
        root["hooks"].get("SessionStart").is_none(),
        "an event list that held only the anterm hook is dropped"
    );
    uninstall_hooks(&t, HookAgent::Claude).expect("uninstall is idempotent");

    unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };
    let _ = std::fs::remove_dir_all(&dir);
}

/// A Codex moved to `CODEX_HOME` reads its hooks from there, and a
/// `hooks.json` under `~/.codex` is one it never looks at.
#[test]
fn codex_hooks_live_in_codex_home_when_it_is_set() {
    let dir = std::env::temp_dir().join(format!("anterm-codex-home-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    unsafe { std::env::set_var("CODEX_HOME", &dir) };

    let host = local_host();
    let t = HookTarget::local(&*host).expect("home resolves in tests");
    assert_eq!(HookAgent::Codex.target_path(&t), dir.join("hooks.json"));
    assert_eq!(hooks_state(&t, HookAgent::Codex), HooksState::NotInstalled);
    let remote_host = FakeRemote::shared();
    let remote = HookTarget::remote(&*remote_host, PathBuf::from("/home/me"));
    assert_eq!(
        HookAgent::Codex.target_path(&remote),
        PathBuf::from("/home/me/.codex/hooks.json"),
        "a local CODEX_HOME says nothing about a remote machine"
    );

    unsafe { std::env::remove_var("CODEX_HOME") };
    let _ = std::fs::remove_dir_all(&dir);
}

/// A Gemini CLI moved to `GEMINI_CLI_HOME` keeps its `.gemini` directory
/// there, and `~/.gemini/settings.json` is a file it never reads.
#[test]
fn gemini_hooks_live_under_gemini_cli_home_when_it_is_set() {
    let dir =
        std::env::temp_dir().join(format!("anterm-gemini-home-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    unsafe { std::env::set_var("GEMINI_CLI_HOME", &dir) };

    let host = local_host();
    let t = HookTarget::local(&*host).expect("home resolves in tests");
    assert_eq!(
        HookAgent::Gemini.target_path(&t),
        dir.join(".gemini").join("settings.json")
    );
    assert_eq!(hooks_state(&t, HookAgent::Gemini), HooksState::NotInstalled);
    let remote_host = FakeRemote::shared();
    let remote = HookTarget::remote(&*remote_host, PathBuf::from("/home/me"));
    assert_eq!(
        HookAgent::Gemini.target_path(&remote),
        PathBuf::from("/home/me/.gemini/settings.json"),
        "a local GEMINI_CLI_HOME says nothing about a remote machine"
    );

    unsafe { std::env::remove_var("GEMINI_CLI_HOME") };
    let _ = std::fs::remove_dir_all(&dir);
}

/// Kimi's hooks share `config.toml` with the user's providers and models,
/// so the merge must leave everything that is not ours — including
/// comments and formatting — byte-for-byte alone.
#[test]
fn kimi_install_preserves_the_user_s_config_toml() {
    let dir = std::env::temp_dir().join(format!("anterm-kimi-hooks-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.toml");
    let user_half = concat!(
        "# my providers\n",
        "default_model = \"kimi-k2\"\n",
        "\n",
        "[[hooks]]\n",
        "event = \"Stop\"\n",
        "command = \"afplay ding.aiff\"\n",
    );
    std::fs::write(&config, user_half).unwrap();
    unsafe { std::env::set_var("KIMI_CODE_HOME", &dir) };

    let host = local_host();
    let t = HookTarget::local(&*host).expect("home resolves in tests");
    assert_eq!(HookAgent::Kimi.target_path(&t), config);

    assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::NotInstalled);
    install_hooks(&t, HookAgent::Kimi).expect("install succeeds");
    assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Installed);
    install_hooks(&t, HookAgent::Kimi).expect("re-install succeeds");

    let written = std::fs::read_to_string(&config).unwrap();
    assert!(
        written.starts_with(user_half),
        "the user's half of config.toml — comment included — survives untouched"
    );
    let doc: toml_edit::DocumentMut = written.parse().expect("still valid TOML");
    let hooks = doc["hooks"].as_array_of_tables().unwrap();
    assert_eq!(
        hooks
            .iter()
            .filter(|e| toml_command_is_marked(e, "agent-hook kimi"))
            .count(),
        KIMI_HOOK_EVENTS.len(),
        "exactly one anterm entry per event after two installs"
    );
    for (event, _) in KIMI_HOOK_EVENTS {
        assert!(
            hooks.iter().any(|e| {
                toml_command_is_marked(e, "agent-hook kimi")
                    && e.get("event").and_then(|v| v.as_str()) == Some(*event)
            }),
            "{event} carries the anterm hook"
        );
    }

    let healthy = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        healthy.replace("agent-hook kimi stop", "agent-hook kimi stop --stale"),
    )
    .unwrap();
    assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Outdated);
    install_hooks(&t, HookAgent::Kimi).expect("reinstall over an outdated entry succeeds");
    assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Installed);

    uninstall_hooks(&t, HookAgent::Kimi).expect("uninstall succeeds");
    assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::NotInstalled);
    let after = std::fs::read_to_string(&config).unwrap();
    assert!(
        after.contains("afplay ding.aiff"),
        "the user's own Stop hook survives uninstall"
    );
    assert!(!after.contains("agent-hook kimi"));
    uninstall_hooks(&t, HookAgent::Kimi).expect("uninstall is idempotent");

    std::fs::write(&config, "not = valid = toml").unwrap();
    assert!(
        install_hooks(&t, HookAgent::Kimi).is_err(),
        "a config.toml that does not parse is left alone"
    );
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        "not = valid = toml",
        "and is not rewritten on the way out"
    );
    assert!(
        uninstall_hooks(&t, HookAgent::Kimi).is_err(),
        "uninstall refuses the same file"
    );
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        "not = valid = toml"
    );

    unsafe { std::env::remove_var("KIMI_CODE_HOME") };
    let _ = std::fs::remove_dir_all(&dir);
}

/// A `config.toml` that parses but spells `hooks` as something other than
/// an array of tables is a file we do not understand. Every one of these
/// must come back as a refusal with the file untouched — the one thing
/// that must never happen to the file holding the user's API keys is a
/// silent rewrite.
#[test]
fn kimi_refuses_a_hooks_key_of_the_wrong_toml_type() {
    let host = FakeRemote::shared();
    let base = std::env::temp_dir().join(format!("anterm-kimi-shapes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);

    for (name, text) in [
        ("a_string", "hooks = \"nope\"\n"),
        ("a_table", "[hooks]\nfoo = 1\n"),
        (
            "an_inline_array",
            "hooks = [{ event = \"Stop\", command = \"afplay a.aiff\" }]\n",
        ),
    ] {
        let home = base.join(name);
        let t = HookTarget::remote(&*host, home.clone());
        let config = HookAgent::Kimi.target_path(&t);
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, text).unwrap();

        assert_eq!(
            hooks_state(&t, HookAgent::Kimi),
            HooksState::NotInstalled,
            "{name}: nothing of ours is in there"
        );
        assert!(
            install_hooks(&t, HookAgent::Kimi).is_err(),
            "{name}: install refuses"
        );
        assert_eq!(
            uninstall_hooks(&t, HookAgent::Kimi).unwrap(),
            HookOutcome::NoAntermHooks,
            "{name}: uninstall finds nothing of ours"
        );
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            text,
            "{name}: the file is byte-for-byte what it was"
        );
    }

    // `hooks = []` is the one shape that carries no configuration at all,
    // so it is promoted instead of refused.
    let home = base.join("an_empty_array");
    let t = HookTarget::remote(&*host, home.clone());
    let config = HookAgent::Kimi.target_path(&t);
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "model = \"k2\"\nhooks = []\n").unwrap();
    install_hooks(&t, HookAgent::Kimi).expect("an empty inline array is promoted, not refused");
    assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Installed);
    let written = std::fs::read_to_string(&config).unwrap();
    assert!(written.starts_with("model = \"k2\"\n"), "{written}");
    assert!(!written.contains("hooks = []"), "{written}");
    assert_eq!(
        uninstall_hooks(&t, HookAgent::Kimi).unwrap(),
        HookOutcome::Removed
    );
    assert!(
        !std::fs::read_to_string(&config).unwrap().contains("hooks"),
        "the key goes when the last entry in it does"
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// The rest of the TOML merge contract: a file that does not exist yet, a
/// second install that changes nothing, entries a hand-edit has mangled,
/// and an uninstall that has to thread its removals between the user's own
/// entries and the tables that follow them.
#[test]
fn kimi_toml_merge_holds_up_across_the_awkward_shapes() {
    let host = FakeRemote::shared();
    let base = std::env::temp_dir().join(format!("anterm-kimi-merge-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let marker = "agent-hook kimi";

    // Nothing there at all: install creates the directory and the file.
    let t = HookTarget::remote(&*host, base.join("fresh"));
    let config = HookAgent::Kimi.target_path(&t);
    assert!(!config.exists());
    assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::NotInstalled);
    assert_eq!(
        uninstall_hooks(&t, HookAgent::Kimi).unwrap(),
        HookOutcome::NothingInstalled,
        "there is no file to take anything out of"
    );
    install_hooks(&t, HookAgent::Kimi).expect("install writes a fresh config.toml");
    assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Installed);
    let once = std::fs::read_to_string(&config).unwrap();
    install_hooks(&t, HookAgent::Kimi).expect("re-install succeeds");
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        once,
        "a second install is byte-for-byte the first — no churn, no growth"
    );

    // A hand-edit that drops `event` leaves an entry that is ours and is
    // broken. That is Outdated, not NotInstalled: `refresh_hooks` only
    // ever revisits Outdated, so anything else hides the damage.
    let mangled = once.replacen("event = \"SessionStart\"\n", "", 1);
    assert_ne!(mangled, once);
    std::fs::write(&config, &mangled).unwrap();
    assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Outdated);
    install_hooks(&t, HookAgent::Kimi).expect("install repairs it");
    assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Installed);
    assert_eq!(std::fs::read_to_string(&config).unwrap(), once);

    // One marked entry too many is Outdated too, and install prunes it.
    // This one has no `event` at all, so counting only the entries that
    // still name one would find the full roster and call it Installed
    // while a broken ninth entry sat there.
    let mut extra = std::fs::read_to_string(&config).unwrap();
    extra.push_str("\n[[hooks]]\ncommand = \"anterm agent-hook kimi stop\"\n");
    std::fs::write(&config, &extra).unwrap();
    assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Outdated);
    install_hooks(&t, HookAgent::Kimi).expect("install prunes the stray");
    assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Installed);
    let doc: toml_edit::DocumentMut =
        std::fs::read_to_string(&config).unwrap().parse().unwrap();
    assert_eq!(
        doc["hooks"]
            .as_array_of_tables()
            .unwrap()
            .iter()
            .filter(|e| toml_command_is_marked(e, marker))
            .count(),
        KIMI_HOOK_EVENTS.len()
    );

    // The user's own entry comes first and another table follows ours:
    // uninstall has to take out the middle and leave both ends alone.
    let t = HookTarget::remote(&*host, base.join("sandwich"));
    let config = HookAgent::Kimi.target_path(&t);
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    let user_half = concat!(
        "# mine\n",
        "[[hooks]]\n",
        "event = \"Stop\"\n",
        "command = \"afplay a.aiff\"  # ding\n",
        "\n",
        "[providers.moonshot]\n",
        "api_key = \"secret\"\n",
    );
    std::fs::write(&config, user_half).unwrap();
    install_hooks(&t, HookAgent::Kimi).expect("install");
    assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Installed);
    let merged = std::fs::read_to_string(&config).unwrap();
    assert!(
        merged.starts_with(
            "# mine\n[[hooks]]\nevent = \"Stop\"\ncommand = \"afplay a.aiff\"  # ding\n"
        ),
        "the user's entry and its trailing comment come through verbatim:\n{merged}"
    );
    assert!(merged.contains("[providers.moonshot]\napi_key = \"secret\"\n"));
    assert_eq!(
        uninstall_hooks(&t, HookAgent::Kimi).unwrap(),
        HookOutcome::Removed
    );
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        user_half,
        "uninstall puts the file back exactly as the user left it"
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// Kimi's `Stop` does not fire when the user interrupts a turn or when one
/// dies on an error, so the pane would sit on "working" forever without
/// the two events that do.
#[test]
fn kimi_reports_every_way_a_turn_ends() {
    for event in ["Stop", "Interrupt", "StopFailure"] {
        assert_eq!(
            KIMI_HOOK_EVENTS
                .iter()
                .find(|(hook_event, _)| *hook_event == event)
                .map(|(_, anterm_event)| *anterm_event),
            Some("stop"),
            "{event} has to end the turn like Stop does"
        );
    }
    assert!(
        !KIMI_HOOK_EVENTS
            .iter()
            .any(|(hook_event, _)| *hook_event == "Notification"),
        "Notification fires for background-task chatter and would strand the pane on waiting"
    );
}
