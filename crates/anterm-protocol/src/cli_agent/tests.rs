use super::*;

/// A later event that leaves a field out keeps what an earlier one said.
#[test]
fn the_readout_keeps_what_later_events_leave_out() {
    let mut state = AgentSessionState::default();
    let event = |readout: AgentReadout| AgentEvent {
        agent: Some(CLIAgent::Claude),
        kind: AgentEventKind::ToolComplete,
        session_id: None,
        message: None,
        cwd: None,
        source: None,
        readout,
        transcript_path: None,
    };
    state.apply_event(&event(AgentReadout {
        permission_mode: Some("plan".into()),
        model: Some("claude-opus-5-5".into()),
        ..Default::default()
    }));
    state.apply_event(&event(AgentReadout {
        effort: Some("high".into()),
        ..Default::default()
    }));
    assert_eq!(state.readout.permission_mode.as_deref(), Some("plan"));
    assert_eq!(state.readout.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(state.readout.effort.as_deref(), Some("high"));
}

#[test]
fn transcript_path_parses_and_persists_across_events() {
    let ev = parse_agent_event_body(
        br#"{"v":1,"agent":"claude","event":"session-start","session_id":"s1","transcript_path":"/Users/me/.claude/projects/p/s1.jsonl"}"#,
    )
    .expect("parses");
    assert_eq!(
        ev.transcript_path.as_deref(),
        Some(std::path::Path::new(
            "/Users/me/.claude/projects/p/s1.jsonl"
        ))
    );
    let mut state = AgentSessionState::default();
    state.apply_event(&ev);
    assert_eq!(
        state.transcript_path.as_deref(),
        Some(std::path::Path::new(
            "/Users/me/.claude/projects/p/s1.jsonl"
        ))
    );
    let follow = parse_agent_event_body(br#"{"v":1,"agent":"claude","event":"tool-complete"}"#)
        .expect("parses");
    state.apply_event(&follow);
    assert_eq!(
        state.transcript_path.as_deref(),
        Some(std::path::Path::new(
            "/Users/me/.claude/projects/p/s1.jsonl"
        )),
        "later events without transcript_path must keep the remembered path"
    );
}

#[test]
fn claude_starts_fresh_under_the_same_id_with_its_flags() {
    const ID: &str = "0b5c3a5e-6d0e-4c1f-9a4b-2f7f1d9e8c11";
    let launched = argv(&["claude", "--model", "opus", "--session-id", ID]);
    assert_eq!(
        CLIAgent::Claude
            .start_command(ID, Some(&launched))
            .as_deref(),
        Some(format!("claude --model opus --session-id {ID}").as_str())
    );
    assert_eq!(CLIAgent::Claude.start_command("not-a-uuid", None), None);
    assert_eq!(CLIAgent::Codex.start_command(ID, None), None);
}

#[test]
fn a_restore_falls_back_to_a_fresh_start_only_where_the_shell_has_or() {
    const ID: &str = "0b5c3a5e-6d0e-4c1f-9a4b-2f7f1d9e8c11";
    let launched = argv(&["claude", "--model", "opus"]);
    let resume = CLIAgent::Claude
        .resume_command(ID, Some(&launched))
        .unwrap();
    let restore = |shell| CLIAgent::Claude.restore_command(ID, Some(&launched), shell);
    let chained = format!("{resume} || claude --model opus --session-id {ID}");
    assert_eq!(restore(Some("/bin/zsh")).as_deref(), Some(chained.as_str()));
    assert_eq!(restore(Some("pwsh.exe")).as_deref(), Some(chained.as_str()));
    // Windows PowerShell 5.1 and nu reject the whole line over `||`, and a
    // shell nobody could name might be either.
    assert_eq!(restore(Some("powershell.exe")), Some(resume.clone()));
    assert_eq!(restore(Some("nu")), Some(resume.clone()));
    assert_eq!(restore(None), Some(resume));
    // Agents with no way to name a new session keep the bare resume.
    assert_eq!(
        CLIAgent::Codex.restore_command(ID, None, Some("bash")),
        CLIAgent::Codex.resume_command(ID, None)
    );
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

#[test]
fn detects_native_binaries() {
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["claude"])),
        Some(CLIAgent::Claude)
    );
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["/opt/homebrew/bin/codex", "--model", "o3"])),
        Some(CLIAgent::Codex)
    );
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["/Users/me/.local/bin/traecli"])),
        Some(CLIAgent::TraeCode)
    );
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["traex"])),
        Some(CLIAgent::TraeCode)
    );
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["/usr/local/bin/gemini"])),
        Some(CLIAgent::Gemini)
    );
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["cursor-agent"])),
        Some(CLIAgent::Cursor)
    );
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["claude/"])),
        Some(CLIAgent::Claude)
    );
}

#[test]
fn strips_leading_env_assignments() {
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["FOO=1", "BAR=baz", "claude"])),
        Some(CLIAgent::Claude)
    );
}

#[test]
fn detects_node_wrapped_claude_by_package_dir() {
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&[
            "node",
            "/Users/x/.npm/_npx/node_modules/@anthropic-ai/claude-code/cli.js",
        ])),
        Some(CLIAgent::Claude)
    );
}

/// The npm package installs `qoder` and `qodercli`, and the documentation
/// tells people to run the first one. Both are `#!/usr/bin/env node`
/// scripts, so what the pty carries is node plus the path to the shim —
/// the dispatcher's own child, which is where the name `qodercli` appears
/// on that path, is not the process group leader and is never read.
#[test]
fn qoder_is_detected_through_either_of_its_binaries() {
    for launcher in [
        "qoder",
        "qodercli",
        "/opt/homebrew/bin/qoder",
        "/opt/homebrew/bin/qodercli",
    ] {
        assert_eq!(
            CLIAgent::detect_from_argv(&argv(&["node", launcher])),
            Some(CLIAgent::QoderCLI),
            "on {launcher}"
        );
        assert_eq!(
            CLIAgent::detect_from_argv(&argv(&[launcher])),
            Some(CLIAgent::QoderCLI),
            "on {launcher}"
        );
    }
}

/// The China build installs the same dispatcher-plus-CLI pair under `-cn`
/// names, so a bare `qoder` has to stay with the global one.
#[test]
fn qoder_cn_is_detected_through_either_of_its_binaries() {
    for launcher in [
        "qodercn",
        "qoderclicn",
        "qoder-cn",
        "/c/Users/me/.qoder-cn/entry/qodercn.cmd",
    ] {
        assert_eq!(
            CLIAgent::detect_from_argv(&argv(&[launcher])),
            Some(CLIAgent::QoderCLICn),
            "on {launcher}"
        );
    }
    for other in ["qoder", "qodercli"] {
        assert_eq!(
            CLIAgent::detect_from_argv(&argv(&[other])),
            Some(CLIAgent::QoderCLI),
            "the global build keeps {other}"
        );
    }
    assert_eq!(
        CLIAgent::from_slug("qoderclicn"),
        Some(CLIAgent::QoderCLICn)
    );
}

/// All three of CodeBuddy's npm bins are node scripts pointing at one file,
/// so the pty sees node plus whichever shim name was typed.
#[test]
fn codebuddy_is_detected_through_each_of_its_binaries() {
    for launcher in [
        "codebuddy",
        "codebuddy-code",
        "cbc",
        "/opt/homebrew/bin/codebuddy",
        "/usr/local/bin/cbc",
    ] {
        assert_eq!(
            CLIAgent::detect_from_argv(&argv(&["node", launcher])),
            Some(CLIAgent::CodeBuddy),
            "on {launcher}"
        );
        assert_eq!(
            CLIAgent::detect_from_argv(&argv(&[launcher])),
            Some(CLIAgent::CodeBuddy),
            "on {launcher}"
        );
    }
    assert_eq!(
        CLIAgent::from_slug("codebuddy"),
        Some(CLIAgent::CodeBuddy),
        "hook events name the agent by this slug"
    );
}

#[test]
fn detects_npx_package_form() {
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["npx", "@anthropic-ai/claude-code"])),
        Some(CLIAgent::Claude)
    );
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["npx", "@google/gemini-cli"])),
        Some(CLIAgent::Gemini)
    );
}

#[test]
fn detects_python_wrapped_aider() {
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&[
            "python3",
            "/usr/lib/python3.12/site-packages/aider/__main__.py",
        ])),
        Some(CLIAgent::Aider)
    );
}

#[test]
fn non_interpreter_does_not_match_on_arguments() {
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["cat", "codex.md"])),
        None
    );
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["vim", "claude-code/notes.txt"])),
        None
    );
    assert_eq!(CLIAgent::detect_from_argv(&argv(&["less", "aider"])), None);
}

#[test]
fn unrelated_commands_are_none() {
    assert_eq!(CLIAgent::detect_from_argv(&argv(&["zsh"])), None);
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["node", "server.js"])),
        None
    );
    assert_eq!(CLIAgent::detect_from_argv(&argv(&[])), None);
}

#[test]
fn every_agent_has_metadata() {
    for a in CLIAgent::ALL {
        assert!(!a.display_name().is_empty());
        assert!(!a.aliases().is_empty());
        assert!(a.accent_rgb() <= 0xFFFFFF);
        assert_eq!(CLIAgent::from_slug(a.slug()), Some(a));
    }
}

#[test]
fn black_branded_avatars_keep_their_brand_field() {
    assert_eq!(CLIAgent::Codex.accent_rgb(), 0x000000);
    assert_eq!(CLIAgent::Grok.accent_rgb(), 0x000000);
}

#[test]
fn only_the_unbranded_agents_use_the_fallback_glyph() {
    let fallback: Vec<&str> = CLIAgent::ALL
        .into_iter()
        .filter(|a| a.icon_path() == "icons/bot.svg")
        .map(CLIAgent::slug)
        .collect();
    assert_eq!(
        fallback,
        [
            "aider",
            "auggie",
            "hermes",
            "vibe",
            "antigravity",
            "empryo",
            "prime-agent",
            "muse",
            "jcode"
        ]
    );
    assert!(
        !fallback.contains(&"omp"),
        "Oh My Pi ships its own mark and must not fall back"
    );
    for a in CLIAgent::ALL {
        let path = a.icon_path();
        assert!(
            path == "icons/bot.svg" || path == format!("icons/agents/{}.svg", a.slug()),
            "{} points at an unexpected {path}",
            a.display_name()
        );
    }
}

#[test]
fn muse_and_jcode_are_detected_and_resume() {
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["muse", "--yolo"])),
        Some(CLIAgent::Muse)
    );
    // The `muse` launcher execs a versioned binary.
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&[
            "/home/me/.local/share/muse/muse-bin-1.4.2",
            "--yolo"
        ])),
        Some(CLIAgent::Muse)
    );
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["/home/me/.jcode/builds/stable/jcode"])),
        Some(CLIAgent::Jcode)
    );
    assert_eq!(CLIAgent::from_slug("muse"), Some(CLIAgent::Muse));
    assert_eq!(CLIAgent::from_slug("jcode"), Some(CLIAgent::Jcode));
    assert_eq!(
        CLIAgent::Muse
            .resume_command("s-1", Some(&argv(&["muse", "--yolo"])))
            .as_deref(),
        Some("muse --yolo resume s-1")
    );
    assert_eq!(
        CLIAgent::Muse
            .resume_command("s-1", Some(&argv(&["muse", "resume", "old", "--yolo"])))
            .as_deref(),
        Some("muse --yolo resume s-1")
    );
    assert_eq!(
        CLIAgent::Muse
            .resume_command("s-1", Some(&argv(&["muse", "resume", "--last", "--yolo"])))
            .as_deref(),
        Some("muse --yolo resume s-1")
    );
    assert_eq!(
        CLIAgent::Jcode
            .resume_command("s-1", Some(&argv(&["jcode", "--resume", "old"])))
            .as_deref(),
        Some("jcode --resume s-1")
    );
    assert_eq!(CLIAgent::Muse.fork_command("s-1", None), None);
    assert_eq!(CLIAgent::Jcode.fork_command("s-1", None), None);
}

#[test]
fn empryo_and_prime_agent_are_detected_and_resume() {
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&[
            "/home/me/.empryo/bin/empryo",
            "--session",
            "750adff9"
        ])),
        Some(CLIAgent::Empryo)
    );
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["prime-agent", "--model", "x"])),
        Some(CLIAgent::PrimeAgent)
    );
    assert_eq!(CLIAgent::from_slug("empryo"), Some(CLIAgent::Empryo));
    assert_eq!(
        CLIAgent::from_slug("prime-agent"),
        Some(CLIAgent::PrimeAgent)
    );
    assert_eq!(
        CLIAgent::Empryo
            .resume_command(
                "750adff9-f366",
                Some(&argv(&["empryo", "--session", "old"]))
            )
            .as_deref(),
        Some("empryo --session 750adff9-f366")
    );
    assert_eq!(
        CLIAgent::PrimeAgent
            .resume_command(
                "s-1",
                Some(&argv(&["prime-agent", "-r", "old", "--model", "m"]))
            )
            .as_deref(),
        Some("prime-agent --model m --resume s-1")
    );
    assert_eq!(
        CLIAgent::PrimeAgent.fork_command("s-1", None).as_deref(),
        Some("prime-agent --fork s-1")
    );
    assert_eq!(
        CLIAgent::PrimeAgent
            .resume_command("s-1", Some(&argv(&["prime-agent", "--no-session"]))),
        None
    );
    assert_eq!(CLIAgent::Empryo.fork_command("s-1", None), None);
}

#[test]
fn detects_newer_agents_by_command() {
    for (cmd, agent) in [
        ("auggie", CLIAgent::Auggie),
        ("agy", CLIAgent::Antigravity),
        ("vibe-acp", CLIAgent::Vibe),
        ("grok", CLIAgent::Grok),
        ("/usr/local/bin/qwen", CLIAgent::Qwen),
        ("pi", CLIAgent::Pi),
        ("hermes", CLIAgent::Hermes),
        ("omp", CLIAgent::OhMyPi),
        ("/opt/homebrew/bin/omp", CLIAgent::OhMyPi),
        ("kimi", CLIAgent::Kimi),
        ("/usr/local/bin/kimi", CLIAgent::Kimi),
        ("crush", CLIAgent::Crush),
        ("/opt/homebrew/bin/crush", CLIAgent::Crush),
    ] {
        assert_eq!(CLIAgent::detect_from_argv(&argv(&[cmd])), Some(agent));
    }
}

/// Oh My Pi is a Pi fork, but it is its own binary with its own config
/// directory and its own flags — one must never be detected as the other.
#[test]
fn oh_my_pi_and_pi_stay_distinct() {
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["omp"])),
        Some(CLIAgent::OhMyPi)
    );
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["pi"])),
        Some(CLIAgent::Pi)
    );
    assert_eq!(CLIAgent::from_slug("omp"), Some(CLIAgent::OhMyPi));
    assert_eq!(CLIAgent::from_slug("oh-my-pi"), None);
    assert_ne!(CLIAgent::Pi.icon_path(), CLIAgent::OhMyPi.icon_path());
}

#[test]
fn custom_rules_map_wrappers_to_agents() {
    let custom: HashMap<String, String> = [("cc".to_string(), "claude".to_string())].into();
    assert_eq!(
        CLIAgent::detect_from_argv_with(&argv(&["/home/x/bin/cc", "-c"]), &custom),
        Some(CLIAgent::Claude)
    );
    let bogus: HashMap<String, String> = [("cc".to_string(), "hal9000".to_string())].into();
    assert_eq!(
        CLIAgent::detect_from_argv_with(&argv(&["cc"]), &bogus),
        None
    );
    assert_eq!(
        CLIAgent::detect_from_argv_with(&argv(&["node", "cc/cli.js"]), &custom),
        None
    );
    let shadow: HashMap<String, String> = [("codex".to_string(), "claude".to_string())].into();
    assert_eq!(
        CLIAgent::detect_from_argv_with(&argv(&["codex"]), &shadow),
        Some(CLIAgent::Codex)
    );
}

#[test]
fn detects_from_typed_command_lines() {
    let none = HashMap::new();
    assert_eq!(
        CLIAgent::detect_from_command_with("claude --resume abc", &none),
        Some(CLIAgent::Claude)
    );
    assert_eq!(
        CLIAgent::detect_from_command_with("claude.exe", &none),
        Some(CLIAgent::Claude)
    );
    assert_eq!(
        CLIAgent::detect_from_command_with(
            r"C:\Users\x\AppData\Roaming\npm\claude.cmd --model opus",
            &none
        ),
        Some(CLIAgent::Claude)
    );
    assert_eq!(
        CLIAgent::detect_from_command_with("CLAUDE", &none),
        Some(CLIAgent::Claude)
    );
    assert_eq!(
        CLIAgent::detect_from_command_with(r#"& "C:\tools\codex.exe""#, &none),
        Some(CLIAgent::Codex)
    );
    assert_eq!(
        CLIAgent::detect_from_command_with(
            r"node C:\x\node_modules\@anthropic-ai\claude-code\cli.js",
            &none
        ),
        Some(CLIAgent::Claude)
    );
    assert_eq!(
        CLIAgent::detect_from_command_with("npx.cmd @google/gemini-cli", &none),
        Some(CLIAgent::Gemini)
    );
    assert_eq!(
        CLIAgent::detect_from_command_with("notepad claude.txt", &none),
        None
    );
    assert_eq!(
        CLIAgent::detect_from_command_with("cat codex.md", &none),
        None
    );
    assert_eq!(CLIAgent::detect_from_command_with("", &none), None);
    let custom: HashMap<String, String> = [("cc".to_string(), "claude".to_string())].into();
    assert_eq!(
        CLIAgent::detect_from_command_with("cc -c", &custom),
        Some(CLIAgent::Claude)
    );
}

#[test]
fn command_argv_preserves_case_for_flag_replay() {
    assert_eq!(
        command_argv("claude --dangerously-skip-permissions"),
        ["claude", "--dangerously-skip-permissions"]
    );
    assert_eq!(
        command_argv("claude --model Opus --resume Abc-123"),
        ["claude", "--model", "Opus", "--resume", "Abc-123"]
    );
    assert_eq!(
        command_argv(r#"& "C:\Tools\claude.exe" --continue"#),
        [r"C:\Tools\claude.exe", "--continue"]
    );
    assert_eq!(command_argv("  "), [""; 0]);
    assert_eq!(
        CLIAgent::Claude
            .resume_command(
                "abc",
                Some(&command_argv("claude --dangerously-skip-permissions"))
            )
            .as_deref(),
        Some("claude --dangerously-skip-permissions --resume abc"),
        "a shell-integration command capture must round-trip into a resume \
         command that keeps the launch flags"
    );
}

#[test]
fn parses_sentinel_events() {
    let ev = parse_agent_event(
        br#"777;notify;anterm://cli-agent;{"v":1,"agent":"claude","event":"permission-request","session_id":"abc-123","message":"Claude needs your permission to use Bash"}"#,
    )
    .expect("well-formed sentinel event");
    assert_eq!(ev.agent, Some(CLIAgent::Claude));
    assert_eq!(ev.kind, AgentEventKind::PermissionRequest);
    assert_eq!(ev.session_id.as_deref(), Some("abc-123"));
    assert!(ev.message.as_deref().unwrap().contains("permission"));

    assert_eq!(parse_agent_event(b"777;notify;Build;done"), None);
    assert_eq!(
        parse_agent_event(br#"777;notify;anterm://cli-agent;{"event":"quantum-leap"}"#),
        None
    );
    assert_eq!(
        parse_agent_event(b"777;notify;anterm://cli-agent;{oops"),
        None
    );
}

#[test]
fn session_state_machine_follows_the_turn() {
    let mut s = AgentSessionState::default();
    assert_eq!(s.status, AgentStatus::Idle);

    let ev = |kind, msg: Option<&str>, id: Option<&str>| AgentEvent {
        agent: Some(CLIAgent::Claude),
        kind,
        session_id: id.map(String::from),
        message: msg.map(String::from),
        cwd: None,
        source: None,
        readout: Default::default(),
        transcript_path: None,
    };

    s.apply_event(&ev(AgentEventKind::SessionStart, None, Some("sid-1")));
    assert_eq!(s.status, AgentStatus::Idle);
    assert_eq!(s.session_id.as_deref(), Some("sid-1"));
    assert!(s.rich);

    s.apply_event(&ev(AgentEventKind::PromptSubmit, None, None));
    assert_eq!(s.status, AgentStatus::Working);

    s.apply_event(&ev(
        AgentEventKind::Notification,
        Some("Claude needs your permission"),
        None,
    ));
    assert_eq!(s.status, AgentStatus::Waiting);
    assert!(s.message.as_deref().unwrap().contains("permission"));

    s.apply_event(&ev(AgentEventKind::ToolComplete, None, None));
    assert_eq!(s.status, AgentStatus::Working);
    assert_eq!(s.message, None, "the stale permission prompt is cleared");

    s.apply_event(&ev(AgentEventKind::ToolComplete, None, None));
    assert_eq!(s.status, AgentStatus::Working);

    s.apply_event(&ev(AgentEventKind::Stop, None, None));
    assert_eq!(s.status, AgentStatus::Done);

    s.apply_event(&ev(AgentEventKind::ToolComplete, None, None));
    assert_eq!(s.status, AgentStatus::Done);

    s.apply_event(&ev(
        AgentEventKind::Notification,
        Some("Claude is waiting for your input"),
        None,
    ));
    assert_eq!(
        s.status,
        AgentStatus::Done,
        "an idle notification between turns must not fabricate a block"
    );

    s.apply_event(&ev(AgentEventKind::SessionEnd, None, None));
    assert_eq!(s.status, AgentStatus::Idle);
    assert_eq!(s.session_id.as_deref(), Some("sid-1"));
}

/// A desktop notification before any hook marks the pane waiting; the
/// first hook report must not read that as a question being answered.
#[test]
fn a_hook_after_a_notification_does_not_start_a_turn() {
    let mut s = AgentSessionState {
        status: AgentStatus::Waiting,
        message: Some("Agent's turn completed".into()),
        ..Default::default()
    };
    s.apply_event(&AgentEvent {
        agent: Some(CLIAgent::Crush),
        kind: AgentEventKind::ToolComplete,
        session_id: Some("sid".into()),
        message: None,
        cwd: None,
        source: None,
        readout: Default::default(),
        transcript_path: None,
    });
    assert_eq!(s.status, AgentStatus::Idle);
    assert_eq!(s.message, None);
    assert!(s.rich);
    assert_eq!(s.activity, 1);
}

#[test]
fn tool_completions_count_even_when_the_status_holds_still() {
    let ev = |kind| AgentEvent {
        agent: Some(CLIAgent::Claude),
        kind,
        session_id: None,
        message: None,
        cwd: None,
        source: None,
        readout: Default::default(),
        transcript_path: None,
    };

    let mut s = AgentSessionState::default();
    s.apply_event(&ev(AgentEventKind::PromptSubmit));
    assert_eq!(s.activity, 0, "a turn starting is not tool activity");

    for n in 1..=3 {
        s.apply_event(&ev(AgentEventKind::ToolComplete));
        assert_eq!(s.status, AgentStatus::Working, "the status holds still…");
        assert_eq!(s.activity, n, "…while the counter is what moves");
    }

    s.apply_event(&ev(AgentEventKind::Stop));
    s.apply_event(&ev(AgentEventKind::ToolComplete));
    assert_eq!(
        s.status,
        AgentStatus::Done,
        "and still doesn't resurrect the turn"
    );
    assert_eq!(s.activity, 4);

    s.apply_event(&ev(AgentEventKind::SessionEnd));
    assert_eq!(s.activity, 4);
}

#[test]
fn an_assumed_interrupt_ends_the_turn_until_the_agent_says_otherwise() {
    let ev = |kind| AgentEvent {
        agent: Some(CLIAgent::Claude),
        kind,
        session_id: None,
        message: None,
        cwd: None,
        source: None,
        readout: Default::default(),
        transcript_path: None,
    };

    let mut s = AgentSessionState::default();
    s.assume_interrupted();
    assert_eq!(s.status, AgentStatus::Idle, "no turn, nothing to interrupt");
    assert!(!s.inferred);

    s.apply_event(&ev(AgentEventKind::PromptSubmit));
    s.apply_event(&ev(AgentEventKind::PermissionRequest));
    s.assume_interrupted();
    assert_eq!(
        s.status,
        AgentStatus::Done,
        "a rejected prompt ends the turn"
    );
    assert_eq!(s.turns, 1);
    assert!(s.inferred);

    s.apply_event(&ev(AgentEventKind::ToolComplete));
    assert_eq!(
        s.status,
        AgentStatus::Working,
        "a tool finishing after the key proves the turn was still going"
    );
    assert!(!s.inferred);

    s.apply_event(&ev(AgentEventKind::Stop));
    assert_eq!(s.turns, 2);
    s.apply_event(&ev(AgentEventKind::ToolComplete));
    assert_eq!(
        s.status,
        AgentStatus::Done,
        "a real Stop is not a guess and stays put"
    );

    s.apply_event(&ev(AgentEventKind::PromptSubmit));
    s.assume_interrupted();
    s.apply_event(&ev(AgentEventKind::Notification));
    assert!(!s.inferred, "any real event retires the guess");
    s.apply_event(&ev(AgentEventKind::ToolComplete));
    assert_eq!(s.status, AgentStatus::Done);
}

#[test]
fn a_stale_turn_goes_idle_and_comes_back_on_the_next_tool() {
    let ev = |kind| AgentEvent {
        agent: Some(CLIAgent::Claude),
        kind,
        session_id: None,
        message: None,
        cwd: None,
        source: None,
        readout: Default::default(),
        transcript_path: None,
    };

    let mut s = AgentSessionState::default();
    s.apply_event(&ev(AgentEventKind::PromptSubmit));
    s.apply_event(&ev(AgentEventKind::PermissionRequest));
    s.assume_stale();
    assert_eq!(
        s.status,
        AgentStatus::Waiting,
        "a prompt nobody answered is not stale"
    );

    s.apply_event(&ev(AgentEventKind::ToolComplete));
    s.assume_stale();
    assert_eq!(s.status, AgentStatus::Idle);
    assert_eq!(s.turns, 0, "going quiet is not finishing");
    assert!(s.inferred);

    s.apply_event(&ev(AgentEventKind::ToolComplete));
    assert_eq!(s.status, AgentStatus::Working);
}

#[test]
fn a_session_state_from_an_older_daemon_is_not_inferred() {
    let s: AgentSessionState = serde_json::from_str(r#"{"status":"done"}"#).unwrap();
    assert!(!s.inferred);
    assert_eq!(s.since_ms, None);
}

#[test]
fn the_status_clock_restarts_only_when_the_status_moves() {
    let mut s = AgentSessionState {
        status: AgentStatus::Working,
        ..AgentSessionState::default()
    };
    s.note_transition(None, 1_000);
    assert_eq!(s.since_ms, Some(1_000));

    // Another tool finishing is news, but not a new status.
    s.note_transition(Some(AgentStatus::Working), 5_000);
    assert_eq!(s.since_ms, Some(1_000));

    s.status = AgentStatus::Waiting;
    s.note_transition(Some(AgentStatus::Working), 9_000);
    assert_eq!(s.since_ms, Some(9_000));
    assert_eq!(s.status_age_secs(189_000), Some(180));
    // A daemon clock ahead of ours reads as just now, not as a wrap.
    assert_eq!(s.status_age_secs(8_000), Some(0));
}

#[test]
fn each_finished_turn_is_counted_once() {
    let ev = |kind| AgentEvent {
        agent: Some(CLIAgent::Claude),
        kind,
        session_id: None,
        message: None,
        cwd: None,
        source: None,
        readout: Default::default(),
        transcript_path: None,
    };

    let mut s = AgentSessionState::default();
    s.apply_event(&ev(AgentEventKind::PromptSubmit));
    assert_eq!(s.turns, 0, "a turn starting has not finished anything");

    s.apply_event(&ev(AgentEventKind::Stop));
    assert_eq!(s.turns, 1);
    s.apply_event(&ev(AgentEventKind::Stop));
    assert_eq!(s.turns, 1, "a repeated stop is the same turn");
    s.apply_event(&ev(AgentEventKind::Notification));
    assert_eq!(s.turns, 1);

    s.apply_event(&ev(AgentEventKind::PromptSubmit));
    s.apply_event(&ev(AgentEventKind::Stop));
    assert_eq!(s.turns, 2, "a second turn is a second count");

    s.apply_event(&ev(AgentEventKind::SessionEnd));
    assert_eq!(s.turns, 2);
}

#[test]
fn session_state_tracks_and_releases_the_agent_cwd() {
    use std::path::PathBuf;

    let ev = |kind, cwd: Option<&str>| AgentEvent {
        agent: Some(CLIAgent::Claude),
        kind,
        session_id: None,
        message: None,
        cwd: cwd.map(PathBuf::from),
        source: None,
        readout: Default::default(),
        transcript_path: None,
    };

    let mut s = AgentSessionState::default();
    s.apply_event(&ev(AgentEventKind::SessionStart, Some("/repo")));
    assert_eq!(s.cwd.as_deref(), Some(std::path::Path::new("/repo")));

    s.apply_event(&ev(
        AgentEventKind::ToolComplete,
        Some("/repo/.claude/worktrees/fix-x"),
    ));
    assert_eq!(
        s.cwd.as_deref(),
        Some(std::path::Path::new("/repo/.claude/worktrees/fix-x"))
    );

    s.apply_event(&ev(AgentEventKind::Stop, None));
    assert_eq!(
        s.cwd.as_deref(),
        Some(std::path::Path::new("/repo/.claude/worktrees/fix-x"))
    );

    s.apply_event(&ev(AgentEventKind::SessionEnd, None));
    assert_eq!(s.cwd, None, "session end releases the cwd claim");
}

#[test]
fn resume_commands_are_shell_safe() {
    assert_eq!(
        CLIAgent::Claude.resume_command("abc-123", None).as_deref(),
        Some("claude --resume abc-123")
    );
    assert_eq!(
        CLIAgent::Codex.resume_command("th_read.9", None).as_deref(),
        Some("codex resume th_read.9")
    );
    assert_eq!(
        CLIAgent::TraeCode
            .resume_command("019c-123", None)
            .as_deref(),
        Some("traecli resume 019c-123")
    );
    assert_eq!(
        CLIAgent::Pi
            .resume_command("0199c3f2-1b0e-7c3a-9f21-6d4b8e2a5c17", None)
            .as_deref(),
        Some("pi --session 0199c3f2-1b0e-7c3a-9f21-6d4b8e2a5c17")
    );
    assert_eq!(
        CLIAgent::Kimi.resume_command("abc-123", None).as_deref(),
        Some("kimi --session abc-123")
    );
    assert_eq!(CLIAgent::Aider.resume_command("abc", None), None);
    assert_eq!(CLIAgent::Claude.resume_command("abc; rm -rf /", None), None);
    assert_eq!(CLIAgent::Claude.resume_command("$(boom)", None), None);
    assert_eq!(CLIAgent::Claude.resume_command("", None), None);
}

#[test]
fn resume_carries_launch_flags() {
    let argv = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    assert_eq!(
        CLIAgent::Claude
            .resume_command(
                "abc-123",
                Some(&argv(&["claude", "--dangerously-skip-permissions"]))
            )
            .as_deref(),
        Some("claude --dangerously-skip-permissions --resume abc-123")
    );
    assert_eq!(
        CLIAgent::Claude
            .resume_command("abc", Some(&argv(&["claude", "--model", "opus"])))
            .as_deref(),
        Some("claude --model opus --resume abc")
    );
    assert_eq!(
        CLIAgent::Kimi
            .resume_command(
                "abc-123",
                Some(&argv(&["kimi", "--session", "old", "--yolo"]))
            )
            .as_deref(),
        Some("kimi --yolo --session abc-123"),
        "a stale --session flag comes off before the new one goes on"
    );
    assert_eq!(
        CLIAgent::Kimi
            .resume_command("abc-123", Some(&argv(&["kimi", "--session=old", "--yolo"])))
            .as_deref(),
        Some("kimi --yolo --session abc-123"),
        "and so does the one-token spelling of it"
    );
    assert_eq!(
        CLIAgent::Kimi
            .resume_command(
                "abc-123",
                Some(&argv(&["kimi", "--resume", "--model", "kimi-k2"]))
            )
            .as_deref(),
        Some("kimi --model kimi-k2 --session abc-123"),
        "`--session` takes an optional id, so a bare one must not eat the flag after it"
    );
    assert_eq!(
        CLIAgent::Kimi
            .resume_command(
                "abc-123",
                Some(&argv(&["kimi", "--continue", "--model", "kimi-k2"]))
            )
            .as_deref(),
        Some("kimi --model kimi-k2 --session abc-123"),
        "`--continue` is mutually exclusive with `--session` and takes no value"
    );
    assert_eq!(
        CLIAgent::Kimi
            .resume_command(
                "abc-123",
                Some(&argv(&["kimi", "--agent", "reviewer", "--yolo"]))
            )
            .as_deref(),
        Some("kimi --yolo --session abc-123"),
        "Kimi rejects `--agent` next to `--session`, and resume rebinds the agent itself"
    );
    assert_eq!(
        CLIAgent::Claude
            .resume_command(
                "abc",
                Some(&argv(&[
                    "node",
                    "/x/node_modules/@anthropic-ai/claude-code/cli.js",
                    "--dangerously-skip-permissions",
                ]))
            )
            .as_deref(),
        Some("claude --dangerously-skip-permissions --resume abc")
    );
    assert_eq!(
        CLIAgent::Claude
            .resume_command(
                "new-id",
                Some(&argv(&["claude", "--resume", "old-id", "--model", "opus"]))
            )
            .as_deref(),
        Some("claude --model opus --resume new-id")
    );
    assert_eq!(
        CLIAgent::Codex
            .resume_command("id-1", Some(&argv(&["codex", "--yolo"])))
            .as_deref(),
        Some("codex resume id-1 --yolo")
    );
    assert_eq!(
        CLIAgent::Codex
            .resume_command("id-2", Some(&argv(&["codex", "resume", "id-1", "--yolo"])))
            .as_deref(),
        Some("codex resume id-2 --yolo")
    );
    assert_eq!(
        CLIAgent::Claude
            .resume_command(
                "abc",
                Some(&argv(&["claude", "--allowedTools", "Bash(git:*)"]))
            )
            .as_deref(),
        Some("claude --resume abc")
    );
    assert_eq!(
        CLIAgent::Claude
            .resume_command("abc", Some(&argv(&["claude", "fix-the-bug"])))
            .as_deref(),
        Some("claude --resume abc")
    );
    assert_eq!(
        CLIAgent::Claude
            .resume_command(
                "abc",
                Some(&argv(&[
                    "CLAUDE_CONFIG_DIR=/opt/claude",
                    "claude",
                    "--dangerously-skip-permissions",
                ]))
            )
            .as_deref(),
        Some("claude --dangerously-skip-permissions --resume abc")
    );
    assert_eq!(
        CLIAgent::Claude
            .resume_command(
                "abc",
                Some(&argv(&["claude", "--model", "opus", "review", "this"]))
            )
            .as_deref(),
        Some("claude --resume abc")
    );
    assert_eq!(
        CLIAgent::Codex
            .resume_command(
                "id-3",
                Some(&argv(&["codex", "resume", "--last", "--yolo"]))
            )
            .as_deref(),
        Some("codex resume id-3 --yolo")
    );
    assert_eq!(
        CLIAgent::Pi
            .resume_command("id-a", Some(&argv(&["pi", "--model", "opus"])))
            .as_deref(),
        Some("pi --model opus --session id-a")
    );
    assert_eq!(
        CLIAgent::Pi
            .resume_command(
                "id-b",
                Some(&argv(&[
                    "pi",
                    "--session",
                    "old-id",
                    "--fork",
                    "old",
                    "-c",
                    "--model",
                    "opus"
                ]))
            )
            .as_deref(),
        Some("pi --model opus --session id-b")
    );
    assert_eq!(
        CLIAgent::Pi.resume_command(
            "id-x",
            Some(&argv(&["pi", "--no-session", "--model", "opus"]))
        ),
        None
    );
    assert_eq!(
        CLIAgent::Pi
            .resume_command(
                "id-c",
                Some(&argv(&[
                    "pi",
                    "--session-dir",
                    "/w/.sessions",
                    "--fork",
                    "old"
                ]))
            )
            .as_deref(),
        Some("pi --session-dir /w/.sessions --session id-c")
    );
    assert_eq!(
        CLIAgent::Claude
            .resume_command(
                "abc",
                Some(&argv(&["cc", "--dangerously-skip-permissions"]))
            )
            .as_deref(),
        Some("claude --resume abc")
    );
    assert_eq!(
        CLIAgent::Amp
            .resume_command("t-1", Some(&argv(&["amp", "--dangerously-allow-all"])))
            .as_deref(),
        Some("amp threads continue t-1 --dangerously-allow-all")
    );
    assert_eq!(
        CLIAgent::Amp
            .resume_command("t-2", Some(&argv(&["amp", "threads", "continue", "t-1"])))
            .as_deref(),
        Some("amp threads continue t-2")
    );
    assert_eq!(
        CLIAgent::Copilot
            .resume_command(
                "s-9",
                Some(&argv(&["copilot", "--resume", "s-1", "--allow-all-tools"]))
            )
            .as_deref(),
        Some("copilot --allow-all-tools --resume s-9")
    );
    assert_eq!(
        CLIAgent::Copilot.resume_command("s-9", None).as_deref(),
        Some("copilot --resume s-9")
    );
    assert_eq!(
        CLIAgent::Grok
            .resume_command("g-2", Some(&argv(&["grok", "--model", "grok-code"])))
            .as_deref(),
        Some("grok --model grok-code --resume g-2")
    );
    assert_eq!(
        CLIAgent::Grok
            .resume_command(
                "g-2",
                Some(&argv(&["grok", "--resume", "g-1", "--fork-session"]))
            )
            .as_deref(),
        Some("grok --resume g-2")
    );
    assert_eq!(
        CLIAgent::Grok
            .resume_command(
                "g-3",
                Some(&argv(&["grok", "-w", "--worktree-ref", "main", "--yolo"]))
            )
            .as_deref(),
        Some("grok --yolo --resume g-3")
    );
    assert_eq!(
        CLIAgent::QoderCLI
            .resume_command("q-1", Some(&argv(&["qodercli", "--model", "qoder-1"])))
            .as_deref(),
        Some("qodercli --model qoder-1 --resume q-1")
    );
    assert_eq!(
        CLIAgent::QoderCLI
            .resume_command(
                "q-2",
                Some(&argv(&["qodercli", "--resume", "q-1", "--fork-session"]))
            )
            .as_deref(),
        Some("qodercli --resume q-2"),
        "a stale --resume id and --fork-session come off before the new one goes on"
    );
    assert_eq!(
        CLIAgent::QoderCLI
            .resume_command(
                "q-3",
                Some(&argv(&["qodercli", "--session-id", "old", "--yolo"]))
            )
            .as_deref(),
        Some("qodercli --yolo --resume q-3"),
        "`--session-id` names a new session and is rejected next to `--resume`"
    );
}

#[test]
fn qoder_resume_and_fork_do_not_recreate_worktrees() {
    for worktree in [
        vec!["--worktree"],
        vec!["--worktree", "old-tree"],
        vec!["--worktree=old-tree"],
    ] {
        for cwd_flag in ["-w", "--cwd"] {
            let mut launch = argv(&["qodercli", "--model", "qoder-1"]);
            launch.extend(argv(&worktree));
            launch.extend(argv(&[cwd_flag, "/repo/current-tree"]));
            assert_eq!(
                CLIAgent::QoderCLI.resume_command("q-1", Some(&launch)),
                Some(format!(
                    "qodercli --model qoder-1 {cwd_flag} /repo/current-tree --resume q-1"
                )),
                "launch argv: {launch:?}"
            );
            assert_eq!(
                CLIAgent::QoderCLI.fork_command("q-1", Some(&launch)),
                Some(format!(
                    "qodercli --model qoder-1 {cwd_flag} /repo/current-tree --resume q-1 --fork-session"
                )),
                "launch argv: {launch:?}"
            );
        }
    }
}

#[test]
fn qoder_session_commands_require_persistence() {
    let ephemeral = argv(&["qodercli", "--print", "--no-session-persistence"]);
    assert_eq!(
        CLIAgent::QoderCLI.resume_command("q-1", Some(&ephemeral)),
        None
    );
    assert_eq!(
        CLIAgent::QoderCLI.fork_command("q-1", Some(&ephemeral)),
        None
    );

    let persistent = argv(&["qodercli", "--model", "qoder-1"]);
    assert_eq!(
        CLIAgent::QoderCLI
            .resume_command("q-1", Some(&persistent))
            .as_deref(),
        Some("qodercli --model qoder-1 --resume q-1")
    );
    assert_eq!(
        CLIAgent::QoderCLI
            .fork_command("q-1", Some(&persistent))
            .as_deref(),
        Some("qodercli --model qoder-1 --resume q-1 --fork-session")
    );
}

#[test]
fn crush_resumes_by_session_and_keeps_its_cwd_flag() {
    let argv = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    assert_eq!(
        CLIAgent::Crush.resume_command("c-1", None).as_deref(),
        Some("crush --session c-1")
    );
    assert_eq!(
        CLIAgent::Crush
            .resume_command("c-2", Some(&argv(&["crush", "--session", "c-1", "--yolo"])))
            .as_deref(),
        Some("crush --yolo --session c-2"),
        "a stale --session and its id come off before the new one goes on"
    );
    assert_eq!(
        CLIAgent::Crush
            .resume_command("c-3", Some(&argv(&["crush", "--continue", "-C", "--yolo"])))
            .as_deref(),
        Some("crush --yolo --session c-3"),
        "both spellings of continue are stale with it"
    );
    // `-c` is `--cwd` in Crush, not `--continue`, and must survive.
    assert_eq!(
        CLIAgent::Crush
            .resume_command("c-4", Some(&argv(&["crush", "-c", "/repo/tree", "--yolo"])))
            .as_deref(),
        Some("crush -c /repo/tree --yolo --session c-4")
    );
    assert_eq!(
        CLIAgent::Crush.fork_command("c-1", None),
        None,
        "Crush has no fork command"
    );
}

#[test]
fn codebuddy_resume_and_fork_drop_stale_session_and_worktree_flags() {
    assert_eq!(
        CLIAgent::CodeBuddy.resume_command("cb-1", None).as_deref(),
        Some("codebuddy --resume cb-1")
    );
    assert_eq!(
        CLIAgent::CodeBuddy.fork_command("cb-1", None).as_deref(),
        Some("codebuddy --resume cb-1 --fork-session")
    );
    for stale in [
        vec!["--resume", "cb-0"],
        vec!["-r", "cb-0"],
        vec!["--continue"],
        vec!["-c"],
        vec!["--session-id", "cb-0"],
        vec!["--resume", "cb-0", "--resume-session-at", "m-9"],
        vec!["--worktree"],
        vec!["--worktree", "old-tree", "--worktree-branch", "main"],
        vec!["-w", "old-tree"],
        vec!["--worktree=old-tree"],
        vec!["--resume", "cb-0", "--fork-session"],
    ] {
        let mut launch = argv(&["codebuddy", "--model", "m-1"]);
        launch.extend(argv(&stale));
        launch.extend(argv(&["--permission-mode", "acceptEdits"]));
        assert_eq!(
            CLIAgent::CodeBuddy
                .resume_command("cb-2", Some(&launch))
                .as_deref(),
            Some("codebuddy --model m-1 --permission-mode acceptEdits --resume cb-2"),
            "launch argv: {launch:?}"
        );
        assert_eq!(
            CLIAgent::CodeBuddy
                .fork_command("cb-2", Some(&launch))
                .as_deref(),
            Some(
                "codebuddy --model m-1 --permission-mode acceptEdits --resume cb-2 --fork-session"
            ),
            "launch argv: {launch:?}"
        );
    }

    let ephemeral = argv(&["codebuddy", "-p", "--no-session-persistence"]);
    assert_eq!(
        CLIAgent::CodeBuddy.resume_command("cb-1", Some(&ephemeral)),
        None,
        "nothing was saved, so there is nothing to reopen"
    );
    assert_eq!(
        CLIAgent::CodeBuddy.fork_command("cb-1", Some(&ephemeral)),
        None
    );
}

#[test]
fn oh_my_pi_resume_and_fork_use_its_own_flags() {
    let argv = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    assert_eq!(
        CLIAgent::OhMyPi.resume_command("s-1", None).as_deref(),
        Some("omp --resume s-1")
    );
    assert_eq!(
        CLIAgent::OhMyPi.fork_command("s-1", None).as_deref(),
        Some("omp --fork s-1")
    );
    assert_eq!(
        CLIAgent::OhMyPi
            .resume_command(
                "s-2",
                Some(&argv(&["omp", "--session", "s-1", "--model", "opus"]))
            )
            .as_deref(),
        Some("omp --model opus --resume s-2"),
        "--session is a third spelling of --resume and sheds with it"
    );
    assert_eq!(
        CLIAgent::OhMyPi
            .fork_command(
                "s-2",
                Some(&argv(&["omp", "--fork", "s-1", "-c", "--yolo"]))
            )
            .as_deref(),
        Some("omp --yolo --fork s-2")
    );
    assert_eq!(
        CLIAgent::OhMyPi
            .resume_command(
                "s-3",
                Some(&argv(&["omp", "--session-dir", "/w/.sessions"]))
            )
            .as_deref(),
        Some("omp --session-dir /w/.sessions --resume s-3"),
        "--session-dir is a different flag and rides along"
    );

    // `--no-session` means the run never persisted one, so there is
    // nothing to resume from and Oh My Pi rejects `--fork` outright.
    for id in ["s-4"] {
        assert_eq!(
            CLIAgent::OhMyPi.resume_command(id, Some(&argv(&["omp", "--no-session"]))),
            None
        );
        assert_eq!(
            CLIAgent::OhMyPi.fork_command(id, Some(&argv(&["omp", "--no-session"]))),
            None
        );
    }
}

#[test]
fn fork_commands_cover_exactly_the_agents_with_a_verified_fork() {
    assert_eq!(
        CLIAgent::Codex.fork_command("abc-123", None).as_deref(),
        Some("codex fork abc-123")
    );
    assert_eq!(
        CLIAgent::TraeCode.fork_command("019c-123", None).as_deref(),
        Some("traecli fork 019c-123")
    );
    assert_eq!(
        CLIAgent::Claude.fork_command("abc-123", None).as_deref(),
        Some("claude --resume abc-123 --fork-session")
    );
    assert_eq!(
        CLIAgent::Grok.fork_command("g-1", None).as_deref(),
        Some("grok --resume g-1 --fork-session")
    );
    assert_eq!(
        CLIAgent::OpenCode.fork_command("s-1", None).as_deref(),
        Some("opencode --session s-1 --fork")
    );
    assert_eq!(
        CLIAgent::Droid.fork_command("session-abc", None).as_deref(),
        Some("droid --fork session-abc")
    );
    assert_eq!(
        CLIAgent::Qwen.fork_command("q-1", None).as_deref(),
        Some("qwen --resume q-1 --fork-session")
    );
    assert_eq!(
        CLIAgent::Goose.fork_command("20260213_9", None).as_deref(),
        Some("goose session --resume --fork --session-id 20260213_9")
    );
    // Undocumented in `amp threads --help`, but `amp threads fork --help`
    // prints its own usage, so the subcommand is real.
    assert_eq!(
        CLIAgent::Amp.fork_command("T-abc", None).as_deref(),
        Some("amp threads fork T-abc")
    );

    // Cursor and Antigravity fork only from inside a running TUI (`/fork`),
    // which is not something a launch command line can reach.
    for agent in [
        CLIAgent::Gemini,
        CLIAgent::Copilot,
        CLIAgent::Cursor,
        CLIAgent::Aider,
        CLIAgent::Auggie,
        CLIAgent::Hermes,
        CLIAgent::Vibe,
        CLIAgent::Antigravity,
    ] {
        assert_eq!(
            agent.fork_command("abc", None),
            None,
            "{} must not claim a fork command",
            agent.slug()
        );
    }

    for agent in CLIAgent::ALL {
        assert_eq!(
            agent.fork_label().is_some(),
            agent.fork_command("abc", None).is_some(),
            "{}: fork_label and fork_command disagree",
            agent.slug()
        );
    }
}

#[test]
fn fork_commands_are_shell_safe() {
    for id in [
        "abc; rm -rf /",
        "$(boom)",
        "",
        "a b",
        "--dangerously-skip-permissions",
    ] {
        assert_eq!(
            CLIAgent::Codex.fork_command(id, None),
            None,
            "codex accepted a non-token id: {id:?}"
        );
        assert_eq!(CLIAgent::Claude.fork_command(id, None), None);
    }
}

#[test]
fn fork_carries_launch_flags_and_sheds_stale_session_targeting() {
    let argv = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    assert_eq!(
        CLIAgent::Codex
            .fork_command("id-1", Some(&argv(&["codex", "--yolo"])))
            .as_deref(),
        Some("codex fork id-1 --yolo")
    );
    assert_eq!(
        CLIAgent::Claude
            .fork_command(
                "abc",
                Some(&argv(&["claude", "--dangerously-skip-permissions"]))
            )
            .as_deref(),
        Some("claude --dangerously-skip-permissions --resume abc --fork-session")
    );

    assert_eq!(
        CLIAgent::Codex
            .fork_command("id-2", Some(&argv(&["codex", "fork", "id-1", "--yolo"])))
            .as_deref(),
        Some("codex fork id-2 --yolo")
    );
    assert_eq!(
        CLIAgent::Claude
            .fork_command(
                "new",
                Some(&argv(&["claude", "--resume", "old", "--fork-session"]))
            )
            .as_deref(),
        Some("claude --resume new --fork-session")
    );
    assert_eq!(
        CLIAgent::Grok
            .fork_command(
                "g-2",
                Some(&argv(&["grok", "--resume", "g-1", "--fork-session"]))
            )
            .as_deref(),
        Some("grok --resume g-2 --fork-session")
    );
    assert_eq!(
        CLIAgent::OpenCode
            .fork_command(
                "s-2",
                Some(&argv(&["opencode", "--session", "s-1", "--fork"]))
            )
            .as_deref(),
        Some("opencode --session s-2 --fork")
    );

    assert_eq!(
        CLIAgent::Codex
            .resume_command("id-2", Some(&argv(&["codex", "fork", "id-1", "--yolo"])))
            .as_deref(),
        Some("codex resume id-2 --yolo")
    );
    assert_eq!(
        CLIAgent::Claude
            .resume_command(
                "new",
                Some(&argv(&["claude", "--resume", "old", "--fork-session"]))
            )
            .as_deref(),
        Some("claude --resume new")
    );
    assert_eq!(
        CLIAgent::OpenCode
            .resume_command(
                "s-2",
                Some(&argv(&["opencode", "--session", "s-1", "--fork"]))
            )
            .as_deref(),
        Some("opencode --session s-2")
    );
}

#[test]
fn newly_wired_agents_resume_the_way_their_own_cli_spells_it() {
    let argv = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    for (agent, id, want) in [
        (CLIAgent::Droid, "session-abc", "droid --resume session-abc"),
        (CLIAgent::Qwen, "q-1", "qwen --resume q-1"),
        (CLIAgent::Auggie, "a-1", "auggie --resume a-1"),
        (
            CLIAgent::Goose,
            "20260213_9",
            "goose session --resume --session-id 20260213_9",
        ),
        (
            CLIAgent::Hermes,
            "20260812_213234_5de948",
            "hermes chat --resume 20260812_213234_5de948",
        ),
        (CLIAgent::Vibe, "v-1", "vibe --resume v-1"),
        (
            CLIAgent::Antigravity,
            "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
            "agy --conversation a1b2c3d4-e5f6-7890-abcd-ef1234567890",
        ),
    ] {
        assert_eq!(
            agent.resume_command(id, None).as_deref(),
            Some(want),
            "{} resumes with the wrong command",
            agent.slug()
        );
    }

    // A subcommand-addressed session leaves nothing in `stale` to strip, so
    // the prefix has to be dropped structurally — otherwise the launch flags
    // go down with it.
    assert_eq!(
        CLIAgent::Amp
            .resume_command(
                "T-2",
                Some(&argv(&[
                    "amp",
                    "threads",
                    "continue",
                    "T-1",
                    "--dangerously-allow-all",
                ]))
            )
            .as_deref(),
        Some("amp threads continue T-2 --dangerously-allow-all")
    );
    assert_eq!(
        CLIAgent::Goose
            .resume_command(
                "20260213_9",
                Some(&argv(&["goose", "session", "--resume", "--name", "old"]))
            )
            .as_deref(),
        Some("goose session --resume --session-id 20260213_9")
    );
    assert_eq!(
        CLIAgent::Auggie
            .resume_command(
                "a-2",
                Some(&argv(&["auggie", "session", "resume", "a-1", "--verbose"]))
            )
            .as_deref(),
        Some("auggie --verbose --resume a-2")
    );
    assert_eq!(
        CLIAgent::Droid
            .resume_command(
                "s-2",
                Some(&argv(&["droid", "--fork", "s-1", "--auto", "low"]))
            )
            .as_deref(),
        Some("droid --auto low --resume s-2")
    );

    // `--id` is an alias of `--session-id` and `-n` of `--name`, and the
    // three share one exclusive clap group — any of them surviving next to
    // the `--session-id` the command appends would fail to parse.
    assert_eq!(
        CLIAgent::Goose
            .resume_command(
                "20260213_9",
                Some(&argv(&["goose", "s", "--resume", "--id", "20260101_1"]))
            )
            .as_deref(),
        Some("goose session --resume --session-id 20260213_9")
    );
    assert_eq!(
        CLIAgent::Goose
            .resume_command(
                "20260213_9",
                Some(&argv(&["goose", "session", "-r", "-n", "old"]))
            )
            .as_deref(),
        Some("goose session --resume --session-id 20260213_9")
    );
    // Qwen rejects `--session-id` alongside `--resume`; Vibe spells
    // `--continue` as `-c` too.
    assert_eq!(
        CLIAgent::Qwen
            .resume_command("q-2", Some(&argv(&["qwen", "--session-id", "old"])))
            .as_deref(),
        Some("qwen --resume q-2")
    );
    assert_eq!(
        CLIAgent::Vibe
            .resume_command("v-2", Some(&argv(&["vibe", "-c"])))
            .as_deref(),
        Some("vibe --resume v-2")
    );

    // Nothing was persisted, so there is nothing to resume or fork.
    for id in ["a-1"] {
        assert_eq!(
            CLIAgent::Auggie
                .resume_command(id, Some(&argv(&["auggie", "--dont-save-session"]))),
            None
        );
    }
    // "If false, chat history is not saved and --continue/--resume will
    // not work" — so neither resume nor fork is offered.
    let no_recording = argv(&["qwen", "--no-chat-recording"]);
    assert_eq!(
        CLIAgent::Qwen.resume_command("q-1", Some(&no_recording)),
        None
    );
    assert_eq!(
        CLIAgent::Qwen.fork_command("q-1", Some(&no_recording)),
        None
    );
}

/// `python3 -m antigravity` opens an xkcd comic. It is the standard way to
/// trigger Python's easter egg, and the interpreter branch used to read that
/// module name as an agent.
#[test]
fn the_python_easter_egg_is_not_a_coding_agent() {
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["python3", "-m", "antigravity"])),
        None
    );
    assert_eq!(
        CLIAgent::detect_from_argv(&argv(&["agy"])),
        Some(CLIAgent::Antigravity)
    );
}

#[test]
fn status_metadata_is_consistent() {
    assert_eq!(AgentStatus::Idle.dot_rgb(), None);
    for st in [
        AgentStatus::Working,
        AgentStatus::Waiting,
        AgentStatus::Done,
    ] {
        assert!(st.dot_rgb().is_some());
    }
    assert_eq!(
        serde_json::to_string(&AgentStatus::Waiting).unwrap(),
        "\"waiting\""
    );
}

fn overrides(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn a_launch_is_the_bare_binary_unless_overridden() {
    let none = HashMap::new();
    assert_eq!(CLIAgent::Claude.launch_command(&none), "claude");
    assert_eq!(CLIAgent::Codex.launch_command(&none), "codex");
    assert_eq!(CLIAgent::Cursor.launch_command(&none), "cursor-agent");
    assert_eq!(CLIAgent::Antigravity.launch_command(&none), "agy");
    assert_eq!(CLIAgent::QoderCLI.launch_command(&none), "qoder");

    let set = overrides(&[
        ("claude", "  claude --dangerously-skip-permissions "),
        // Keys are read the way `from_slug` reads them.
        ("Codex", "codex --model o3"),
        // A blank entry is no override at all.
        ("gemini", "   "),
    ]);
    assert_eq!(
        CLIAgent::Claude.launch_command(&set),
        "claude --dangerously-skip-permissions"
    );
    assert_eq!(CLIAgent::Codex.launch_command(&set), "codex --model o3");
    assert_eq!(CLIAgent::Gemini.launch_command(&set), "gemini");
    assert_eq!(CLIAgent::Aider.launch_command(&set), "aider");
}

#[test]
fn every_agent_launches_as_a_name_it_is_detected_by() {
    let none = HashMap::new();
    for agent in CLIAgent::ALL {
        let argv = command_argv(&agent.launch_command(&none));
        assert_eq!(
            CLIAgent::detect_from_argv(&argv),
            Some(agent),
            "{agent:?} would launch as something detection does not know"
        );
    }
}

#[test]
fn a_wrapper_named_by_a_launch_override_is_detected_as_its_agent() {
    let aliases = launch_aliases(&overrides(&[("claude", "FOO=1 cc --fast")]));
    // Run as a binary, and run as a script under its interpreter.
    for run in [
        argv(&["cc", "--fast"]),
        argv(&["/Users/me/bin/cc", "--fast"]),
        argv(&["/bin/bash", "/Users/me/bin/cc", "--fast"]),
        argv(&["node", "/Users/me/bin/cc.js", "--fast"]),
    ] {
        assert_eq!(
            CLIAgent::detect_from_argv_with(&run, &aliases),
            Some(CLIAgent::Claude),
            "{run:?}"
        );
    }
    // Without the override the same wrapper is nobody.
    assert_eq!(
        CLIAgent::detect_from_argv_with(&argv(&["cc", "--fast"]), &HashMap::new()),
        None
    );
}

#[test]
fn a_launch_override_never_maps_an_interpreter_or_a_real_agent_name() {
    let aliases = launch_aliases(&overrides(&[
        ("claude", "npx @anthropic-ai/claude-code"),
        ("codex", "bash -lc codex"),
        // Already detected as itself; mapping it would add nothing.
        ("gemini", "gemini --yolo"),
        ("not-an-agent", "cc"),
    ]));
    assert!(aliases.is_empty(), "{aliases:?}");
    // A shell running something else is still nobody.
    assert_eq!(
        CLIAgent::detect_from_argv_with(
            &argv(&["bash", "./build.sh"]),
            &launch_aliases(&overrides(&[("claude", "cc")]))
        ),
        None
    );
}

#[test]
fn launch_args_drop_the_session_and_the_prompt_but_keep_the_flags() {
    assert_eq!(
        CLIAgent::Claude.launch_argv_for_default(&argv(&[
            "node",
            "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js",
            "--model",
            "opus",
            "--resume",
            "abc-123",
            "--append-system-prompt",
            "be terse",
            "fix the bug",
        ])),
        Some(argv(&[
            "claude",
            "--model",
            "opus",
            "--append-system-prompt",
            "be terse",
        ]))
    );
    assert_eq!(
        CLIAgent::Codex.launch_argv_for_default(&argv(&[
            "codex",
            "resume",
            "019a",
            "--model=o3",
            "prompt",
        ])),
        Some(argv(&["codex", "--model=o3"]))
    );
    // A wrapper cannot be taken apart; what was typed is what launched it.
    assert_eq!(
        CLIAgent::Claude.launch_argv_for_default(&argv(&["cc", "--fast", "x"])),
        Some(argv(&["cc", "--fast", "x"]))
    );
    assert_eq!(CLIAgent::Claude.launch_argv_for_default(&[]), None);
}

#[test]
fn program_on_path_finds_only_executables() {
    let dir = tempfile::TempDir::new().unwrap();
    let bin = dir.path().join("claude");
    std::fs::write(&bin, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::write(dir.path().join("codex"), "not executable").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!program_on_path("codex", dir.path().as_os_str()));
    }
    std::fs::create_dir(dir.path().join("gemini")).unwrap();
    let path =
        std::env::join_paths([std::path::Path::new("/nonexistent"), dir.path()]).unwrap();
    assert!(program_on_path("claude", &path));
    assert!(
        !program_on_path("gemini", &path),
        "a directory is not a program"
    );
    assert!(!program_on_path("aider", &path));
    assert!(program_on_path(
        bin.to_str().unwrap(),
        std::ffi::OsStr::new("")
    ));
    assert!(!program_on_path("", &path));
}
