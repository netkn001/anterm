use serde::{Deserialize, Serialize};

use super::CLIAgent;

pub const AGENT_EVENT_SENTINEL: &str = "anterm://cli-agent";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentStatus {
    #[default]
    Idle,
    Working,
    Waiting,
    Done,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionState {
    #[serde(default = "AgentSessionState::default_status")]
    pub status: AgentStatus,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub launch_argv: Option<Vec<String>>,
    #[serde(default)]
    pub rich: bool,
    #[serde(default)]
    pub cwd: Option<std::path::PathBuf>,
    #[serde(default)]
    pub activity: u64,
    /// How many turns this session has finished: bumped each time it settles
    /// into `Done`. The status alone cannot tell a client that attaches to a
    /// `Done` pane whether that is the turn it already showed the reader or a
    /// later one that finished while nobody was watching (#870).
    #[serde(default)]
    pub turns: u64,
    /// The status was concluded by anterm, not reported by the agent: a turn
    /// the user interrupted ([`Self::assume_interrupted`]) or one that went
    /// quiet for too long ([`Self::assume_stale`]). Cleared by the next real
    /// event, and a tool finishing after it proves the guess wrong and puts
    /// the turn back on [`AgentStatus::Working`].
    #[serde(default)]
    pub inferred: bool,
    /// The agent's settings as its hooks last described them. Only Claude
    /// Code's hooks carry these today.
    #[serde(default)]
    pub readout: AgentReadout,
    /// When `status` last changed, in milliseconds since the Unix epoch by
    /// the daemon's clock ([`Self::note_transition`]). What "waiting 3m"
    /// is counted from, and how the oldest of several waiting agents is
    /// told apart from the newest. `None` from a daemon that predates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<u64>,
}

/// What an agent says about how it is set up — what the message composer's
/// toolbar shows. Each field is only ever what the agent reported; a field
/// it has not reported stays `None` rather than being guessed at.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentReadout {
    /// The agent's own word for it: `default`, `acceptEdits`, `plan`,
    /// `bypassPermissions`, …
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// The model id, as the agent's transcript records it.
    #[serde(default)]
    pub model: Option<String>,
    /// The reasoning effort level (`low`, `medium`, `high`, …); empty when
    /// the agent runs a model that takes none.
    #[serde(default)]
    pub effort: Option<String>,
}

impl AgentReadout {
    /// Take what `newer` reports and keep the rest: one event rarely carries
    /// every field, and a missing one says nothing about its value.
    pub fn merge(&mut self, newer: &AgentReadout) {
        fn take<T: Clone>(slot: &mut Option<T>, newer: &Option<T>) {
            if newer.is_some() {
                slot.clone_from(newer);
            }
        }
        take(&mut self.permission_mode, &newer.permission_mode);
        take(&mut self.model, &newer.model);
        take(&mut self.effort, &newer.effort);
    }
}

impl AgentStatus {
    pub fn dot_rgb(self) -> Option<u32> {
        match self {
            AgentStatus::Idle => None,
            AgentStatus::Working => Some(0x3B82F6),
            AgentStatus::Waiting => Some(0xF59E0B),
            AgentStatus::Done => Some(0x22C55E),
        }
    }
}

impl AgentSessionState {
    fn default_status() -> AgentStatus {
        AgentStatus::Idle
    }

    /// Whether a turn is in flight: the only statuses a user interrupt or a
    /// lost `Stop` can leave behind.
    pub fn mid_turn(&self) -> bool {
        matches!(self.status, AgentStatus::Working | AgentStatus::Waiting)
    }

    /// End the turn the user just interrupted.
    ///
    /// Most agents fire no hook when <kbd>Esc</kbd> or <kbd>Ctrl+C</kbd>
    /// cancels a turn — Claude's `Stop` explicitly skips user interrupts — so
    /// without this the pane stays on working until the next prompt, and a
    /// close asks about a turn that is long over. The caller has already
    /// waited for a real event to say otherwise.
    pub fn assume_interrupted(&mut self) {
        if !self.mid_turn() {
            return;
        }
        self.turns = self.turns.wrapping_add(1);
        self.status = AgentStatus::Done;
        self.message = None;
        self.inferred = true;
    }

    /// Give up on a turn that has gone silent for longer than any real one
    /// stays quiet. Idle rather than Done: nothing says it finished, only
    /// that nothing says it is still going.
    pub fn assume_stale(&mut self) {
        if self.status != AgentStatus::Working {
            return;
        }
        self.status = AgentStatus::Idle;
        self.message = None;
        self.inferred = true;
    }

    /// Stamp [`Self::since_ms`] when the status is no longer the one it was
    /// `before` the caller changed anything. Kept out of the transitions
    /// themselves so they stay free of the clock; the daemon, the one place
    /// a status moves, calls this after each.
    pub fn note_transition(&mut self, before: Option<AgentStatus>, now_ms: u64) {
        if before != Some(self.status) {
            self.since_ms = Some(now_ms);
        }
    }

    /// How long the session has held its status, in seconds, by `now_ms`.
    /// Never negative: a remote daemon's clock can run ahead of this one.
    pub fn status_age_secs(&self, now_ms: u64) -> Option<u64> {
        self.since_ms
            .map(|since| now_ms.saturating_sub(since) / 1000)
    }

    pub fn apply_event(&mut self, ev: &AgentEvent) {
        // A status set before the first hook came from a desktop notification
        // — Crush's "turn completed", say — which says the pane wants a look,
        // not that a question is open. Taken for one, the first tool report
        // turned it into a turn running, and nothing would ever end it.
        if !self.rich && self.status == AgentStatus::Waiting {
            self.status = AgentStatus::Idle;
            self.message = None;
        }
        self.rich = true;
        let guessed = std::mem::take(&mut self.inferred);
        if let Some(id) = &ev.session_id {
            self.session_id = Some(id.clone());
        }
        if let Some(cwd) = &ev.cwd {
            self.cwd = Some(cwd.clone());
        }
        self.readout.merge(&ev.readout);
        match ev.kind {
            AgentEventKind::SessionStart => {
                self.status = AgentStatus::Idle;
                self.message = None;
            }
            AgentEventKind::PromptSubmit => {
                self.status = AgentStatus::Working;
                self.message = None;
            }
            AgentEventKind::PermissionRequest | AgentEventKind::QuestionAsked => {
                self.status = AgentStatus::Waiting;
                self.message = ev.message.clone();
            }
            AgentEventKind::Notification => {
                if self.status == AgentStatus::Working {
                    self.status = AgentStatus::Waiting;
                    self.message = ev.message.clone();
                }
            }
            AgentEventKind::ToolComplete => {
                self.activity = self.activity.wrapping_add(1);
                if self.status == AgentStatus::Waiting || guessed {
                    self.status = AgentStatus::Working;
                    self.message = None;
                }
            }
            AgentEventKind::Stop => {
                if self.status != AgentStatus::Done {
                    self.turns = self.turns.wrapping_add(1);
                }
                self.status = AgentStatus::Done;
                self.message = ev.message.clone();
            }
            AgentEventKind::SessionEnd => {
                self.status = AgentStatus::Idle;
                self.message = None;
                self.cwd = None;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentEventKind {
    SessionStart,
    PromptSubmit,
    PermissionRequest,
    QuestionAsked,
    ToolComplete,
    Notification,
    Stop,
    SessionEnd,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEvent {
    pub agent: Option<CLIAgent>,
    pub kind: AgentEventKind,
    pub session_id: Option<String>,
    pub message: Option<String>,
    pub cwd: Option<std::path::PathBuf>,
    /// What started the session, on a session start that says: `startup`,
    /// `resume`, `clear` (Codex's `/new`), `compact`.
    pub source: Option<String>,
    pub readout: AgentReadout,
}

pub fn parse_agent_event(payload: &[u8]) -> Option<AgentEvent> {
    let rest = payload.strip_prefix(b"777;notify;")?;
    let rest = rest.strip_prefix(AGENT_EVENT_SENTINEL.as_bytes())?;
    parse_agent_event_body(rest.strip_prefix(b";")?)
}

/// The JSON body of an agent event, without the OSC around it — what a hook
/// hands the daemon over its socket.
pub fn parse_agent_event_body(json: &[u8]) -> Option<AgentEvent> {
    #[derive(Deserialize)]
    struct Wire {
        #[serde(default)]
        #[allow(dead_code)]
        v: u32,
        #[serde(default)]
        agent: Option<String>,
        event: String,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        source: Option<String>,
        #[serde(flatten)]
        readout: AgentReadout,
    }

    let w: Wire = serde_json::from_slice(json).ok()?;
    let kind = serde_json::from_value::<AgentEventKind>(serde_json::Value::String(w.event)).ok()?;
    let nonempty = |s: Option<String>| s.filter(|s| !s.trim().is_empty());
    Some(AgentEvent {
        agent: w.agent.as_deref().and_then(CLIAgent::from_slug),
        kind,
        session_id: nonempty(w.session_id),
        message: nonempty(w.message),
        cwd: nonempty(w.cwd).map(std::path::PathBuf::from),
        source: nonempty(w.source),
        readout: w.readout,
    })
}
