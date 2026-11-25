use std::io;
use std::path::{Path, PathBuf};

use super::target::{HookAgent, HookOutcome, HookTarget, HooksState, home_dir};

pub(super) const OWNED_FILE_STEM_JSON: &str = "anterm.json";
pub(super) const OWNED_FILE_STEM_JS: &str = "anterm.js";

/// `PermissionRequest` fires the moment the permission dialog opens; the
/// `Notification` that also reports it can trail by seconds, and stays as the
/// fallback for older Claude builds that do not know the event. A hook that
/// prints nothing leaves the decision to the dialog. `PreToolUse` is there for
/// `AskUserQuestion` alone (see [`super::runner::QUESTION_TOOLS`]).
pub(super) const CLAUDE_HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt-submit"),
    ("PermissionRequest", "permission-request"),
    ("PreToolUse", "tool-start"),
    ("Notification", "notification"),
    ("PostToolUse", "tool-complete"),
    ("Stop", "stop"),
    ("SessionEnd", "session-end"),
];

/// Codex's `PermissionRequest` answered with no output falls through to its
/// own approval prompt, so it reports the wait without deciding anything.
/// `PostToolUse` is what takes the pane off waiting once the approved tool has
/// run, and `PreToolUse` catches `request_user_input` (see
/// [`super::runner::QUESTION_TOOLS`]), which Codex runs without asking for approval.
pub(super) const CODEX_HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt-submit"),
    ("PermissionRequest", "permission-request"),
    ("PreToolUse", "tool-start"),
    ("PostToolUse", "tool-complete"),
    ("Stop", "stop"),
];

pub(super) const TRAE_CODE_HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt-submit"),
    ("PermissionRequest", "permission-request"),
    ("PostToolUse", "tool-complete"),
    ("Stop", "stop"),
    ("SessionEnd", "session-end"),
];

/// Gemini names the turn boundaries after the agent rather than the user, and
/// omitting `matcher` matches everything (`hookPlanner.ts`, `!entry.matcher`),
/// so the bare entries [`hook_map_install`] already writes are enough.
pub(super) const GEMINI_HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("BeforeAgent", "prompt-submit"),
    ("Notification", "notification"),
    ("AfterTool", "tool-complete"),
    ("AfterAgent", "stop"),
    ("SessionEnd", "session-end"),
];

/// Empryo uses Claude-compatible lifecycle hooks in ~/.empryo/hooks.json.
pub(super) const EMPRYO_HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt-submit"),
    ("PostToolUse", "tool-complete"),
    ("Stop", "stop"),
    ("StopFailure", "stop"),
    ("SessionEnd", "session-end"),
];

pub(super) const DROID_HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt-submit"),
    ("Notification", "notification"),
    ("PostToolUse", "tool-complete"),
    ("Stop", "stop"),
    ("SessionEnd", "session-end"),
];

/// Qwen is the only agent here with a first-class permission event, so it needs
/// none of the notification sniffing in [`effective_event`] — and it gets no
/// `Notification` hook at all, which would only muddy a status the dedicated
/// event already reports precisely.
pub(super) const QWEN_HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt-submit"),
    ("PermissionRequest", "permission-request"),
    ("PostToolUse", "tool-complete"),
    ("Stop", "stop"),
    ("SessionEnd", "session-end"),
];

/// jcode's lifecycle keys under `[hooks]` in its `config.toml`, each holding
/// one command or an ordered list of them. jcode runs these as detached
/// observers and describes the event in `JCODE_HOOK_*` environment variables
/// rather than on stdin — see [`hook_payload`].
pub(super) const JCODE_HOOK_EVENTS: &[(&str, &str)] = &[
    ("turn_start", "prompt-submit"),
    ("turn_end", "stop"),
    ("session_start", "session-start"),
    ("session_end", "session-end"),
    ("post_tool", "tool-complete"),
];

/// Kimi Code's hooks live as `[[hooks]]` entries in its main `config.toml` —
/// the same file that holds the user's providers and models — so they go
/// through the TOML merge strategy rather than a JSON map or an owned file.
/// Like Qwen it has a first-class permission event, so it needs no
/// `Notification` hook and none of the sniffing in [`effective_event`].
///
/// `Stop` alone does not cover every way a turn ends here: Kimi's own event
/// reference says `Stop` "does not fire on interrupts, so this event fires
/// instead" of `Interrupt`, and a turn that dies on an error reports
/// `StopFailure`. Without those two an <kbd>Esc</kbd> or a failed turn would
/// leave the pane on "working" forever and `anterm wait` would only ever time
/// out, so both report the same end-of-turn as `Stop` does. All three are
/// observation-only events, and a doubled `stop` is idempotent.
pub(super) const KIMI_HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt-submit"),
    ("PermissionRequest", "permission-request"),
    ("PostToolUse", "tool-complete"),
    ("Stop", "stop"),
    ("Interrupt", "stop"),
    ("StopFailure", "stop"),
    ("SessionEnd", "session-end"),
];

const GROK_HOOK_TIMEOUT_SECS: u32 = 10;

pub(super) const GROK_HOOK_EVENTS: &[(&str, &str, Option<&str>)] = &[
    ("SessionStart", "session-start", None),
    ("UserPromptSubmit", "prompt-submit", None),
    ("Notification", "notification", Some("elicitation_dialog")),
    ("PostToolUse", "tool-complete", None),
    ("Stop", "stop", None),
    ("SessionEnd", "session-end", None),
];

pub(super) const QODER_HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt-submit"),
    ("PermissionRequest", "permission-request"),
    // An authorized MCP tool can still pause for user input mid-call.
    ("Elicitation", "question-asked"),
    ("PostToolUse", "tool-complete"),
    ("Stop", "stop"),
    ("StopFailure", "stop"),
    ("SessionEnd", "session-end"),
];

/// CodeBuddy's hooks are Claude Code's, file layout and event names alike, but
/// like Qoder it has a first-class `PermissionRequest`, so it takes Qoder's
/// table rather than Claude's `Notification` sniffing. A turn that dies on an
/// API error reports `StopFailure` instead of `Stop`; without it the pane
/// would stay on working.
pub(super) const CODEBUDDY_HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt-submit"),
    ("PermissionRequest", "permission-request"),
    // An authorized MCP tool can still pause for user input mid-call.
    ("Elicitation", "question-asked"),
    ("PostToolUse", "tool-complete"),
    ("Stop", "stop"),
    ("StopFailure", "stop"),
    ("SessionEnd", "session-end"),
];

/// Crush currently fires exactly one hook, `PreToolUse`, before every
/// top-level tool call and before its permission check. Its payload still
/// carries `session_id` and `cwd`, which is what resume needs.
///
/// It maps to `tool-complete`, not `prompt-submit`: with no `Stop` to follow,
/// a turn started here would never end, and a pane stuck on working asks
/// before every close and holds the tray and `anterm wait` on a turn long over.
/// `tool-complete` records the session and bumps the activity counter while
/// leaving the status alone. When Crush ships `UserPromptSubmit`/`Stop` and
/// friends, they slot in here.
pub(super) const CRUSH_HOOK_EVENTS: &[(&str, &str)] = &[("PreToolUse", "tool-complete")];

/// Cursor's `hooks.json` events, camel-cased and flat (see
/// [`HookAgent::flat_hook_map`]). The payloads name the session
/// `conversation_id`, which [`build_hook_sequence`] reads as the session id.
///
/// The interactive `cursor-agent` TUI — what runs in a anterm pane — fires the
/// editor's turn hooks: `beforeSubmitPrompt` opens a turn and `stop` closes
/// it, with a status of `completed`, `aborted` or `error`. `postToolUse` is
/// plain tool activity. Print mode (`cursor-agent -p`) sends neither
/// `beforeSubmitPrompt` nor `stop` (a known gap per Cursor staff, 2026-08),
/// so a `-p` run records its session but shows no status.
///
/// None of these are Cursor's permission hooks, which would block the agent
/// on the empty stdout `anterm agent-hook` prints.
pub(super) const CURSOR_HOOK_EVENTS: &[(&str, &str)] = &[
    ("sessionStart", "session-start"),
    ("beforeSubmitPrompt", "prompt-submit"),
    ("postToolUse", "tool-complete"),
    ("stop", "stop"),
    ("sessionEnd", "session-end"),
];

pub(super) fn hook_map_state(
    target: &HookTarget,
    path: &Path,
    agent: HookAgent,
    events: &[(&str, &str)],
) -> HooksState {
    let Ok(text) = target.read(path) else {
        return HooksState::NotInstalled;
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&text) else {
        return HooksState::NotInstalled;
    };
    let marker = agent.marker();
    let (mut any, mut complete) = (false, true);
    // A file without the version Cursor demands is ignored by it, hooks and
    // all. Only a missing one counts: install never overwrites the user's.
    if agent.hook_map_version().is_some() && root.get("version").is_none() {
        complete = false;
    }
    for (hook_event, anterm_event) in events {
        let ours = root
            .get("hooks")
            .and_then(|h| h.get(hook_event))
            .and_then(|e| e.as_array())
            .and_then(|list| list.iter().find_map(|m| marker_hook(m, &marker)));
        match ours {
            Some(hook) => {
                any = true;
                let command = hook.get("command").and_then(|c| c.as_str());
                let name = hook.get("name").and_then(|n| n.as_str());
                if command != Some(target.hook_command(agent, anterm_event).as_str())
                    || name != agent.hook_entry_name()
                {
                    complete = false;
                }
            }
            None => complete = false,
        }
    }
    match (any, complete) {
        (false, _) => HooksState::NotInstalled,
        (true, true) => HooksState::Installed,
        (true, false) => HooksState::Outdated,
    }
}

pub(super) fn hook_map_install(
    target: &HookTarget,
    path: &Path,
    agent: HookAgent,
    events: &[(&str, &str)],
) -> anyhow::Result<()> {
    let mut root: serde_json::Value = match target.read(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            anyhow::anyhow!(
                "{} is not valid JSON ({e}); not touching it",
                path.display()
            )
        })?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(anyhow::anyhow!("read {}: {e}", path.display())),
    };
    if !root.is_object() {
        return Err(anyhow::anyhow!(
            "{} is not a JSON object; not touching it",
            path.display()
        ));
    }

    let Some(root_obj) = root.as_object_mut() else {
        return Err(anyhow::anyhow!(
            "{} root JSON value is not an object; not touching it",
            path.display()
        ));
    };

    if let Some(version) = agent.hook_map_version() {
        root_obj
            .entry("version")
            .or_insert_with(|| serde_json::json!(version));
    }

    let hooks = root_obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return Err(anyhow::anyhow!(
            "\"hooks\" in {} is not an object; not touching it",
            path.display()
        ));
    };

    let marker = agent.marker();
    for (hook_event, anterm_event) in events {
        let command = target.hook_command(agent, anterm_event);
        let entries = hooks_obj
            .entry(*hook_event)
            .or_insert_with(|| serde_json::json!([]));
        let Some(list) = entries.as_array_mut() else {
            continue;
        };
        list.retain(|matcher| marker_command(matcher, &marker).is_none());
        if agent.flat_hook_map() {
            list.push(serde_json::json!({ "command": command }));
        } else {
            let mut hook = serde_json::json!({ "type": "command", "command": command });
            if let Some(name) = agent.hook_entry_name() {
                hook["name"] = serde_json::Value::String(name.to_string());
            }
            list.push(serde_json::json!({ "hooks": [hook] }));
        }
    }

    target.write(path, serde_json::to_string_pretty(&root)?.as_bytes())
}

pub(super) fn hook_map_uninstall(
    target: &HookTarget,
    path: &Path,
    agent: HookAgent,
) -> anyhow::Result<HookOutcome> {
    let text = match target.read(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(HookOutcome::NothingInstalled);
        }
        Err(e) => return Err(anyhow::anyhow!("read {}: {e}", path.display())),
    };
    let mut root: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        anyhow::anyhow!(
            "{} is not valid JSON ({e}); not touching it",
            path.display()
        )
    })?;

    let marker = agent.marker();
    let mut removed = 0;
    if let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for entries in hooks.values_mut() {
            if let Some(list) = entries.as_array_mut() {
                let before = list.len();
                list.retain(|matcher| marker_command(matcher, &marker).is_none());
                removed += before - list.len();
            }
        }
        hooks.retain(|_, entries| entries.as_array().is_none_or(|list| !list.is_empty()));
    }
    if removed == 0 {
        return Ok(HookOutcome::NoAntermHooks);
    }
    target.write(path, serde_json::to_string_pretty(&root)?.as_bytes())?;
    Ok(HookOutcome::Removed)
}

/// The anterm command an entry in a hook map carries, if it is one of ours.
///
/// Two shapes reach here. Claude and its imitators nest it at
/// `entry.hooks[].command`; Crush lifts `command` to the entry itself, the
/// same level as its `matcher`. Both are one list under `hooks.<Event>`, so
/// the state reader, the installer and the uninstaller all stay shared.
pub(super) fn marker_command<'a>(entry: &'a serde_json::Value, marker: &str) -> Option<&'a str> {
    marker_hook(entry, marker)?.get("command")?.as_str()
}

/// The object that holds anterm's command in an entry of a hook map — the
/// entry itself when it is flat, else the hook nested in it — so the state
/// reader can check the fields beside the command too.
fn marker_hook<'a>(entry: &'a serde_json::Value, marker: &str) -> Option<&'a serde_json::Value> {
    let ours = |h: &serde_json::Value| {
        h.get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|c| c.contains(marker))
    };
    if ours(entry) {
        return Some(entry);
    }
    entry
        .get("hooks")
        .and_then(|h| h.as_array())?
        .iter()
        .find(|h| ours(h))
}

pub(super) const MUSE_PLUGIN_ID: &str = "anterm-presence";
pub(super) const MUSE_HOOK_EVENTS: &[(&str, &str, &str)] = &[
    ("session-start", "SessionStart", "session-start"),
    ("prompt-submit", "UserPromptSubmit", "prompt-submit"),
    (
        "permission-request",
        "PermissionRequest",
        "permission-request",
    ),
    ("tool-complete", "PostToolUse", "tool-complete"),
    ("stop", "Stop", "stop"),
    ("stop-failure", "StopFailure", "stop"),
    ("interrupt", "Interrupt", "stop"),
    ("session-end", "SessionEnd", "session-end"),
];

pub(super) fn muse_plugin_manifest(target: &HookTarget) -> serde_json::Value {
    let hooks: Vec<_> = MUSE_HOOK_EVENTS
        .iter()
        .map(|(id, event, anterm_event)| {
            serde_json::json!({
                "id": id, "event": event,
                "command": [target.exe.to_string_lossy(), "agent-hook", "muse", anterm_event],
                "timeoutMs": 5000, "async": *event == "Interrupt",
            })
        })
        .collect();
    serde_json::json!({
        "schemaVersion": 1, "name": MUSE_PLUGIN_ID, "displayName": "anterm presence",
        "version": "1.0.0", "description": "anterm agent-hook muse presence",
        "compat": {"source": "native", "manifestDir": ".muse-plugin"},
        "capabilities": {"hooks": hooks},
    })
}

/// Muse's native CLI owns its cache and approval records; do not write those directly.
fn muse_cli(args: &[String]) -> anyhow::Result<serde_json::Value> {
    let mut cmd = std::process::Command::new("muse");
    cmd.env("MUSE_NO_AUTO_UPDATE", "1")
        .args(["plugins"])
        .args(args)
        .arg("--json");
    // Settings and `anterm doctor` both wait on this; a Muse that hangs (a
    // login prompt, a stuck update check) must not take either down with it.
    let out = crate::proc::output_within(
        crate::proc::hide_console(&mut cmd),
        std::time::Duration::from_secs(30),
    )?;
    if !out.status.success() {
        anyhow::bail!(
            "muse plugins {} failed ({}): {}",
            args.first().map(String::as_str).unwrap_or(""),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

pub(super) fn muse_inspected_state(target: &HookTarget, inspected: &serde_json::Value) -> HooksState {
    if inspected["plugin"]["id"] != MUSE_PLUGIN_ID {
        return HooksState::NotInstalled;
    }
    let expected = muse_plugin_manifest(target);
    let complete = inspected["active"] == true
        && inspected["valid"] == true
        && expected["capabilities"]["hooks"]
            .as_array()
            .is_some_and(|expected_hooks| {
                expected_hooks.iter().all(|hook| {
                    let declared = inspected["plugin"]["capabilities"]["hooks"]
                        .as_array()
                        .is_some_and(|hooks| {
                        hooks.iter().any(|installed| {
                            installed["id"] == hook["id"]
                                && installed["event"] == hook["event"]
                                && installed["command"] == hook["command"]
                                && installed["async"] == hook["async"]
                        })
                    });
                let approved =
                    inspected["runtime_capabilities"]
                        .as_array()
                        .is_some_and(|entries| {
                            entries.iter().any(|entry| {
                                entry["candidate"]["kind"] == "hook"
                                    && entry["candidate"]["capability_id"] == hook["id"]
                                    && entry["status"] == "trusted_enabled"
                            })
                        });
                    declared && approved
                })
            });
    if complete {
        HooksState::Installed
    } else {
        HooksState::Outdated
    }
}

pub(super) fn muse_hooks_state(target: &HookTarget) -> HooksState {
    // A source bundle alone is not an installed plugin. Host has no remote exec API.
    if !target.is_local() {
        return HooksState::NotInstalled;
    }
    muse_cli(&["inspect".into(), MUSE_PLUGIN_ID.into()])
        .map(|value| muse_inspected_state(target, &value))
        .unwrap_or(HooksState::NotInstalled)
}

fn muse_install_commands(bundle: &Path) -> String {
    format!(
        "muse plugins install \"{}\" --scope user && muse plugins approve {MUSE_PLUGIN_ID} && muse plugins enable {MUSE_PLUGIN_ID}",
        bundle.display()
    )
}

pub(super) fn muse_install_with(
    target: &HookTarget,
    path: &Path,
    mut run: impl FnMut(&[String]) -> anyhow::Result<serde_json::Value>,
) -> anyhow::Result<HookOutcome> {
    let bundle = path
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow::anyhow!("invalid Muse bundle path"))?;
    let content = serde_json::to_string_pretty(&muse_plugin_manifest(target))?;
    owned_file_install(target, path, &content, &HookAgent::Muse.marker())?;
    if !target.is_local() {
        return Ok(HookOutcome::MuseInstallManually(muse_install_commands(
            bundle,
        )));
    }
    run(&[
        "install".into(),
        bundle.to_string_lossy().into_owned(),
        "--scope".into(),
        "user".into(),
    ])?;
    run(&["approve".into(), MUSE_PLUGIN_ID.into()])?;
    run(&["enable".into(), MUSE_PLUGIN_ID.into()])?;
    let inspected = run(&["inspect".into(), MUSE_PLUGIN_ID.into()])?;
    if muse_inspected_state(target, &inspected) != HooksState::Installed {
        anyhow::bail!("Muse plugin was not activated; inspect {MUSE_PLUGIN_ID} before retrying");
    }
    Ok(HookOutcome::Installed)
}

pub(super) fn muse_hooks_install(target: &HookTarget, path: &Path) -> anyhow::Result<HookOutcome> {
    muse_install_with(target, path, muse_cli)
}

pub(super) fn muse_hooks_uninstall(target: &HookTarget, path: &Path) -> anyhow::Result<HookOutcome> {
    if !target.is_local() {
        return Ok(HookOutcome::MuseInstallManually(format!(
            "muse plugins remove {MUSE_PLUGIN_ID}"
        )));
    }
    if muse_hooks_state(target) != HooksState::NotInstalled {
        muse_cli(&["remove".into(), MUSE_PLUGIN_ID.into()])?;
    }
    owned_file_uninstall(target, path, &HookAgent::Muse.marker())
}

/// jcode accepts either one command or an ordered list (HookCommands).
pub(super) fn jcode_commands(item: &toml_edit::Item) -> anyhow::Result<Vec<String>> {
    if let Some(command) = item.as_str() {
        return Ok(vec![command.to_owned()]);
    }
    if let Some(array) = item.as_array() {
        return array
            .iter()
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    anyhow::anyhow!("jcode hook commands must be strings; not touching it")
                })
            })
            .collect();
    }
    anyhow::bail!("jcode hook must be a string or an array of strings; not touching it")
}

fn set_jcode_commands(table: &mut toml_edit::Table, key: &str, commands: &[String]) {
    match commands {
        [] => {
            table.remove(key);
        }
        [command] => {
            table[key] = toml_edit::value(command.clone());
        }
        _ => {
            let mut array = toml_edit::Array::new();
            for command in commands {
                array.push(command.as_str());
            }
            table[key] = toml_edit::value(array);
        }
    }
}

pub(super) fn jcode_hooks_state(target: &HookTarget, path: &Path) -> HooksState {
    let Ok(text) = target.read(path) else {
        return HooksState::NotInstalled;
    };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return HooksState::NotInstalled;
    };
    let Some(table) = doc.get("hooks").and_then(|item| item.as_table()) else {
        return HooksState::NotInstalled;
    };
    let marker = HookAgent::Jcode.marker();
    let commands: Vec<_> = JCODE_HOOK_EVENTS
        .iter()
        .map(|(key, _)| {
            table
                .get(*key)
                .map(jcode_commands)
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_default()
        })
        .collect();
    let ours = commands
        .iter()
        .flatten()
        .any(|command| command.contains(&marker));
    let complete = JCODE_HOOK_EVENTS
        .iter()
        .zip(&commands)
        .all(|((_, event), list)| {
            let expected = target.hook_command(HookAgent::Jcode, event);
            let marked: Vec<_> = list
                .iter()
                .filter(|command| command.contains(&marker))
                .collect();
            marked.len() == 1 && marked[0] == &expected
        });
    if complete {
        HooksState::Installed
    } else if ours {
        HooksState::Outdated
    } else {
        HooksState::NotInstalled
    }
}

pub(super) fn jcode_hooks_install(target: &HookTarget, path: &Path) -> anyhow::Result<()> {
    let mut doc: toml_edit::DocumentMut = match target.read(path) {
        Ok(text) => text.parse().map_err(|e| {
            anyhow::anyhow!(
                "{} is not valid TOML ({e}); not touching it",
                path.display()
            )
        })?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => toml_edit::DocumentMut::new(),
        Err(e) => return Err(anyhow::anyhow!("read {}: {e}", path.display())),
    };
    let hooks = doc.entry("hooks").or_insert(toml_edit::table());
    let table = hooks.as_table_mut().ok_or_else(|| {
        anyhow::anyhow!("{} hooks is not a table; not touching it", path.display())
    })?;
    let marker = HookAgent::Jcode.marker();
    for (key, event) in JCODE_HOOK_EVENTS {
        let mut commands = table
            .get(*key)
            .map(jcode_commands)
            .transpose()?
            .unwrap_or_default();
        commands.retain(|command| !command.contains(&marker));
        commands.push(target.hook_command(HookAgent::Jcode, event));
        set_jcode_commands(table, key, &commands);
    }
    target.write(path, doc.to_string().as_bytes())
}

pub(super) fn jcode_hooks_uninstall(target: &HookTarget, path: &Path) -> anyhow::Result<HookOutcome> {
    let text = match target.read(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(HookOutcome::NothingInstalled),
        Err(e) => return Err(anyhow::anyhow!("read {}: {e}", path.display())),
    };
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| {
        anyhow::anyhow!(
            "{} is not valid TOML ({e}); not touching it",
            path.display()
        )
    })?;
    let Some(table) = doc.get_mut("hooks").and_then(|v| v.as_table_mut()) else {
        return Ok(HookOutcome::NoAntermHooks);
    };
    let marker = HookAgent::Jcode.marker();
    let mut removed = false;
    for (key, _) in JCODE_HOOK_EVENTS {
        let Some(item) = table.get(*key) else {
            continue;
        };
        let mut commands = jcode_commands(item)?;
        let before = commands.len();
        commands.retain(|command| !command.contains(&marker));
        if commands.len() != before {
            set_jcode_commands(table, key, &commands);
            removed = true;
        }
    }
    if !removed {
        return Ok(HookOutcome::NoAntermHooks);
    }
    target.write(path, doc.to_string().as_bytes())?;
    Ok(HookOutcome::Removed)
}

pub(super) fn toml_hooks_state(
    target: &HookTarget,
    path: &Path,
    agent: HookAgent,
    events: &[(&str, &str)],
) -> HooksState {
    let Ok(text) = target.read(path) else {
        return HooksState::NotInstalled;
    };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return HooksState::NotInstalled;
    };
    let marker = agent.marker();
    let marked: Vec<&toml_edit::Table> = doc
        .get("hooks")
        .and_then(|h| h.as_array_of_tables())
        .into_iter()
        .flatten()
        .filter(|entry| toml_command_is_marked(entry, &marker))
        .collect();
    if marked.is_empty() {
        return HooksState::NotInstalled;
    }
    // Every marked entry counts towards the total, `event` or no `event`: a
    // hand-edit that drops the key leaves an entry that is ours and is broken,
    // which is exactly what Outdated means. Reporting NotInstalled instead
    // would hide it from `refresh_hooks`, which only ever revisits Outdated.
    let complete = marked.len() == events.len()
        && events.iter().all(|(hook_event, anterm_event)| {
            let command = target.hook_command(agent, anterm_event);
            marked.iter().any(|entry| {
                entry.get("event").and_then(|e| e.as_str()) == Some(*hook_event)
                    && entry.get("command").and_then(|c| c.as_str()) == Some(command.as_str())
            })
        });
    if complete {
        HooksState::Installed
    } else {
        HooksState::Outdated
    }
}

pub(super) fn toml_hooks_install(
    target: &HookTarget,
    path: &Path,
    agent: HookAgent,
    events: &[(&str, &str)],
) -> anyhow::Result<()> {
    let mut doc: toml_edit::DocumentMut = match target.read(path) {
        Ok(text) => text.parse().map_err(|e| {
            anyhow::anyhow!(
                "{} is not valid TOML ({e}); not touching it",
                path.display()
            )
        })?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => toml_edit::DocumentMut::new(),
        Err(e) => return Err(anyhow::anyhow!("read {}: {e}", path.display())),
    };

    // `hooks = []` and no `hooks` key at all say the same thing, but toml_edit
    // keeps an empty inline array and an array of tables apart. Promote the
    // one to the other rather than refusing over a difference that carries no
    // configuration and that the user cannot see.
    if doc
        .get("hooks")
        .and_then(|h| h.as_array())
        .is_some_and(|a| a.is_empty())
    {
        doc.remove("hooks");
    }

    let hooks = doc.entry("hooks").or_insert(toml_edit::Item::ArrayOfTables(
        toml_edit::ArrayOfTables::new(),
    ));
    let Some(list) = hooks.as_array_of_tables_mut() else {
        return Err(anyhow::anyhow!(
            "\"hooks\" in {} is not an array of tables; not touching it",
            path.display()
        ));
    };

    let marker = agent.marker();
    list.retain(|entry| !toml_command_is_marked(entry, &marker));
    for (hook_event, anterm_event) in events {
        let mut entry = toml_edit::Table::new();
        entry["event"] = toml_edit::value(*hook_event);
        entry["command"] = toml_edit::value(target.hook_command(agent, anterm_event));
        list.push(entry);
    }

    target.write(path, doc.to_string().as_bytes())
}

pub(super) fn toml_hooks_uninstall(
    target: &HookTarget,
    path: &Path,
    agent: HookAgent,
) -> anyhow::Result<HookOutcome> {
    let text = match target.read(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(HookOutcome::NothingInstalled);
        }
        Err(e) => return Err(anyhow::anyhow!("read {}: {e}", path.display())),
    };
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| {
        anyhow::anyhow!(
            "{} is not valid TOML ({e}); not touching it",
            path.display()
        )
    })?;

    let marker = agent.marker();
    let mut removed = 0;
    if let Some(list) = doc
        .get_mut("hooks")
        .and_then(|h| h.as_array_of_tables_mut())
    {
        let before = list.len();
        list.retain(|entry| !toml_command_is_marked(entry, &marker));
        removed = before - list.len();
        if list.is_empty() {
            doc.remove("hooks");
        }
    }
    if removed == 0 {
        return Ok(HookOutcome::NoAntermHooks);
    }
    target.write(path, doc.to_string().as_bytes())?;
    Ok(HookOutcome::Removed)
}

pub(super) fn toml_command_is_marked(entry: &toml_edit::Table, marker: &str) -> bool {
    entry
        .get("command")
        .and_then(|c| c.as_str())
        .is_some_and(|c| c.contains(marker))
}

pub(super) fn enable_codex_hooks_feature() -> Result<(), String> {
    let candidates = [
        PathBuf::from("/opt/homebrew/bin/codex"),
        PathBuf::from("/usr/local/bin/codex"),
    ]
    .into_iter()
    .chain(home_dir().map(|h| h.join(".local/bin/codex")))
    .find(|p| p.exists());
    let program = candidates.unwrap_or_else(|| PathBuf::from("codex"));
    let mut cmd = std::process::Command::new(&program);
    cmd.args(["features", "enable", "hooks"]);
    match crate::proc::hide_console(&mut cmd).output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(format!(
            "codex exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => Err(format!("{}: {e}", program.display())),
    }
}

/// The key anterm's entry lives under in Antigravity's `hooks.json`.
///
/// Antigravity names every hook set at the root of the file —
/// `{"<name>": {"<Event>": [...]}}` — rather than grouping handlers under a
/// shared `hooks` object the way Claude does, so anterm owns one named set and
/// never has to pick its commands out of the user's lists.
pub(super) const ANTIGRAVITY_HOOK_SET: &str = "anterm";

/// Antigravity CLI's lifecycle events, mapped onto anterm's.
///
/// It has no session-start or session-end event. `PreInvocation` fires before
/// every model call of a turn, so `effective_event` keeps only the first one
/// as the turn start; `Stop` fires when the execution loop ends. The
/// `PreToolUse` gate is left alone: it must answer with a `decision`, and an
/// empty reply from a status hook has no business deciding anything.
pub(super) fn antigravity_hook_set(target: &HookTarget) -> serde_json::Value {
    let handler = |event: &str| {
        serde_json::json!({
            "type": "command",
            "command": target.hook_command(HookAgent::Antigravity, event),
            "timeout": 5,
        })
    };
    serde_json::json!({
        "PreInvocation": [handler("prompt-submit")],
        "PostToolUse": [{ "matcher": "*", "hooks": [handler("tool-complete")] }],
        "Stop": [handler("stop")],
    })
}

fn read_json_object(target: &HookTarget, path: &Path) -> anyhow::Result<Option<serde_json::Value>> {
    let text = match target.read(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow::anyhow!("read {}: {e}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Some(serde_json::json!({})));
    }
    let root: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        anyhow::anyhow!(
            "{} is not valid JSON ({e}); not touching it",
            path.display()
        )
    })?;
    if !root.is_object() {
        return Err(anyhow::anyhow!(
            "{} is not a JSON object; not touching it",
            path.display()
        ));
    }
    Ok(Some(root))
}

pub(super) fn named_hook_set_state(
    target: &HookTarget,
    path: &Path,
    expected: &serde_json::Value,
) -> HooksState {
    match read_json_object(target, path) {
        Ok(Some(root)) => match root.get(ANTIGRAVITY_HOOK_SET) {
            None => HooksState::NotInstalled,
            Some(set) if set == expected => HooksState::Installed,
            Some(_) => HooksState::Outdated,
        },
        _ => HooksState::NotInstalled,
    }
}

pub(super) fn named_hook_set_install(
    target: &HookTarget,
    path: &Path,
    set: serde_json::Value,
) -> anyhow::Result<()> {
    let mut root = read_json_object(target, path)?.unwrap_or_else(|| serde_json::json!({}));
    let Some(obj) = root.as_object_mut() else {
        return Err(anyhow::anyhow!(
            "{} root JSON value is not an object; not touching it",
            path.display()
        ));
    };
    obj.insert(ANTIGRAVITY_HOOK_SET.to_string(), set);
    target.write(path, serde_json::to_string_pretty(&root)?.as_bytes())
}

pub(super) fn named_hook_set_uninstall(target: &HookTarget, path: &Path) -> anyhow::Result<HookOutcome> {
    let Some(mut root) = read_json_object(target, path)? else {
        return Ok(HookOutcome::NothingInstalled);
    };
    let Some(obj) = root.as_object_mut() else {
        return Ok(HookOutcome::NoAntermHooks);
    };
    if obj.remove(ANTIGRAVITY_HOOK_SET).is_none() {
        return Ok(HookOutcome::NoAntermHooks);
    }
    target.write(path, serde_json::to_string_pretty(&root)?.as_bytes())?;
    Ok(HookOutcome::Removed)
}

pub(super) fn owned_file_content(target: &HookTarget, agent: HookAgent) -> Option<String> {
    match agent {
        HookAgent::Copilot => copilot_hooks_json(target),
        HookAgent::OpenCode => opencode_plugin_js(target),
        HookAgent::Pi | HookAgent::OhMyPi | HookAgent::PrimeAgent => pi_extension_ts(target, agent),
        HookAgent::Grok => grok_hooks_json(target),
        HookAgent::Goose => goose_hooks_json(target),
        HookAgent::Claude
        | HookAgent::Codex
        | HookAgent::TraeCode
        | HookAgent::Gemini
        | HookAgent::Droid
        | HookAgent::Qwen
        | HookAgent::QoderCLI
        | HookAgent::QoderCLICn
        | HookAgent::Crush
        | HookAgent::CodeBuddy
        | HookAgent::Cursor
        | HookAgent::Antigravity
        | HookAgent::Kimi
        | HookAgent::Empryo
        | HookAgent::Jcode
        | HookAgent::Muse => None,
    }
}

pub(super) fn owned_file_state(target: &HookTarget, path: &Path, expected: &str, marker: &str) -> HooksState {
    let Ok(contents) = target.read(path) else {
        return HooksState::NotInstalled;
    };
    if contents == expected {
        HooksState::Installed
    } else if contents.contains(marker) {
        HooksState::Outdated
    } else {
        HooksState::NotInstalled
    }
}

pub(super) fn owned_file_install(
    target: &HookTarget,
    path: &Path,
    content: &str,
    marker: &str,
) -> anyhow::Result<()> {
    if let Ok(existing) = target.read(path)
        && !existing.contains(marker)
    {
        return Err(anyhow::anyhow!(
            "{} exists but wasn't written by anterm; not touching it",
            path.display()
        ));
    }
    target.write(path, content.as_bytes())
}

pub(super) fn owned_file_uninstall(
    target: &HookTarget,
    path: &Path,
    marker: &str,
) -> anyhow::Result<HookOutcome> {
    let contents = match target.read(path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(HookOutcome::NothingInstalled);
        }
        Err(e) => return Err(anyhow::anyhow!("read {}: {e}", path.display())),
    };
    if !contents.contains(marker) {
        return Err(anyhow::anyhow!(
            "{} wasn't written by anterm; not touching it",
            path.display()
        ));
    }
    target.host.remove(path, false)?;
    // Take the directories anterm generated with it, innermost first, stopping at
    // the one named after anterm. `remove` is not recursive, so a directory still
    // holding someone else's file simply survives the attempt. Goose nests one
    // level deeper than the rest (`.../anterm/hooks/hooks.json`), which is why
    // this walks rather than checking a single parent.
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d.file_name().is_some_and(|n| n == "anterm") {
            let _ = target.host.remove(d, false);
            break;
        }
        if !d
            .parent()
            .and_then(|p| p.file_name())
            .is_some_and(|n| n == "anterm")
        {
            break;
        }
        let _ = target.host.remove(d, false);
        dir = d.parent();
    }
    Ok(HookOutcome::Removed)
}

pub(super) fn copilot_hooks_json(target: &HookTarget) -> Option<String> {
    let hook = |event: &str, timeout: u32| {
        serde_json::json!([{
            "type": "command",
            "bash": target.hook_command(HookAgent::Copilot, event),
            "timeoutSec": timeout,
        }])
    };
    let root = serde_json::json!({
        "version": 1,
        "hooks": {
            "sessionStart": hook("session-start", 5),
            "userPromptSubmitted": hook("prompt-submit", 5),
            "agentStop": hook("stop", 10),
            "sessionEnd": hook("session-end", 5),
            "notification": hook("notification", 5),
        }
    });
    serde_json::to_string_pretty(&root).ok()
}

pub(super) fn grok_hooks_json(target: &HookTarget) -> Option<String> {
    let mut hooks = serde_json::Map::new();
    for (event, sentinel, matcher) in GROK_HOOK_EVENTS {
        let mut group = serde_json::json!({
            "hooks": [{
                "type": "command",
                "command": target.hook_command(HookAgent::Grok, sentinel),
                "timeout": GROK_HOOK_TIMEOUT_SECS,
            }]
        });
        if let Some(matcher) = matcher {
            group["matcher"] = serde_json::Value::String((*matcher).to_string());
        }
        hooks.insert((*event).to_string(), serde_json::json!([group]));
    }
    serde_json::to_string_pretty(&serde_json::json!({ "hooks": hooks })).ok()
}

/// Goose has no permission hook — `PreToolUse` fires on every call, approved or
/// not, so there is nothing here that could report a blocked turn. The four
/// events it does have still carry the pane from idle to working to done.
pub(super) const GOOSE_HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt-submit"),
    ("PostToolUse", "tool-complete"),
    ("Stop", "stop"),
    ("SessionEnd", "session-end"),
];

pub(super) fn goose_hooks_json(target: &HookTarget) -> Option<String> {
    let mut hooks = serde_json::Map::new();
    for (event, sentinel) in GOOSE_HOOK_EVENTS {
        hooks.insert(
            (*event).to_string(),
            serde_json::json!([{
                "hooks": [{
                    "type": "command",
                    "command": target.hook_command(HookAgent::Goose, sentinel),
                }]
            }]),
        );
    }
    serde_json::to_string_pretty(&serde_json::json!({ "hooks": hooks })).ok()
}

/// The OpenCode plugin bridge — one file that both plugin loaders accept.
///
/// OpenCode 1.x (since 1.3.4) takes a default export `{ id, server }`, and
/// OpenCode 2.x takes `{ id, setup }` (or `{ id, effect }`) and rejects
/// anything else with "Plugin must export a default definition with an id
/// and an effect or setup function" (#999). Each loader ignores the key the
/// other one reads, so a default export carrying all three loads on both.
/// The named `AntermPresence` export is the same 1.x factory, for 1.x builds
/// older than 1.3.4 that call every export; newer loaders never look at it.
///
/// The two majors differ in more than the export: 2.x hands `setup` no Bun
/// `$` and no `event` hook — events come from `ctx.event.subscribe()`, with
/// their fields under `data` rather than `properties`, and permission prompts
/// arrive as `permission.asked`. Both feed one tracker, so the session-id,
/// subagent and busy/idle rules are written once.
///
/// The emitter is spawned directly (no shell): `sh` is not there on Windows,
/// and 2.x may run on Node rather than Bun.
pub(super) fn opencode_plugin_js(target: &HookTarget) -> Option<String> {
    let exe = serde_json::to_string(&target.exe.display().to_string()).ok()?;
    Some(
        r#"// anterm agent-hook opencode bridge — generated by anterm, do not edit.
// Bridges OpenCode plugin events onto `anterm agent-hook opencode <event>`,
// which is inert outside anterm (gated on the ANTERM env var).
// Loads on OpenCode 1.x (default `{ id, server }`) and 2.x (`{ id, setup }`).
import { spawn } from "node:child_process"

const EXE = __EXE__

// Spawned directly, not through a shell: there is no `sh` on Windows.
function run(event, sessionId) {
  return new Promise((resolve) => {
    try {
      const payload = sessionId ? JSON.stringify({ session_id: sessionId }) : ""
      const child = spawn(EXE, ["agent-hook", "opencode", event], {
        stdio: [payload ? "pipe" : "ignore", "ignore", "ignore"],
        windowsHide: true,
      })
      child.on("error", () => resolve())
      child.on("close", () => resolve())
      if (payload) {
        child.stdin.on("error", () => {})
        child.stdin.end(payload)
      }
    } catch {
      resolve()
    }
  })
}

// One pane's view of the event stream, shared by both plugin APIs.
function presence() {
  let sessionId = ""
  let announced = ""
  // A subagent runs in a child session on the *same* event stream, and its
  // status events are indistinguishable from the pane's own — only session
  // creation names a parent. Remember the children, so a task tool cannot
  // hand the pane the wrong session to resume, nor call the pane done when
  // only the subagent is.
  const children = new Set()
  // Every event carries the session id, so a pane that restarts can resume
  // the same session with `opencode --session`.
  const emit = (event) => run(event, sessionId)
  // The session id is only known once opencode creates the session; the
  // session-start report rides on the first event that names one, so a
  // restored pane can reattach to it.
  const capture = async (id) => {
    if (id && !children.has(id)) sessionId = id
    if (sessionId && sessionId !== announced) {
      announced = sessionId
      await emit("session-start")
    }
  }
  const ACTION = {
    "session.status.busy": "prompt-submit",
    "session.status.idle": "stop",
    "session.idle": "stop",
    "permission.asked": "permission-request",
    "permission.replied": "prompt-submit",
  }
  // `fields` is 1.x's `event.properties` or 2.x's `event.data`.
  const observe = async (type, fields) => {
    const info = fields.info
    // 1.x: `session.created`/`session.updated` carry the whole session.
    if (info?.id && info.parentID) children.add(info.id)
    // 2.x: `session.created` names the session and its parent directly.
    if (type === "session.created" && fields.sessionID && fields.parentID) children.add(fields.sessionID)
    if (fields.sessionID && children.has(fields.sessionID)) return
    await capture(fields.sessionID)
    const key = type === "session.status" ? `session.status.${fields.status?.type}` : type
    const action = ACTION[key]
    if (action) await emit(action)
  }
  return { emit, capture, observe }
}

// OpenCode 1.x: a factory returning hooks, run in the TUI's own process.
const server = async () => {
  if (!process.env["ANTERM"]) return {}
  const pane = presence()
  return {
    dispose: async () => {
      await pane.emit("session-end")
    },
    "tool.execute.before": async (input) => {
      await pane.capture(input?.sessionID)
      await pane.emit("prompt-submit")
    },
    "permission.ask": async (input) => {
      await pane.capture(input?.sessionID)
      await pane.emit("permission-request")
    },
    event: async ({ event }) => {
      await pane.observe(event.type, event.properties ?? {})
    },
  }
}

// OpenCode 2.x: `setup` subscribes to the event stream and returns cleanup.
const setup = async (ctx) => {
  if (!process.env["ANTERM"]) return
  // By default 2.x runs plugins in one background service shared by every
  // opencode on the machine: its sessions are not this pane's, and the
  // emitter it spawns has no way back to the pane's terminal. Only a server
  // the TUI started for itself (`--standalone`, or `opencode serve` run in
  // the pane) belongs to one pane.
  if (process.argv.includes("--service")) return
  if (typeof ctx?.event?.subscribe !== "function") return
  const pane = presence()
  const abort = new AbortController()
  // Not awaited: setup has to return for the plugin to finish loading.
  ;(async () => {
    try {
      for await (const event of ctx.event.subscribe({ signal: abort.signal })) {
        if (event?.type) await pane.observe(event.type, event.data ?? {})
      }
    } catch {}
  })()
  return async () => {
    abort.abort()
    await pane.emit("session-end")
  }
}

export const AntermPresence = server

export default { id: "anterm", server, setup }
"#
        .replace("__EXE__", &exe),
    )
}

/// The Pi extension bridge, shared with Oh My Pi.
///
/// Oh My Pi is a fork of Pi and kept the extension contract intact — same
/// default-exported factory, same four lifecycle events, same
/// `ctx.sessionManager.getSessionId()`. Only the package it is imported from
/// and the slug the emitter is called with differ, so one template serves both
/// rather than two copies drifting apart.
pub(super) fn pi_extension_ts(target: &HookTarget, agent: HookAgent) -> Option<String> {
    let (slug, package) = match agent {
        // Pi moved from `@mariozechner/` to `@earendil-works/`. Current Pi
        // resolves both names for extensions, and the import is type-only, so
        // it is erased before an older Pi that knows only the old name runs it.
        HookAgent::Pi => ("pi", "@earendil-works/pi-coding-agent"),
        HookAgent::OhMyPi => ("omp", "@oh-my-pi/pi-coding-agent"),
        HookAgent::PrimeAgent => ("prime-agent", "@earendil-works/pi-coding-agent"),
        _ => return None,
    };
    let exe = serde_json::to_string(&target.exe.display().to_string()).ok()?;
    Some(format!(
        r#"/* anterm agent-hook {slug} bridge — generated by anterm, do not edit. */
import type {{ ExtensionAPI }} from "{package}";
import {{ spawnSync }} from "node:child_process";

const EXE = {exe};

/** The slice of Pi's handler context we read — structural, so this bridge does
 *  not depend on the context type staying exported. */
type SessionCtx = {{ sessionManager?: {{ getSessionId?(): string | undefined }} }};

function emit(event: string, ctx?: SessionCtx): void {{
  try {{
    let payload = "";
    try {{
      const id = ctx?.sessionManager?.getSessionId?.();
      if (id) payload = JSON.stringify({{ session_id: id }});
    }} catch {{}}
    const args = ["agent-hook", "{slug}", event];
    // Nothing to send → leave stdin closed rather than handing the emitter a
    // pipe it has to read to EOF.
    if (payload) {{
      spawnSync(EXE, args, {{ input: payload, stdio: ["pipe", "ignore", "ignore"] }});
    }} else {{
      spawnSync(EXE, args, {{ stdio: ["ignore", "ignore", "ignore"] }});
    }}
  }} catch {{}}
}}

export default function (pi: ExtensionAPI) {{
  if (!process.env["ANTERM"]) return;
  // Extension load = the agent is running in this pane. No context here yet,
  // so the id rides on session_start instead.
  emit("session-start");
  // What the pane showed before a UI prompt put it on "waiting", so closing
  // the prompt can put it back.
  let turn = "session-start";
  // Whether this build says when a run is over for good. agent_end also ends
  // each attempt an automatic retry, a compaction or a queued message picks
  // up again, and reading that as "done" calls the pane finished while it
  // still works. Pi's agent_settled comes once the run is really over, and a
  // build that sends it shows so the first time it does. Nothing earlier
  // tells: on() returns nothing in Pi or Oh My Pi, and a build that takes the
  // name without ever sending it must still end its turns on agent_end.
  let settles = false;
  pi.on("agent_start", (_event, ctx) => emit((turn = "prompt-submit"), ctx));
  pi.on("agent_end", (_event, ctx) => {{
    if (!settles) emit((turn = "stop"), ctx);
  }});
  pi.on("session_shutdown", (_event, ctx) => emit("session-end", ctx));
  // Last, and guarded: the three above already worked, so a Pi build that
  // rejects this event name must not take them — or the whole extension —
  // down with it.
  try {{
    pi.on("session_start", (_event, ctx) => emit("session-start", ctx));
  }} catch {{}}
  // Pi-only: Oh My Pi may not expose the UI-prompt hooks, so each one is
  // guarded on its own — a fork that rejects the event name must not take the
  // rest of the bridge down with it. Without these the pane never reports
  // "waiting for you" while a dialog is open, and the event vocabulary already
  // carries question-asked / permission-request for exactly that.
  try {{
    pi.on("ui_prompt_start", (event, ctx) => {{
      emit(event.kind === "confirm" ? "permission-request" : "question-asked", ctx);
    }});
  }} catch {{}}
  // The dialog closed: restore what it interrupted. Not a blanket
  // prompt-submit — Pi also prompts while idle (/model, a command's select),
  // and that would leave a finished pane reading "working" with no turn to
  // ever end it.
  try {{
    pi.on("ui_prompt_end", (_event, ctx) => emit(turn, ctx));
  }} catch {{}}
  try {{
    pi.on("agent_settled" as any, (_event: unknown, ctx: SessionCtx) => {{
      settles = true;
      emit((turn = "stop"), ctx);
    }});
  }} catch {{}}
  // Oh My Pi asks before running a tool through events of its own rather
  // than Pi's UI-prompt pair, and announces its retries — which follow an
  // agent_end — where Pi does not.
  try {{
    pi.on("tool_approval_requested" as any, (_event: unknown, ctx: SessionCtx) =>
      emit("permission-request", ctx),
    );
    pi.on("tool_approval_resolved" as any, (_event: unknown, ctx: SessionCtx) => emit(turn, ctx));
    pi.on("auto_retry_start" as any, (_event: unknown, ctx: SessionCtx) =>
      emit((turn = "prompt-submit"), ctx),
    );
  }} catch {{}}
}}
"#
    ))
}
