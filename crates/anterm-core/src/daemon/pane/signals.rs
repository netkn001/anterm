use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use portable_pty::MasterPty;

use crate::osc::OscTokenizer;
use crate::daemon::protocol::{DaemonMsg, RemoteContext, ShellSpec};

use super::PaneState;
use super::fanout::notify;

/// When the agent session last heard from the agent, for the two conclusions
/// anterm draws from silence: a turn the user interrupted, and a turn whose end
/// was lost.
#[derive(Default)]
pub(super) struct AgentClock {
    /// Bumped by every hook event and every session reset, so a pending
    /// interrupt can tell whether anything spoke after the key it was armed by.
    pub(super) generation: u64,
    /// The last hook event, or when the sweep first saw a turn it had no
    /// event time for (a pane carried across a daemon handoff).
    pub(super) last_event: Option<std::time::Instant>,
    /// An interrupt key is already waiting out [`INTERRUPT_SETTLE`].
    pub(super) interrupt_pending: bool,
}

/// How long after an interrupt key a real hook event still gets to speak
/// first. Agents that do report an interrupt (Kimi's `Interrupt`, a `Stop`
/// racing the key) do it well inside this.
const INTERRUPT_SETTLE: Duration = Duration::from_millis(1000);

/// How long a turn may stay on working with no hook event at all before the
/// status is given up on. Far past any single quiet stretch a live turn has:
/// Claude's longest tool call is capped at ten minutes, and every other event
/// — a tool finishing, a permission prompt — resets the clock.
pub(crate) const AGENT_STALE_AFTER: Duration = Duration::from_secs(30 * 60);

/// Whether one input write is the user asking a running agent to stop: a bare
/// <kbd>Esc</kbd> or <kbd>Ctrl+C</kbd>, in the legacy encoding or in the
/// kitty keyboard protocol and xterm `modifyOtherKeys` forms a TUI may have
/// switched the terminal into. Only a whole write counts — the key arrives on
/// its own, and a paste that happens to hold `0x03` is not a keypress.
#[cfg(test)]
pub(super) fn is_interrupt_key(bytes: &[u8]) -> bool {
    interrupt_key(bytes).is_some()
}

/// Which interrupt key one input write is — see [`is_interrupt_key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InterruptKey {
    Escape,
    CtrlC,
}

/// Whether `bytes` stops `agent`'s turn. Grok Build takes only Ctrl+C for
/// that: its Esc leaves a running turn running, and counting it would call
/// the turn over while it still works.
pub(super) fn interrupts(agent: Option<crate::cli_agent::CLIAgent>, bytes: &[u8]) -> bool {
    match interrupt_key(bytes) {
        Some(InterruptKey::Escape) => agent != Some(crate::cli_agent::CLIAgent::Grok),
        Some(InterruptKey::CtrlC) => true,
        None => false,
    }
}

fn interrupt_key(bytes: &[u8]) -> Option<InterruptKey> {
    match bytes {
        b"\x1b" => return Some(InterruptKey::Escape),
        b"\x03" | b"\x1b[27;5;99~" => return Some(InterruptKey::CtrlC),
        _ => {}
    }
    let body = bytes
        .strip_prefix(b"\x1b[")
        .and_then(|b| b.strip_suffix(b"u"))
        .and_then(|b| std::str::from_utf8(b).ok())?;
    let mut fields = body.split(';');
    let code = fields
        .next()
        .and_then(|f| f.split(':').next())
        .and_then(|c| c.parse::<u32>().ok())?;
    let mut modifiers = fields.next().unwrap_or("1").split(':');
    let mods = modifiers
        .next()
        .and_then(|m| m.parse::<u32>().ok())
        .unwrap_or(1);
    let event = modifiers.next().map_or(Some(1), |e| e.parse::<u32>().ok());
    if event != Some(1) {
        return None;
    }
    // Caps Lock and Num Lock ride along in the mask without changing the key.
    const LOCKS: u32 = 64 | 128;
    let mods = mods.saturating_sub(1) & !LOCKS;
    match (code, mods) {
        (27, 0) => Some(InterruptKey::Escape),
        (99, 4) => Some(InterruptKey::CtrlC),
        _ => None,
    }
}

pub(super) fn agent_state_snapshot(st: &PaneState) -> Option<crate::daemon::control::PaneAgentState> {
    st.agent_session
        .clone()
        .map(|state| crate::daemon::control::PaneAgentState {
            pane_id: st.id,
            agent: st.agent,
            state,
        })
}

/// What the daemon has learned about a pane that the machine tree wants to
/// hold. Compared before and after each read, and written out only when it
/// moved — every field here costs a `PaneFacts` delta to every attached client.
#[derive(Debug, PartialEq)]
pub(super) struct ObservedFacts {
    pub(super) cwd: Option<String>,
    pub(super) osc_title: Option<String>,
    pub(super) agent: Option<crate::machine::AgentFacts>,
    pub(super) shell: Option<ShellSpec>,
}

pub(super) fn observed_facts(st: &PaneState) -> ObservedFacts {
    let cwd = st.cwd.as_ref().map(|p| p.to_string_lossy().into_owned());
    let agent = st.agent.map(|agent| crate::machine::AgentFacts {
        agent,
        session_id: st.agent_session.as_ref().and_then(|s| s.session_id.clone()),
        launch_argv: st
            .agent_session
            .as_ref()
            .and_then(|s| s.launch_argv.clone())
            .or_else(|| st.agent_argv.clone()),
        status: st.agent_session.as_ref().map(|s| s.status),
    });
    ObservedFacts {
        cwd,
        osc_title: st.osc_title.clone(),
        agent,
        shell: st.shell_spec.clone(),
    }
}

pub(super) fn publish_facts(pane: u64, alive: bool, after: ObservedFacts) {
    crate::machine::observe_pane(pane, |p| {
        if after.cwd.is_some() {
            p.cwd = after.cwd;
        }
        // Unlike the others this one is also cleared by a reset, so it is
        // assigned either way.
        p.osc_title = after.osc_title;
        p.agent = after.agent;
        if after.shell.is_some() {
            p.shell = after.shell;
        }
        if alive {
            p.live = true;
        }
    });
}

pub(super) fn facts_changed(before: &ObservedFacts, after: &ObservedFacts) -> bool {
    before.cwd != after.cwd
        || before.osc_title != after.osc_title
        || agent_facts_changed(before.agent.as_ref(), after.agent.as_ref())
        || before.shell != after.shell
}

fn agent_facts_changed(
    before: Option<&crate::machine::AgentFacts>,
    after: Option<&crate::machine::AgentFacts>,
) -> bool {
    match (before, after) {
        (None, None) => false,
        (Some(a), Some(b)) => {
            a.agent != b.agent
                || a.session_id != b.session_id
                || a.launch_argv != b.launch_argv
                || a.status != b.status
        }
        _ => true,
    }
}

pub(super) fn apply_signals(st: &mut PaneState, signals: SniffSignals) {
    if let Some(cwd) = signals.cwd {
        if st.cwd.as_ref() != Some(&cwd) {
            notify(st, DaemonMsg::Cwd(cwd.clone()));
            st.cwd = Some(cwd);
        }
    }
    if let Some(title) = signals.title {
        // No `notify`: a window renders its own tabs from its own terminal,
        // which parsed the same sequence. This is only for the tree.
        st.osc_title = (!title.is_empty()).then_some(title);
    }
    for shell in signals.shell {
        #[cfg(windows)]
        if shell_mark_capture_changed(&st.shell, &shell) {
            apply_agent(
                st,
                agent_from_shell_mark(&shell, &crate::config::agent_detection_aliases()),
            );
        }
        st.shell = shell.clone();
        notify(
            st,
            DaemonMsg::Prompt {
                active: shell.active,
                at_prompt: shell.at_prompt,
                last_exit: shell.last_exit_code,
            },
        );
    }
    apply_agent_signals(st, signals.agent_events, signals.notification);
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn shell_mark_capture_changed(prev: &ShellState, next: &ShellState) -> bool {
    prev.command != next.command
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn agent_from_shell_mark(
    shell: &ShellState,
    custom: &std::collections::HashMap<String, String>,
) -> Option<(crate::cli_agent::CLIAgent, Vec<String>)> {
    let cmd = shell.command.as_deref()?;
    let agent = crate::cli_agent::CLIAgent::detect_from_command_with(cmd, custom)?;
    Some((agent, crate::cli_agent::command_argv(cmd)))
}

/// Start waiting out an interrupt key on a pane whose agent is mid-turn.
///
/// The key only *may* have stopped the turn — it also dismisses menus and
/// clears half-typed input — so nothing changes until [`INTERRUPT_SETTLE`]
/// passes with no hook event. A guess that still turns out wrong is undone by
/// the agent's next finished tool ([`AgentSessionState::apply_event`]).
///
/// [`AgentSessionState::apply_event`]: crate::cli_agent::AgentSessionState::apply_event
pub(super) fn arm_interrupt(state: &Arc<Mutex<PaneState>>) {
    let generation = {
        let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
        let armable = st
            .agent_session
            .as_ref()
            .is_some_and(|s| s.rich && s.mid_turn());
        if !armable || st.agent_clock.interrupt_pending {
            return;
        }
        st.agent_clock.interrupt_pending = true;
        st.agent_clock.generation
    };
    let waiter = state.clone();
    let spawned = std::thread::Builder::new()
        .name("anterm-agent-interrupt".into())
        .spawn(move || {
            std::thread::sleep(INTERRUPT_SETTLE);
            let mut st = waiter.lock().unwrap_or_else(|e| e.into_inner());
            settle_interrupt(&mut st, generation);
        });
    if spawned.is_err() {
        state.lock().unwrap_or_else(|e| e.into_inner()).agent_clock.interrupt_pending = false;
    }
}

pub(super) fn settle_interrupt(st: &mut PaneState, armed_at: u64) {
    st.agent_clock.interrupt_pending = false;
    if st.agent_clock.generation != armed_at || !st.alive {
        return;
    }
    let Some(sess) = st.agent_session.as_mut().filter(|s| s.mid_turn()) else {
        return;
    };
    let before = sess.status;
    sess.assume_interrupted();
    sess.note_transition(Some(before), unix_ms());
    notify(st, DaemonMsg::AgentStatus(st.agent_session.clone()));
}

/// Give up on a turn that has said nothing for [`AGENT_STALE_AFTER`]. Run by
/// the daemon's periodic sweep, since a silent pane produces nothing that
/// would otherwise wake anything up to notice.
pub(super) fn expire_stale_agent(st: &mut PaneState, now: std::time::Instant) {
    let working = st
        .agent_session
        .as_ref()
        .is_some_and(|s| s.status == crate::cli_agent::AgentStatus::Working);
    if !working {
        return;
    }
    let since = *st.agent_clock.last_event.get_or_insert(now);
    if now.saturating_duration_since(since) < AGENT_STALE_AFTER {
        return;
    }
    if let Some(sess) = st.agent_session.as_mut() {
        let before = sess.status;
        sess.assume_stale();
        sess.note_transition(Some(before), unix_ms());
    }
    notify(st, DaemonMsg::AgentStatus(st.agent_session.clone()));
}

pub(super) fn apply_agent_signals(
    st: &mut PaneState,
    events: Vec<crate::cli_agent::AgentEvent>,
    notification: Option<String>,
) {
    use crate::cli_agent::{AgentSessionState, AgentStatus};

    if events.is_empty() && notification.is_none() {
        return;
    }
    let before = st.agent_session.clone();
    let now_ms = unix_ms();

    for event in &events {
        // A hook names the agent that emitted it. Once this pane has a
        // foreground agent, a different one's session id must not land here:
        // omp forking with a Claude UUID is "session not found".
        if let (Some(current), Some(from)) = (st.agent, event.agent)
            && current != from
        {
            continue;
        }
        if st.agent.is_none() && event.agent.is_some() {
            st.agent = event.agent;
            notify(st, DaemonMsg::Agent(st.agent));
        }
        st.agent_clock.generation = st.agent_clock.generation.wrapping_add(1);
        st.agent_clock.last_event = Some(std::time::Instant::now());
        // Stamped per event: a status that moves and comes back within one
        // batch (Done, a new prompt, Done again) began a fresh spell.
        let sess = st
            .agent_session
            .get_or_insert_with(AgentSessionState::default);
        let was = sess.status;
        sess.apply_event(event);
        sess.note_transition(Some(was), now_ms);
    }

    if let Some(body) = notification
        && st.agent.is_some()
        && !st.agent_session.as_ref().is_some_and(|s| s.rich)
    {
        let sess = st
            .agent_session
            .get_or_insert_with(AgentSessionState::default);
        sess.status = AgentStatus::Waiting;
        sess.message = Some(body);
    }

    if let (Some(sess), Some(argv)) = (&mut st.agent_session, &st.agent_argv)
        && sess.launch_argv.is_none()
    {
        sess.launch_argv = Some(argv.clone());
    }

    // The notification-only path, and a session this batch created.
    if let Some(sess) = st.agent_session.as_mut() {
        sess.note_transition(before.as_ref().map(|b| b.status), now_ms);
    }

    if st.agent_session != before {
        notify(st, DaemonMsg::AgentStatus(st.agent_session.clone()));
    }
}

/// Wall-clock milliseconds, for the [`AgentSessionState::since_ms`] stamp a
/// client reads back as an age. Not an `Instant`: the stamp crosses the
/// socket and survives a handoff to the next daemon.
///
/// [`AgentSessionState::since_ms`]: crate::cli_agent::AgentSessionState::since_ms
fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub(super) fn apply_probed_cwd(st: &mut PaneState, probed: Option<PathBuf>) {
    let Some(probed) = probed else {
        return;
    };
    if st.remote.is_some() {
        return;
    }
    if st.cwd.as_deref().is_some_and(|cur| same_dir(cur, &probed)) {
        return;
    }
    notify(st, DaemonMsg::Cwd(probed.clone()));
    st.cwd = Some(probed);
}

/// Whether a report from a process outside the pane it names still belongs
/// to it, because the agent it is about runs its hooks there by design.
///
/// Prime Agent's TUI hands each session to a background daemon — started by
/// the first session, detached, and outliving it — which runs that session's
/// extensions, anterm's bridge among them, with the environment of the TUI that
/// asked for it. The report's `$ANTERM_PANE` is therefore right while the
/// process is never under the pane's shell. Taking it on the pane's word is
/// safe as long as that pane is running Prime Agent itself.
pub(super) fn owns_detached_report(
    reported: Option<crate::cli_agent::CLIAgent>,
    running: Option<crate::cli_agent::CLIAgent>,
) -> bool {
    use crate::cli_agent::CLIAgent;
    reported == Some(CLIAgent::PrimeAgent) && running == reported
}

/// A pane as a Codex hook report sees it: whether Codex runs in it, where,
/// and which session it last reported.
#[derive(Debug, Clone)]
pub struct CodexCandidate {
    pub pane: u64,
    pub runs_codex: bool,
    pub cwd: Option<PathBuf>,
    pub session: Option<String>,
}

/// The pane a Codex hook's report belongs to.
///
/// Codex runs its hooks in its app-server: one background process that every
/// Codex session on the machine talks to, started by whichever session came
/// first and outliving it. A hook therefore carries that first session's
/// environment — its `$ANTERM_PANE` — and runs under that pane's shell only for
/// as long as that session lasts, so neither says which pane the report is
/// about. The report does: its session, then its directory. A pane that has
/// already reported the session owns it; otherwise it is the one pane running
/// Codex in that directory, or among every Codex pane when none runs there
/// (`codex -C` leaves the shell elsewhere). Of several, a session `/new`
/// started (`cleared`) goes to one that had a session before and any other to
/// one yet to report, and the named pane breaks a tie that still stands.
/// `None` is no pane, or no telling which.
///
/// jcode is built the same way — its sessions share one background server
/// that runs every session's hooks with the environment of whichever client
/// started it — so its reports take the same road, with `runs_codex` meaning
/// "runs jcode".
pub fn codex_report_target(
    panes: &[CodexCandidate],
    named: u64,
    session: Option<&str>,
    cwd: Option<&Path>,
    cleared: bool,
) -> Option<u64> {
    let codex = || panes.iter().filter(|p| p.runs_codex);
    if let Some(session) = session
        && let Some(owner) = codex().find(|p| p.session.as_deref() == Some(session))
    {
        return Some(owner.pane);
    }
    let here: Vec<&CodexCandidate> = codex()
        .filter(|p| {
            cwd.zip(p.cwd.as_deref())
                .is_some_and(|(cwd, c)| same_dir(c, cwd))
        })
        .collect();
    let pool = if here.is_empty() {
        codex().collect()
    } else {
        here
    };
    let likely: Vec<&CodexCandidate> = pool
        .iter()
        .copied()
        .filter(|p| p.session.is_some() == cleared)
        .collect();
    let pool = if likely.is_empty() { pool } else { likely };
    match pool.as_slice() {
        [only] => Some(only.pane),
        [] => None,
        several => several.iter().find(|p| p.pane == named).map(|p| p.pane),
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

/// Take a batch of prompt marks out of the local line editor's reach.
///
/// Called when a foreground program owns the pty, which means the marks are
/// being relayed — an `ssh` passing the far shell's prompt through, a nested
/// shell drawing its own. The local editor must not engage on those, so
/// `at_prompt` is cleared across the whole batch.
///
/// [`ShellState::mark_at_prompt`] is deliberately left standing: it is the
/// same marks read for a different question, and on a remote pane it is the
/// only thing this machine knows about the far shell (#840). Because of it,
/// dedup has to compare what is actually emitted rather than the whole struct
/// — two marks that used to collapse into one `Prompt` message must still
/// collapse when only their unsuppressed twin tells them apart, and the
/// survivor has to carry the *newest* twin: a whole turn can arrive in one read
/// (`D`, the prompt's `A`/`B`, then the next command's `C`), and dropping the
/// last entry's reading would hand the pane back a prompt it has already left.
pub(super) fn suppress_relayed_prompt_marks(shell: &mut Vec<ShellState>) {
    for s in shell.iter_mut() {
        s.at_prompt = false;
    }
    shell.dedup_by(|a, b| {
        let same = a.active == b.active
            && a.at_prompt == b.at_prompt
            && a.last_exit_code == b.last_exit_code
            && a.command == b.command;
        // `dedup_by` keeps `b`, the earlier of the pair, and drops `a`. Move
        // the reading over first so the collapse costs a `Prompt` message and
        // nothing else.
        if same {
            b.mark_at_prompt = a.mark_at_prompt;
        }
        same
    });
}

pub(super) fn apply_remote_context(st: &mut PaneState, remote: Option<RemoteContext>) {
    if st.remote == remote {
        return;
    }
    st.cwd = None;
    // A new far side has to prove its own shell integration. The mark that is
    // standing right now belongs to whatever the pane was before this hop —
    // the near shell's "I started `ssh`", or the previous host's prompt.
    st.remote_prompt_seen = false;
    notify(st, DaemonMsg::RemoteContext(remote.clone()));
    st.remote = remote;
}

/// Record that this read carried a prompt mark while the pane was remote.
///
/// Called *after* [`apply_remote_context`], not with the signals: the read that
/// first sees a pane as remote is very often the one carrying the far shell's
/// opening prompt, and latching before the hop was applied would throw exactly
/// that mark away — the reset above would clear it a line later. Ordering it
/// here means the far side's first prompt counts, which is what makes a later
/// "not at a prompt" mean "the remote is busy" rather than "we never heard
/// from it" (#840).
pub(super) fn latch_remote_prompt(st: &mut PaneState, saw_prompt_mark: bool) {
    st.remote_prompt_seen |= saw_prompt_mark && st.remote.is_some();
}

pub(super) fn apply_agent(
    st: &mut PaneState,
    detected: Option<(crate::cli_agent::CLIAgent, Vec<String>)>,
) {
    let (agent, argv) = match detected {
        Some((agent, argv)) => (Some(agent), Some(argv)),
        None => (None, None),
    };
    if st.agent == agent {
        stamp_launch_argv(st, argv);
        return;
    }
    // The session belongs to whoever was in the foreground. Switching from
    // Claude to omp (or back to the shell) must not keep the previous id.
    if st.agent_session.is_some() {
        st.agent_session = None;
        st.agent_clock.generation = st.agent_clock.generation.wrapping_add(1);
        st.agent_clock.last_event = None;
        notify(st, DaemonMsg::AgentStatus(None));
    }
    if agent.is_none() {
        st.agent_argv = None;
    }
    notify(st, DaemonMsg::Agent(agent));
    st.agent = agent;
    stamp_launch_argv(st, argv);
}

fn stamp_launch_argv(st: &mut PaneState, argv: Option<Vec<String>>) {
    let Some(argv) = argv else { return };
    if argv.is_empty() {
        return;
    }
    if st.agent_argv.as_ref() == Some(&argv) {
        return;
    }
    st.agent_argv = Some(argv.clone());
    if let Some(sess) = &mut st.agent_session
        && sess.launch_argv.as_ref() != Some(&argv)
    {
        sess.launch_argv = Some(argv);
        notify(st, DaemonMsg::AgentStatus(st.agent_session.clone()));
    }
}

pub(super) fn foreground_command_running(
    master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
    shell_pid: Option<u32>,
) -> bool {
    is_foreground_command(pty_foreground_pgid(master), shell_pid)
}

#[cfg(unix)]
pub(super) fn pty_foreground_pgid(master: &Mutex<Option<Box<dyn MasterPty + Send>>>) -> Option<i32> {
    master
        .lock()
        .ok()
        .and_then(|m| m.as_ref().and_then(|m| m.process_group_leader()))
}

#[cfg(not(unix))]
pub(super) fn pty_foreground_pgid(_master: &Mutex<Option<Box<dyn MasterPty + Send>>>) -> Option<i32> {
    None
}

pub(super) fn is_foreground_command(fg_pgid: Option<i32>, shell_pid: Option<u32>) -> bool {
    match (fg_pgid, shell_pid) {
        (Some(pg), Some(shell)) if pg > 0 => pg as u32 != shell,
        _ => false,
    }
}

#[cfg(target_os = "macos")]
pub(super) fn foreground_cwd(
    master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
    shell_pid: Option<u32>,
) -> Option<PathBuf> {
    use std::ffi::CStr;

    let read_cwd = |pid: i32| -> Option<PathBuf> {
        if pid <= 0 {
            return None;
        }
        let mut vinfo: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
        let ret = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                &mut vinfo as *mut _ as *mut libc::c_void,
                size,
            )
        };
        if ret != size {
            return None;
        }
        let s = unsafe { CStr::from_ptr(vinfo.pvi_cdir.vip_path.as_ptr() as *const libc::c_char) }
            .to_str()
            .ok()?;
        if s.is_empty() {
            None
        } else {
            Some(PathBuf::from(s))
        }
    };

    pty_foreground_pgid(master)
        .and_then(read_cwd)
        .or_else(|| read_cwd(shell_pid.map(|p| p as i32).unwrap_or(0)))
}

#[cfg(target_os = "linux")]
pub(super) fn foreground_cwd(
    master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
    shell_pid: Option<u32>,
) -> Option<PathBuf> {
    let read_cwd = |pid: i32| -> Option<PathBuf> {
        if pid <= 0 {
            return None;
        }
        let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
        cwd.is_dir().then_some(cwd)
    };
    pty_foreground_pgid(master)
        .and_then(read_cwd)
        .or_else(|| read_cwd(shell_pid.map(|p| p as i32).unwrap_or(0)))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn foreground_cwd(
    _master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
    _shell_pid: Option<u32>,
) -> Option<PathBuf> {
    None
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn foreground_remote_context(
    master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
) -> Option<RemoteContext> {
    let pid = master
        .lock()
        .ok()
        .and_then(|m| m.as_ref().and_then(|m| m.process_group_leader()))?;
    let argv = crate::daemon::remote::foreground_argv(pid)?;
    crate::daemon::remote::parse_ssh_invocation(&argv).map(|inv| inv.context)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn foreground_remote_context(
    _master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
) -> Option<RemoteContext> {
    None
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn foreground_agent(
    master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
) -> Option<Option<(crate::cli_agent::CLIAgent, Vec<String>)>> {
    let detect = || {
        let pid = master
            .lock()
            .ok()
            .and_then(|m| m.as_ref().and_then(|m| m.process_group_leader()))?;
        let argv = crate::daemon::remote::foreground_argv(pid)?;
        let agent = crate::cli_agent::CLIAgent::detect_from_argv_with(
            &argv,
            &crate::config::agent_detection_aliases(),
        )?;
        Some((agent, argv))
    };
    Some(detect())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn foreground_agent(
    _master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
) -> Option<Option<(crate::cli_agent::CLIAgent, Vec<String>)>> {
    None
}

#[derive(Default, Clone, PartialEq, Eq)]
pub(super) struct ShellState {
    pub(super) active: bool,
    pub(super) at_prompt: bool,
    /// `at_prompt` as the marks themselves read it, kept out of the
    /// suppression the reader applies when a foreground program is running.
    ///
    /// Two different questions share one pair of marks. "Should the local line
    /// editor engage?" must say no while `ssh` owns the pty, whoever drew the
    /// prompt — that is `at_prompt`. "Is anything running in this pane?" wants
    /// the opposite: the prompt an `ssh` is relaying belongs to the shell that
    /// is actually driving the pane, and it is the only thing this side can
    /// read about the far one. Keeping both means neither answer has to be
    /// derived from the other's.
    pub(super) mark_at_prompt: bool,
    pub(super) last_exit_code: Option<i32>,
    pub(super) command: Option<String>,
}

#[derive(Default)]
pub(super) struct SniffSignals {
    pub(super) cwd: Option<PathBuf>,
    /// The last title the pane set in this read, already capped. `Some("")` is
    /// a reset — an empty OSC 0/2 clears the title rather than setting a blank
    /// one, the same way the GUI's terminal treats it.
    pub(super) title: Option<String>,
    pub(super) shell: Vec<ShellState>,
    pub(super) agent_events: Vec<crate::cli_agent::AgentEvent>,
    pub(super) notification: Option<String>,
}

pub(super) struct OscSniffer {
    tok: OscTokenizer,
    shell: ShellState,
    /// Whether the title the pane is showing still belongs to something that
    /// is running — see [`crate::osc::TitleLifetime`] (#889).
    title_life: crate::osc::TitleLifetime,
    notes: crate::osc::Notifications,
}

impl OscSniffer {
    pub(super) fn new() -> Self {
        Self {
            tok: OscTokenizer::new(&[b"0", b"2", b"7", b"133", b"9", b"99", b"777"]),
            shell: ShellState::default(),
            title_life: crate::osc::TitleLifetime::default(),
            notes: crate::osc::Notifications::default(),
        }
    }

    pub(super) fn feed(&mut self, bytes: &[u8]) -> SniffSignals {
        let mut signals = SniffSignals::default();
        let shell = &mut self.shell;
        let title_life = &mut self.title_life;
        let notes = &mut self.notes;
        self.tok.feed(bytes, |payload| {
            // In stream order, so that a shell which re-titles itself right
            // after the `D` mark gets the last word over the retirement.
            if title_life.saw(payload) == crate::osc::TitleEffect::Retire {
                signals.title = Some(String::new());
            }
            if let Some(path) = parse_osc7(payload) {
                signals.cwd = Some(path);
            } else if let Some(title) = parse_osc_title(payload) {
                signals.title = Some(title);
            } else if let Some(rest) = payload.strip_prefix(b"133;") {
                if handle_osc133(shell, rest) {
                    match signals.shell.last_mut() {
                        Some(last) if last.at_prompt == shell.at_prompt => {
                            *last = shell.clone();
                        }
                        _ => signals.shell.push(shell.clone()),
                    }
                }
            } else if let Some(event) = crate::cli_agent::parse_agent_event(payload) {
                signals.agent_events.push(event);
            } else if let Some((title, body)) = notes.parse(payload) {
                if title.as_deref() != Some(crate::cli_agent::AGENT_EVENT_SENTINEL) {
                    signals.notification = Some(body);
                }
            }
        });
        signals
    }
}

fn handle_osc133(shell: &mut ShellState, rest: &[u8]) -> bool {
    shell.active = true;
    match rest.first() {
        Some(b'A') | Some(b'B') => shell.at_prompt = true,
        Some(b'C') => {
            shell.at_prompt = false;
            shell.command = rest
                .strip_prefix(b"C;")
                .map(|c| String::from_utf8_lossy(&percent_decode(c)).into_owned())
                .filter(|s| !s.trim().is_empty());
        }
        Some(b'D') => {
            shell.at_prompt = true;
            shell.command = None;
            shell.last_exit_code = rest
                .strip_prefix(b"D;")
                .and_then(|c| std::str::from_utf8(c).ok())
                .and_then(|s| s.trim().parse::<i32>().ok());
        }
        _ => return false,
    }
    // The unsuppressed twin, set here so every arm gets it and no future arm
    // can forget to. Suppression happens later, on the reader thread, and only
    // ever touches `at_prompt`.
    shell.mark_at_prompt = shell.at_prompt;
    true
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    let s = String::from_utf8_lossy(bytes);
    PathBuf::from(strip_uri_drive_slash(s.as_ref()))
}

#[cfg_attr(unix, allow(dead_code))]
pub(super) fn strip_uri_drive_slash(path: &str) -> &str {
    let b = path.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        &path[1..]
    } else {
        path
    }
}

pub(crate) fn parse_osc7(payload: &[u8]) -> Option<PathBuf> {
    let rest = payload.strip_prefix(b"7;")?;
    let path_bytes: &[u8] = if let Some(after) = rest.strip_prefix(b"file://") {
        let idx = after.iter().position(|&c| c == b'/')?;
        &after[idx..]
    } else if rest.first() == Some(&b'/') {
        rest
    } else {
        return None;
    };
    let decoded = percent_decode(path_bytes);
    if decoded.is_empty() {
        return None;
    }
    Some(path_from_bytes(&decoded))
}

/// Longest title the tree will keep. The window that owns the pane shows about
/// forty columns of it; the rest is only ever paid for — in `machine.json`, and
/// in a `PaneFacts` delta to every attached client — so it is cut here rather
/// than at each place that renders one.
pub(super) const MAX_OSC_TITLE: usize = 256;

/// The title from an OSC 0 (icon *and* window) or OSC 2 (window) payload,
/// capped. An empty payload comes back as `Some("")`: that is a reset, which
/// the caller has to tell apart from "no title in this read".
///
/// OSC 1 is deliberately not read. It sets the icon name alone, which the GUI's
/// terminal ignores — and the point of recording a title at all is to agree
/// with what the GUI shows.
pub(crate) fn parse_osc_title(payload: &[u8]) -> Option<String> {
    let rest = payload
        .strip_prefix(b"0;")
        .or_else(|| payload.strip_prefix(b"2;"))?;
    let title = String::from_utf8_lossy(rest);
    let title = title.trim();
    Some(match title.chars().count() > MAX_OSC_TITLE {
        true => title.chars().take(MAX_OSC_TITLE).collect(),
        false => title.to_string(),
    })
}

pub(super) fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' && i + 2 < input.len() {
            if let (Some(h), Some(l)) = (hex_val(input[i + 1]), hex_val(input[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

pub(super) fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
