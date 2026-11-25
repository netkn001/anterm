use std::collections::HashMap;

use serde::{Deserialize, Serialize};

mod runtime;
#[cfg(test)]
mod tests;

pub use runtime::{
    AGENT_EVENT_SENTINEL, AgentEvent, AgentEventKind, AgentReadout, AgentSessionState, AgentStatus,
    parse_agent_event, parse_agent_event_body,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CLIAgent {
    Claude,
    Codex,
    Gemini,
    Aider,
    Amp,
    OpenCode,
    Copilot,
    Cursor,
    Goose,
    Droid,
    Pi,
    Auggie,
    Hermes,
    Vibe,
    Antigravity,
    Grok,
    Qwen,
    OhMyPi,
    Kimi,
    // Keep new variants at the end: daemon messages serialize this enum and
    // moving an existing discriminant would break mixed-version clients.
    TraeCode,
    QoderCLI,
    Crush,
    CodeBuddy,
    Empryo,
    PrimeAgent,
    QoderCLICn,
    Muse,
    Jcode,
}

impl CLIAgent {
    pub const ALL: [CLIAgent; 28] = [
        CLIAgent::Claude,
        CLIAgent::Codex,
        CLIAgent::TraeCode,
        CLIAgent::Gemini,
        CLIAgent::Aider,
        CLIAgent::Amp,
        CLIAgent::OpenCode,
        CLIAgent::Copilot,
        CLIAgent::Cursor,
        CLIAgent::Goose,
        CLIAgent::Droid,
        CLIAgent::Pi,
        CLIAgent::Auggie,
        CLIAgent::Hermes,
        CLIAgent::Vibe,
        CLIAgent::Antigravity,
        CLIAgent::Grok,
        CLIAgent::Qwen,
        CLIAgent::OhMyPi,
        CLIAgent::Kimi,
        CLIAgent::QoderCLI,
        CLIAgent::QoderCLICn,
        CLIAgent::Crush,
        CLIAgent::CodeBuddy,
        CLIAgent::Empryo,
        CLIAgent::PrimeAgent,
        CLIAgent::Muse,
        CLIAgent::Jcode,
    ];

    fn aliases(self) -> &'static [&'static str] {
        match self {
            CLIAgent::Claude => &["claude", "claude-code"],
            CLIAgent::Codex => &["codex", "codex-cli"],
            CLIAgent::TraeCode => &["traecli", "traex"],
            CLIAgent::Gemini => &["gemini", "gemini-cli"],
            CLIAgent::Aider => &["aider", "aider-chat"],
            CLIAgent::Amp => &["amp"],
            CLIAgent::OpenCode => &["opencode"],
            CLIAgent::Copilot => &["copilot"],
            CLIAgent::Cursor => &["cursor-agent"],
            CLIAgent::Goose => &["goose"],
            CLIAgent::Droid => &["droid"],
            CLIAgent::Pi => &["pi"],
            CLIAgent::Auggie => &["auggie"],
            CLIAgent::Hermes => &["hermes"],
            CLIAgent::Vibe => &["vibe", "vibe-acp"],
            // `agy` only. The `antigravity` binary the IDE installs is a
            // launcher shim in the shape of VS Code's `code`, not the terminal
            // agent — and the name also collides with `python3 -m antigravity`,
            // the standard way to trigger Python's own easter egg.
            CLIAgent::Antigravity => &["agy"],
            CLIAgent::Grok => &["grok"],
            CLIAgent::Qwen => &["qwen", "qwen-code"],
            // Oh My Pi is a fork of Pi, but it ships one binary of its own and
            // never installs a `pi`, so the two names stay disjoint.
            CLIAgent::OhMyPi => &["omp"],
            // Both the standalone Kimi Code CLI and the legacy open-source
            // kimi-cli install a `kimi` — same vendor, same brand, so one
            // detection covers them. Only the standalone one has hooks.
            CLIAgent::Kimi => &["kimi", "kimi-code"],
            // The npm package installs two binaries and `qoder` is the one the
            // documentation tells people to run: it dispatches to the CLI for a
            // bare invocation, a flag, or a prompt, and only hands off to the
            // IDE for `ide`/`chat`/`serve-web`/`tunnel` or a path that exists.
            // Detecting only `qodercli` would miss every session started the
            // documented way, since the dispatcher is what the pty sees. An IDE
            // launch is the cost: it wears the CLI's avatar for as long as the
            // launcher takes to exit.
            CLIAgent::QoderCLI => &["qoder", "qodercli"],
            // The mainland-China build of Qoder: the same vendor as the global
            // one, a separate install that keeps its state in `~/.qoder-cn`.
            // It ships the same dispatcher-plus-CLI pair under `-cn` names, so
            // `qoder-cn` is detected too. A bare `qoder` still belongs to the
            // global build.
            CLIAgent::QoderCLICn => &["qodercn", "qoderclicn", "qoder-cn"],
            // Charm's terminal agent. One binary, and the name on `PATH` is
            // the one it starts as.
            CLIAgent::Crush => &["crush"],
            // Tencent's terminal agent. The npm package installs `codebuddy`,
            // `codebuddy-code` and the short `cbc`, all pointing at one script.
            // `cbc` also names the COIN-OR solver; a solver run wearing the
            // avatar until it exits is the cost of catching the short name.
            CLIAgent::CodeBuddy => &["codebuddy", "codebuddy-code", "cbc"],
            CLIAgent::Empryo => &["empryo"],
            CLIAgent::Muse => &["muse"],
            CLIAgent::Jcode => &["jcode"],
            CLIAgent::PrimeAgent => &["prime-agent"],
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            CLIAgent::Claude => "claude",
            CLIAgent::Codex => "codex",
            CLIAgent::TraeCode => "traecli",
            CLIAgent::Gemini => "gemini",
            CLIAgent::Aider => "aider",
            CLIAgent::Amp => "amp",
            CLIAgent::OpenCode => "opencode",
            CLIAgent::Copilot => "copilot",
            CLIAgent::Cursor => "cursor",
            CLIAgent::Goose => "goose",
            CLIAgent::Droid => "droid",
            CLIAgent::Pi => "pi",
            CLIAgent::Auggie => "auggie",
            CLIAgent::Hermes => "hermes",
            CLIAgent::Vibe => "vibe",
            CLIAgent::Antigravity => "antigravity",
            CLIAgent::Grok => "grok",
            CLIAgent::Qwen => "qwen",
            CLIAgent::OhMyPi => "omp",
            CLIAgent::Kimi => "kimi",
            CLIAgent::QoderCLI => "qodercli",
            CLIAgent::Crush => "crush",
            CLIAgent::CodeBuddy => "codebuddy",
            CLIAgent::Empryo => "empryo",
            CLIAgent::Muse => "muse",
            CLIAgent::Jcode => "jcode",
            CLIAgent::PrimeAgent => "prime-agent",
            CLIAgent::QoderCLICn => "qoderclicn",
        }
    }

    pub fn from_slug(name: &str) -> Option<CLIAgent> {
        let name = name.trim().to_ascii_lowercase();
        CLIAgent::ALL.into_iter().find(|a| a.slug() == name)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            CLIAgent::Claude => "Claude Code",
            CLIAgent::Codex => "Codex",
            CLIAgent::TraeCode => "TraeCode",
            CLIAgent::Gemini => "Gemini",
            CLIAgent::Aider => "Aider",
            CLIAgent::Amp => "Amp",
            CLIAgent::OpenCode => "OpenCode",
            CLIAgent::Copilot => "Copilot",
            CLIAgent::Cursor => "Cursor",
            CLIAgent::Goose => "Goose",
            CLIAgent::Droid => "Droid",
            CLIAgent::Pi => "Pi",
            CLIAgent::Auggie => "Auggie",
            CLIAgent::Hermes => "Hermes",
            CLIAgent::Vibe => "Vibe",
            CLIAgent::Antigravity => "Antigravity",
            CLIAgent::Grok => "Grok",
            CLIAgent::Qwen => "Qwen Code",
            CLIAgent::OhMyPi => "Oh My Pi",
            CLIAgent::Kimi => "Kimi Code",
            CLIAgent::QoderCLI => "Qoder CLI",
            CLIAgent::Crush => "Crush",
            CLIAgent::CodeBuddy => "CodeBuddy",
            CLIAgent::Empryo => "Empryo",
            CLIAgent::Muse => "Muse Code",
            CLIAgent::Jcode => "jcode",
            CLIAgent::PrimeAgent => "Prime Agent",
            CLIAgent::QoderCLICn => "Qoder CN CLI",
        }
    }

    pub fn resume_command(
        self,
        session_id: &str,
        launch_argv: Option<&[String]>,
    ) -> Option<String> {
        if launch_argv.is_some_and(|argv| self.opts_out_of_sessions(argv)) {
            return None;
        }
        let flags = self.session_command_flags(session_id, launch_argv)?;
        match self {
            CLIAgent::Claude => Some(format!("claude{flags} --resume {session_id}")),
            CLIAgent::Codex => Some(format!("codex resume {session_id}{flags}")),
            CLIAgent::TraeCode => Some(format!("traecli resume {session_id}{flags}")),
            CLIAgent::Gemini => Some(format!("gemini{flags} --resume {session_id}")),
            CLIAgent::OpenCode => Some(format!("opencode{flags} --session {session_id}")),
            CLIAgent::Amp => Some(format!("amp threads continue {session_id}{flags}")),
            CLIAgent::Auggie => Some(format!("auggie{flags} --resume {session_id}")),
            CLIAgent::Hermes => Some(format!("hermes chat{flags} --resume {session_id}")),
            CLIAgent::Qwen => Some(format!("qwen{flags} --resume {session_id}")),
            CLIAgent::Goose => Some(format!(
                "goose session{flags} --resume --session-id {session_id}"
            )),
            CLIAgent::Vibe => Some(format!("vibe{flags} --resume {session_id}")),
            CLIAgent::Antigravity => Some(format!("agy{flags} --conversation {session_id}")),
            CLIAgent::Cursor => Some(format!("cursor-agent{flags} --resume {session_id}")),
            CLIAgent::Droid => Some(format!("droid{flags} --resume {session_id}")),
            CLIAgent::Copilot => Some(format!("copilot{flags} --resume {session_id}")),
            CLIAgent::Grok => Some(format!("grok{flags} --resume {session_id}")),
            CLIAgent::QoderCLI => Some(format!("qodercli{flags} --resume {session_id}")),
            CLIAgent::QoderCLICn => Some(format!("qodercn{flags} --resume {session_id}")),
            CLIAgent::Pi => Some(format!("pi{flags} --session {session_id}")),
            CLIAgent::OhMyPi => Some(format!("omp{flags} --resume {session_id}")),
            CLIAgent::Kimi => Some(format!("kimi{flags} --session {session_id}")),
            CLIAgent::Crush => Some(format!("crush{flags} --session {session_id}")),
            CLIAgent::CodeBuddy => Some(format!("codebuddy{flags} --resume {session_id}")),
            CLIAgent::Empryo => Some(format!("empryo{flags} --session {session_id}")),
            // Root options may sit on either side of `resume`.
            CLIAgent::Muse => Some(format!("muse{flags} resume {session_id}")),
            CLIAgent::Jcode => Some(format!("jcode{flags} --resume {session_id}")),
            CLIAgent::PrimeAgent => Some(format!("prime-agent{flags} --resume {session_id}")),
            _ => None,
        }
    }

    fn opts_out_of_sessions(self, argv: &[String]) -> bool {
        let ephemeral: &[&str] = match self {
            CLIAgent::Pi | CLIAgent::OhMyPi | CLIAgent::PrimeAgent => &["--no-session"],
            // "Do not save conversation history" — nothing is persisted, so
            // there is no session left to resume from.
            CLIAgent::Auggie => &["--dont-save-session"],
            // "If false, chat history is not saved and --continue/--resume
            // will not work" — the yargs negation of `--chat-recording`.
            CLIAgent::Qwen => &["--no-chat-recording"],
            // Print mode still emits a session id in hooks when persistence
            // is disabled, but there is no saved conversation to reopen.
            CLIAgent::QoderCLI | CLIAgent::CodeBuddy | CLIAgent::QoderCLICn => {
                &["--no-session-persistence"]
            }
            _ => &[],
        };
        argv.iter().any(|t| ephemeral.contains(&t.as_str()))
    }

    pub fn fork_command(self, session_id: &str, launch_argv: Option<&[String]>) -> Option<String> {
        if launch_argv.is_some_and(|argv| self.opts_out_of_sessions(argv)) {
            return None;
        }
        let flags = self.session_command_flags(session_id, launch_argv)?;
        match self {
            CLIAgent::Codex => Some(format!("codex fork {session_id}{flags}")),
            CLIAgent::TraeCode => Some(format!("traecli fork {session_id}{flags}")),
            CLIAgent::Claude => Some(format!(
                "claude{flags} --resume {session_id} --fork-session"
            )),
            CLIAgent::Grok => Some(format!("grok{flags} --resume {session_id} --fork-session")),
            CLIAgent::QoderCLI => Some(format!(
                "qodercli{flags} --resume {session_id} --fork-session"
            )),
            // qoder-cn is the same CLI against a different config directory, so
            // it takes the same switches.
            CLIAgent::QoderCLICn => Some(format!(
                "qodercn{flags} --resume {session_id} --fork-session"
            )),
            CLIAgent::CodeBuddy => Some(format!(
                "codebuddy{flags} --resume {session_id} --fork-session"
            )),
            CLIAgent::OpenCode => Some(format!("opencode{flags} --session {session_id} --fork")),
            CLIAgent::OhMyPi => Some(format!("omp{flags} --fork {session_id}")),
            CLIAgent::PrimeAgent => Some(format!("prime-agent{flags} --fork {session_id}")),
            // Droid forks with a standalone flag rather than resume-plus-a-switch.
            CLIAgent::Droid => Some(format!("droid{flags} --fork {session_id}")),
            // `fork` is missing from `amp threads --help`, but the subcommand is
            // real — `amp threads fork --help` prints its own usage.
            CLIAgent::Amp => Some(format!("amp threads fork {session_id}{flags}")),
            CLIAgent::Qwen => Some(format!("qwen{flags} --resume {session_id} --fork-session")),
            // Goose forks by adding a switch to the same resume invocation.
            CLIAgent::Goose => Some(format!(
                "goose session{flags} --resume --fork --session-id {session_id}"
            )),
            _ => None,
        }
    }

    pub fn fork_label(self) -> Option<&'static str> {
        match self {
            CLIAgent::Claude
            | CLIAgent::Codex
            | CLIAgent::TraeCode
            | CLIAgent::Grok
            | CLIAgent::OpenCode
            | CLIAgent::OhMyPi
            | CLIAgent::PrimeAgent
            | CLIAgent::Droid
            | CLIAgent::Amp
            | CLIAgent::Qwen
            | CLIAgent::Goose
            | CLIAgent::QoderCLI
            | CLIAgent::QoderCLICn
            | CLIAgent::CodeBuddy => Some("Fork Session"),
            _ => None,
        }
    }

    /// A fresh start under `session_id`, for when [`Self::resume_command`]
    /// finds nothing: Claude saves no conversation for a session that never
    /// took a turn (or ran with transcript saving off), and resuming one
    /// fails at once. `None` for agents that cannot name a new session.
    pub fn start_command(self, session_id: &str, launch_argv: Option<&[String]>) -> Option<String> {
        if self != CLIAgent::Claude || uuid::Uuid::parse_str(session_id).is_err() {
            return None;
        }
        let flags = self.session_command_flags(session_id, launch_argv)?;
        Some(format!("claude{flags} --session-id {session_id}"))
    }

    /// The line a restored pane types to bring its agent back: the resume,
    /// falling back to [`Self::start_command`] with `||` when the pane's shell
    /// has that operator. `shell_program` is that shell, `None` when it is not
    /// known — then, as in a shell without `||`, the resume goes alone, since
    /// one that cannot parse the line would not run the resume either.
    ///
    /// `||` fires on any failed exit, not only "no conversation found". The
    /// other ways a resume fails fast (a flag this version rejects, no login)
    /// fail the fresh start the same way; one that exits non-zero after the
    /// user took turns follows with a start under an id that now has a
    /// conversation, which Claude turns down as already in use. Telling the
    /// cases apart would mean reading Claude's stderr, which a typed line
    /// cannot do portably.
    pub fn restore_command(
        self,
        session_id: &str,
        launch_argv: Option<&[String]>,
        shell_program: Option<&str>,
    ) -> Option<String> {
        let resume = self.resume_command(session_id, launch_argv)?;
        let fresh = shell_program
            .filter(|shell| crate::shell_quote::runs_or_list(shell))
            .and_then(|_| self.start_command(session_id, launch_argv));
        Some(match fresh {
            Some(fresh) => format!("{resume} || {fresh}"),
            None => resume,
        })
    }

    fn session_command_flags(
        self,
        session_id: &str,
        launch_argv: Option<&[String]>,
    ) -> Option<String> {
        // A leading `-` would be read as a flag, not the id: `--resume
        // --dangerously-skip-permissions`. No agent writes such an id, but
        // the listing takes ids from file and directory names on disk.
        if session_id.is_empty()
            || session_id.starts_with('-')
            || !session_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        {
            return None;
        }
        Some(
            launch_argv
                .and_then(|argv| self.replay_flags(argv))
                .map(|flags| {
                    flags.iter().fold(String::new(), |mut s, f| {
                        s.push(' ');
                        s.push_str(f);
                        s
                    })
                })
                .unwrap_or_default(),
        )
    }

    fn replay_flags(self, argv: &[String]) -> Option<Vec<String>> {
        let tail = self.session_free_tail(argv)?;
        let safe = |t: &str| {
            !t.is_empty()
                && t.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_=./,:@+~".contains(&b))
        };
        if !tail.iter().all(|t| safe(t)) {
            return None;
        }
        let mut prev_was_flag = false;
        for t in &tail {
            let is_flag = t.starts_with('-');
            if !is_flag && !prev_was_flag {
                return None;
            }
            prev_was_flag = is_flag;
        }
        Some(tail.into_iter().map(String::from).collect())
    }

    /// What follows the agent's own name in `argv`, with everything that names
    /// a session taken out: the resume/fork subcommands and the flags listed as
    /// stale for this agent, each with its value. `None` when no token in
    /// `argv` names this agent — a wrapper, where there is no telling which
    /// arguments are the agent's.
    fn session_free_tail(self, argv: &[String]) -> Option<Vec<&str>> {
        let names_self = |token: &str| {
            token.split(['/', '\\']).any(|seg| {
                CLIAgent::match_token(&base_stem(seg).to_ascii_lowercase()) == Some(self)
            })
        };
        let argv = &argv[argv.iter().take_while(|t| is_env_assignment(t)).count()..];
        let named = argv.iter().position(|t| names_self(t))?;
        let mut tail: Vec<&str> = argv[named + 1..].iter().map(String::as_str).collect();

        if matches!(self, CLIAgent::Codex | CLIAgent::TraeCode | CLIAgent::Muse)
            && matches!(tail.first(), Some(&"resume") | Some(&"fork"))
        {
            tail.remove(0);
            if tail.first().is_some_and(|t| !t.starts_with('-')) {
                tail.remove(0);
            }
        }

        // Agents that reach their session through subcommands leave `stale`
        // nothing to drop — `amp threads continue <id>` names the thread with a
        // positional argument, and `goose session --resume` hides the flags one
        // level down. Either way the prefix has to come off here, because the
        // "a bare token must follow a flag" check below would otherwise reject
        // the tail wholesale and take every launch flag down with it. The
        // replacement command spells the subcommand out again itself.
        let (groups, verbs): (&[&str], &[&str]) = match self {
            CLIAgent::Amp => (
                &["threads", "t"],
                &["continue", "c", "fork", "f", "handoff", "h"],
            ),
            CLIAgent::Auggie => (&["session"], &["resume", "continue"]),
            CLIAgent::Goose => (&["session", "s"], &[]),
            CLIAgent::Hermes => (&["chat"], &[]),
            _ => (&[], &[]),
        };
        if tail.first().is_some_and(|t| groups.contains(t)) {
            tail.remove(0);
            if tail.first().is_some_and(|t| verbs.contains(t)) {
                tail.remove(0);
                if tail.first().is_some_and(|t| !t.starts_with('-')) {
                    tail.remove(0);
                }
            }
        }

        let stale: &[&str] = match self {
            CLIAgent::Claude => &[
                "--resume",
                "-r",
                "--continue",
                "-c",
                "--session-id",
                "--from-pr",
                "--fork-session",
            ],
            // `--session-id` and `--session-file` name a session too, and Gemini
            // rejects them outright alongside `--resume`.
            CLIAgent::Gemini => &["--resume", "-r", "--session-id", "--session-file"],
            CLIAgent::Cursor => &["--resume", "-r", "--continue"],
            CLIAgent::Copilot | CLIAgent::Auggie | CLIAgent::Hermes => {
                &["--resume", "-r", "--continue", "-c"]
            }
            // `--session-id` names a *new* session and Qwen rejects it
            // alongside `--resume`, so it is as stale as the resume flags.
            CLIAgent::Qwen => &[
                "--resume",
                "-r",
                "--continue",
                "-c",
                "--fork-session",
                "--session-id",
            ],
            CLIAgent::Droid => &["--resume", "-r", "--fork", "--session-id", "-s"],
            // `--session-id`/`--id`, `-n`/`--name` and the legacy `--path` are
            // one mutually-exclusive clap group in Goose; any of them surviving
            // next to the `--session-id` this command appends is a parse error.
            CLIAgent::Goose => &[
                "--resume",
                "-r",
                "--fork",
                "--session-id",
                "--id",
                "--name",
                "-n",
                "--path",
            ],
            CLIAgent::Vibe => &["--resume", "--continue", "-c"],
            CLIAgent::Antigravity => &["--conversation", "--continue", "-c"],
            CLIAgent::OpenCode => &["--session", "-s", "--continue", "-c", "--fork"],
            CLIAgent::Codex => &["--last"],
            CLIAgent::TraeCode => &["--last", "--resume", "--session-id"],
            CLIAgent::Pi => &[
                "--session",
                "--session-id",
                "--fork",
                "--resume",
                "-r",
                "--continue",
                "-c",
            ],
            // `--resume`, `-r` and `--session` are three spellings of one flag
            // in Oh My Pi; `--session-dir` is a different one and survives.
            CLIAgent::OhMyPi => &["--resume", "-r", "--session", "--fork", "--continue", "-c"],
            // Prime Agent is a Pi fork with the same session switches;
            // `--session-dir` is a different option and survives.
            CLIAgent::PrimeAgent => &["--resume", "-r", "--fork", "--continue", "-c"],
            // empryo resumes with `--session <id>`; `--save-session` only
            // applies to headless runs and is not a session selector.
            CLIAgent::Empryo => &["--session"],
            // `muse resume --last` picks the newest session instead of the id.
            CLIAgent::Muse => &["--last"],
            CLIAgent::Jcode => &["--resume"],
            // `--resume`/`-r` is Kimi's hidden alias for `--session`/`-S`.
            // `--agent`/`--agent-file` bind the main agent at session creation
            // and Kimi rejects either next to `--session` outright; resuming
            // restores the bound agent by itself, so replaying them would only
            // turn a working resume into a startup error.
            CLIAgent::Kimi => &[
                "--session",
                "-S",
                "--resume",
                "-r",
                "--continue",
                "-c",
                "--agent",
                "--agent-file",
            ],
            CLIAgent::Grok => &[
                "--resume",
                "-r",
                "--load",
                "--continue",
                "-c",
                "--session-id",
                "-s",
                "--fork-session",
                "--worktree",
                "-w",
                "--worktree-ref",
                "--ref",
            ],
            // `--resume`/`-r` restores a past session and `--continue`/`-c` the
            // most recent one; `--session-id` is a third spelling of the same
            // thing. All three clash with the `--resume {id}` this command
            // appends, and `--fork-session` is the flag the fork variant
            // appends itself. `--worktree` would create or switch trees again;
            // Qoder's `-w` means `--cwd` and must survive.
            CLIAgent::QoderCLI | CLIAgent::QoderCLICn => &[
                "--resume",
                "-r",
                "--continue",
                "-c",
                "--session-id",
                "--fork-session",
                "--worktree",
            ],
            // `--session`/`-s` names the conversation to restore and
            // `--continue`/`-C` the most recent one; both clash with the
            // `--session {id}` this command appends. `-c` is *not* in that
            // group here — Crush spells `--cwd` with it, and it must survive.
            CLIAgent::Crush => &["--session", "-s", "--continue", "-C"],
            // CodeBuddy keeps Claude Code's session flags. `--resume-session-at`
            // cuts a resumed conversation short and only makes sense next to
            // the resume it came with. Unlike Qoder, `-w` *is* `--worktree`
            // here, and replaying either would branch off a fresh tree.
            CLIAgent::CodeBuddy => &[
                "--resume",
                "-r",
                "--continue",
                "-c",
                "--session-id",
                "--fork-session",
                "--resume-session-at",
                "--worktree",
                "-w",
                "--worktree-branch",
            ],
            _ => &[],
        };
        let mut i = 0;
        while i < tail.len() {
            let t = tail[i];
            if stale.contains(&t)
                || stale
                    .iter()
                    .any(|f| f.len() > 2 && t.starts_with(&format!("{f}=")))
            {
                tail.remove(i);
                if i < tail.len() && !tail[i].starts_with('-') {
                    tail.remove(i);
                }
            } else {
                i += 1;
            }
        }
        Some(tail)
    }

    pub fn accent_rgb(self) -> u32 {
        match self {
            CLIAgent::Claude => 0xD97757,
            CLIAgent::Codex => 0x000000,
            // The brand mark carries its own green foreground on a black
            // field, so the surrounding tab avatar needs to stay black too.
            CLIAgent::TraeCode => 0x000000,
            CLIAgent::Gemini => 0x4285F4,
            CLIAgent::Aider => 0x14B014,
            CLIAgent::Amp => 0xF34E3F,
            CLIAgent::OpenCode => 0x6E56CF,
            CLIAgent::Copilot => 0x8957E5,
            CLIAgent::Cursor => 0x9AA0A6,
            CLIAgent::Goose => 0x3ECC5F,
            CLIAgent::Droid => 0xEF6F2E,
            CLIAgent::Pi => 0x0EA5E9,
            CLIAgent::Auggie => 0x16A34A,
            CLIAgent::Hermes => 0x8B5CF6,
            CLIAgent::Vibe => 0xFA520F,
            CLIAgent::Antigravity => 0x3186FF,
            CLIAgent::Grok => 0x000000,
            CLIAgent::Qwen => 0x6D44E8,
            CLIAgent::OhMyPi => 0xF97316,
            // The blue of the flame in Kimi's brand mark; the glyph itself is
            // black, which Codex and Grok already have covered.
            CLIAgent::Kimi => 0x027AFF,
            CLIAgent::QoderCLI => 0xFFFFFF,
            // qoder-cn ships the same mark on the same black field.
            CLIAgent::QoderCLICn => 0xFFFFFF,
            // The blue-violet field Charm ships the Crush heart on.
            CLIAgent::Crush => 0x6B50FF,
            CLIAgent::Empryo => 0xE8663D,
            CLIAgent::Muse => 0x0866FF,
            CLIAgent::Jcode => 0x111111,
            CLIAgent::PrimeAgent => 0x111111,
            // The near-black field CodeBuddy's own app icon sits on.
            CLIAgent::CodeBuddy => 0x1F1F1F,
        }
    }

    /// The colour the agent's mark is drawn in.
    ///
    /// SVG assets render as a single-colour mask, so whatever `fill` the file
    /// carries is thrown away and the colour has to come from here. It lives
    /// next to [`Self::accent_rgb`] rather than at either draw site because
    /// there are two of those — the tab avatar and the tray icon — and a mark
    /// that answers differently depending on which one is asking is the same
    /// agent wearing two faces.
    ///
    /// White for every mark that is a silhouette sitting on its brand colour.
    /// TraeCode is the one whose mark is the coloured half of the pair, on an
    /// accent that is deliberately black.
    pub fn icon_rgb(self) -> u32 {
        match self {
            CLIAgent::TraeCode => 0x32F08C,
            CLIAgent::QoderCLI | CLIAgent::QoderCLICn => 0x000000,
            _ => 0xFFFFFF,
        }
    }

    pub fn icon_path(self) -> &'static str {
        match self {
            CLIAgent::Claude => "icons/agents/claude.svg",
            CLIAgent::Codex => "icons/agents/codex.svg",
            CLIAgent::TraeCode => "icons/agents/traecli.svg",
            CLIAgent::Gemini => "icons/agents/gemini.svg",
            CLIAgent::Amp => "icons/agents/amp.svg",
            CLIAgent::OpenCode => "icons/agents/opencode.svg",
            CLIAgent::Copilot => "icons/agents/copilot.svg",
            CLIAgent::Cursor => "icons/agents/cursor.svg",
            CLIAgent::Goose => "icons/agents/goose.svg",
            CLIAgent::Droid => "icons/agents/droid.svg",
            CLIAgent::Grok => "icons/agents/grok.svg",
            CLIAgent::Pi => "icons/agents/pi.svg",
            CLIAgent::OhMyPi => "icons/agents/omp.svg",
            CLIAgent::Qwen => "icons/agents/qwen.svg",
            CLIAgent::Kimi => "icons/agents/kimi.svg",
            CLIAgent::QoderCLI => "icons/agents/qodercli.svg",
            CLIAgent::QoderCLICn => "icons/agents/qoderclicn.svg",
            CLIAgent::Crush => "icons/agents/crush.svg",
            CLIAgent::CodeBuddy => "icons/agents/codebuddy.svg",
            CLIAgent::Aider
            | CLIAgent::Auggie
            | CLIAgent::Hermes
            | CLIAgent::Vibe
            | CLIAgent::Antigravity
            | CLIAgent::Empryo
            | CLIAgent::PrimeAgent
            | CLIAgent::Muse
            | CLIAgent::Jcode => "icons/bot.svg",
        }
    }

    fn match_token(token: &str) -> Option<CLIAgent> {
        // Muse Code's `muse` launcher execs a versioned binary,
        // `muse-bin-<version>`, so that is the name the process runs under.
        let token = if token.starts_with("muse-bin-") {
            "muse"
        } else {
            token
        };
        CLIAgent::ALL
            .into_iter()
            .find(|a| a.aliases().contains(&token))
    }

    /// The program a launch with no override runs: the first name the agent
    /// is detected by, which is the one its installer puts on `PATH`.
    pub fn binary(self) -> &'static str {
        self.aliases()[0]
    }

    /// The command line a quick launch types for this agent: the user's
    /// `agent_launch` entry when there is one, the bare binary otherwise.
    ///
    /// Keys are matched the way [`CLIAgent::from_slug`] reads them, so a
    /// hand-written `"Claude"` still counts. A blank entry is no override.
    pub fn launch_command(self, overrides: &HashMap<String, String>) -> String {
        overrides
            .iter()
            .find(|(slug, _)| CLIAgent::from_slug(slug) == Some(self))
            .map(|(_, line)| line.trim())
            .filter(|line| !line.is_empty())
            .map_or_else(|| self.binary().to_string(), str::to_string)
    }

    /// The arguments that start an interactive session whose first message is
    /// `prompt`. `None` for an agent whose positional prompt means a one-shot,
    /// non-interactive run, or whose form is not known — the caller then
    /// starts the agent bare rather than guess.
    pub fn prompt_args(self, prompt: &str) -> Option<Vec<String>> {
        match self {
            CLIAgent::Claude | CLIAgent::Codex => Some(vec![prompt.to_string()]),
            CLIAgent::Gemini => Some(vec!["-i".to_string(), prompt.to_string()]),
            _ => None,
        }
    }

    /// `argv` as a command line worth launching this agent with again: its
    /// binary followed by the flags it ran with, minus everything that names
    /// one particular session (a resume, a fork, a session id) and minus
    /// positional arguments such as a prompt, which belong to that one run.
    ///
    /// A run that went through something that does not name the agent — a
    /// wrapper script mapped by `agent_commands` — cannot be taken apart, so
    /// it comes back whole: it is what was typed, and it launched the agent.
    pub fn launch_argv_for_default(self, argv: &[String]) -> Option<Vec<String>> {
        let env = argv.iter().take_while(|t| is_env_assignment(t)).count();
        if argv.len() == env {
            return None;
        }
        let Some(tail) = self.session_free_tail(argv) else {
            return Some(argv.to_vec());
        };
        let mut out: Vec<String> = argv[..env].to_vec();
        out.push(self.binary().to_string());
        let mut takes_value = false;
        for t in tail {
            let is_flag = t.starts_with('-') && t.len() > 1;
            if is_flag {
                out.push(t.to_string());
                takes_value = !t.contains('=');
            } else if takes_value {
                out.push(t.to_string());
                takes_value = false;
            }
        }
        Some(out)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn detect_from_argv(argv: &[String]) -> Option<CLIAgent> {
        Self::detect_from_argv_with(argv, &HashMap::new())
    }

    pub fn detect_from_argv_with(
        argv: &[String],
        custom: &HashMap<String, String>,
    ) -> Option<CLIAgent> {
        let mut rest = argv
            .iter()
            .map(String::as_str)
            .skip_while(|t| is_env_assignment(t));

        let launcher = rest.next()?;
        let launcher_stem = base_stem(launcher);

        if let Some(agent) = CLIAgent::match_token(launcher_stem) {
            return Some(agent);
        }
        if let Some(agent) = custom
            .get(&launcher_stem.to_ascii_lowercase())
            .and_then(|slug| CLIAgent::from_slug(slug))
        {
            return Some(agent);
        }

        // A wrapper that is a script runs as its interpreter, so the name the
        // user mapped is the script's, one token in: `bash ~/bin/cc`,
        // `node ~/bin/cc.js`. Only the mapped names are looked for there — the
        // built-in ones already have the scan below.
        if (is_interpreter(launcher_stem) || is_shell(launcher_stem))
            && let Some(agent) = rest
                .clone()
                .find(|a| !a.starts_with('-') && !is_env_assignment(a))
                .and_then(|script| custom.get(&base_stem(script).to_ascii_lowercase()))
                .and_then(|slug| CLIAgent::from_slug(slug))
        {
            return Some(agent);
        }

        if is_interpreter(launcher_stem) {
            for arg in rest {
                if arg.starts_with('-') {
                    continue;
                }
                for segment in arg.split(['/', '\\']) {
                    if let Some(agent) =
                        CLIAgent::match_token(&base_stem(segment).to_ascii_lowercase())
                    {
                        return Some(agent);
                    }
                }
            }
        }

        None
    }

    pub fn detect_from_command_with(
        command: &str,
        custom: &HashMap<String, String>,
    ) -> Option<CLIAgent> {
        let argv: Vec<String> = command_argv(command)
            .iter()
            .map(|t| t.to_ascii_lowercase())
            .collect();
        Self::detect_from_argv_with(&argv, custom)
    }
}

/// Splits a shell-integration command capture into argv tokens, preserving
/// case so the result can serve as `launch_argv` for flag replay on resume.
/// Quoted arguments containing spaces come out split; `replay_flags` rejects
/// such tokens rather than replaying them wrong.
pub fn command_argv(command: &str) -> Vec<String> {
    let mut argv: Vec<String> = command
        .split_whitespace()
        .map(|t| t.trim_matches(['"', '\'']).to_string())
        .filter(|t| !t.is_empty())
        .collect();
    if argv.first().is_some_and(|t| t == "&") {
        argv.remove(0);
    }
    argv
}

fn is_env_assignment(token: &str) -> bool {
    match token.split_once('=') {
        Some((key, _)) => {
            let mut bytes = key.bytes();
            bytes
                .next()
                .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        }
        None => false,
    }
}

fn base_stem(token: &str) -> &str {
    let trimmed = token.trim_end_matches(['/', '\\']);
    let name = match trimmed.rfind(['/', '\\']) {
        Some(i) => &trimmed[i + 1..],
        None => trimmed,
    };
    for ext in [
        ".js", ".mjs", ".cjs", ".ts", ".py", ".rb", ".sh", ".exe", ".cmd", ".bat", ".ps1",
    ] {
        if let Some(stem) = name.strip_suffix(ext) {
            return stem;
        }
    }
    name
}

fn is_interpreter(stem: &str) -> bool {
    matches!(
        stem.to_ascii_lowercase().as_str(),
        "node"
            | "nodejs"
            | "bun"
            | "deno"
            | "npx"
            | "pnpm"
            | "yarn"
            | "python"
            | "python3"
            | "ruby"
            | "uv"
            | "uvx"
            | "env"
    )
}

fn is_shell(stem: &str) -> bool {
    matches!(
        stem.to_ascii_lowercase().as_str(),
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish"
    )
}

/// The program a launch command line runs: its first word that is not an
/// environment assignment.
pub fn launch_program(command: &str) -> Option<String> {
    command_argv(command)
        .into_iter()
        .find(|t| !is_env_assignment(t))
}

/// The wrapper names `agent_launch` implies, in the shape `agent_commands`
/// takes: `{"claude": "cc --fast"}` means a `cc` in a pane is Claude Code.
///
/// This is how a pane opened by a quick launch through a wrapper is still
/// recognised without the user also writing an `agent_commands` entry: the
/// launch typed exactly this command, so its program is known to be that
/// agent. A program that is already an agent's own name needs no entry, and
/// an interpreter (`npx`, `node`, `env`, …) or a shell is never mapped
/// wholesale — `npx` is not Claude Code just because Claude Code is one of the
/// things it can run.
pub fn launch_aliases(overrides: &HashMap<String, String>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (slug, line) in overrides {
        let Some(agent) = CLIAgent::from_slug(slug) else {
            continue;
        };
        let Some(program) = launch_program(line) else {
            continue;
        };
        let stem = base_stem(&program).to_ascii_lowercase();
        if stem.is_empty()
            || CLIAgent::match_token(&stem).is_some()
            || is_interpreter(&stem)
            || is_shell(&stem)
        {
            continue;
        }
        out.insert(stem, agent.slug().to_string());
    }
    out
}

/// Whether `program` names an executable file, either directly (it has a
/// directory in it) or through one of the directories in `path`.
///
/// On Windows a bare name is also tried with the extensions an npm or winget
/// install leaves behind, since `claude` there is really `claude.cmd`.
pub fn program_on_path(program: &str, path: &std::ffi::OsStr) -> bool {
    let program = program.trim();
    if program.is_empty() {
        return false;
    }
    if program.contains(['/', '\\']) {
        // The launch is typed into a shell, which expands a leading `~/`.
        let home = program
            .strip_prefix("~/")
            .and_then(|rest| Some(std::path::PathBuf::from(std::env::var_os("HOME")?).join(rest)));
        return is_executable(home.as_deref().unwrap_or(std::path::Path::new(program)));
    }
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat", ".ps1"]
    } else {
        &[""]
    };
    std::env::split_paths(path).any(|dir| {
        exts.iter()
            .any(|ext| is_executable(&dir.join(format!("{program}{ext}"))))
    })
}

fn is_executable(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
