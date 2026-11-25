use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cli_agent::CLIAgent;
use crate::daemon::protocol::{NativeSshSpec, ShellSpec};
use crate::tag::{SidebarTags, TagId};

pub const MAX_PANES: usize = 16 * 1024;

/// How long a closed tab stays reopenable (#1021).
///
/// Long enough to cover "closed it before lunch, want it back after", short
/// enough that a workspace's recently-closed list does not turn into an archive
/// of terminal output nobody asked to keep: each entry pins its panes' last
/// screens on disk, and those go when the entry does.
pub const CLOSED_TAB_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// How many closed tabs a workspace remembers, newest kept. The same bound the
/// window's in-memory list has always had, so a workspace that moves from one
/// to the other reopens exactly as far back as it did before.
pub const MAX_CLOSED_TABS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TabId(uuid::Uuid);

impl TabId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for TabId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    A,
    B,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    #[serde(default)]
    pub tabs: Vec<Tab>,
    #[serde(default)]
    pub active_tab: Option<TabId>,
    #[serde(default, skip_serializing_if = "SidebarTags::is_empty")]
    pub tags: SidebarTags,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closed: Vec<ClosedTab>,
    #[serde(default)]
    pub panes: Vec<PaneRecord>,
}

/// A closed tab kept by the machine for reopening.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClosedTab {
    /// The tab as it stood when it closed: its id, name, tags and layout. The
    /// pane ids in `root` are dead by the time anyone reads this; they are the
    /// keys a reopen asks the daemon to restore each pane's screen from.
    pub tab: Tab,
    /// When it closed, in seconds since the Unix epoch.
    #[serde(default)]
    pub closed_at: u64,
}

/// What [`MachineStore::tab_reopen`] hands back: the closed tab and the
/// records of the panes it held, which are what its layout is rebuilt from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReopenedTab {
    pub tab: Tab,
    #[serde(default)]
    pub closed_at: u64,
    #[serde(default)]
    pub panes: Vec<PaneRecord>,
}

/// What closing a tab into the recently-closed list leaves for the caller to
/// do to the processes — see [`MachineStore::tab_close_remembered`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Remembered {
    /// The tab's panes. Their processes are to be stopped with their screens
    /// kept, because the entry that now names them restores from those.
    pub stopped: Vec<u64>,
    /// Panes the closing client held that the tree never recorded in the tab.
    /// Nothing can bring them back, so they end the way a close always ended
    /// a pane: screen and all.
    pub strays: Vec<u64>,
    /// Panes of entries the list outgrew, whose screens nothing will ask for
    /// again.
    pub dropped: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    #[serde(default)]
    pub id: TabId,
    #[serde(default)]
    pub name: Option<String>,
    /// User-defined tags attached to this tab.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<TagId>,
    pub root: PaneNode,
    /// The tab was put to sleep: its panes were stopped to give their memory
    /// back, and it stays in the workspace to be woken later (#762).
    ///
    /// The panes keep their ids in `root` and their records in
    /// [`Machine::panes`] — cwd, shell, agent session — because waking is a
    /// restore of exactly those panes, through the same path a reboot takes.
    /// That is also what keeps the sweeps off them: a pane the tree still
    /// names is one whose stored screen and history are kept.
    ///
    /// Left out of the document while false, so a tree written by this build
    /// reads the same to one that predates the field.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hibernated: bool,
}

impl Tab {
    pub fn leaf(pane: u64) -> Tab {
        Tab {
            id: TabId::new(),
            name: None,
            tags: Vec::new(),
            root: PaneNode::Leaf { pane },
            hibernated: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PaneNode {
    Leaf {
        pane: u64,
    },
    Split {
        axis: Axis,
        #[serde(default = "default_ratio")]
        ratio: f32,
        a: Box<PaneNode>,
        b: Box<PaneNode>,
    },
}

fn default_ratio() -> f32 {
    0.5
}

impl PaneNode {
    pub fn pane_ids(&self) -> Vec<u64> {
        let mut out = Vec::new();
        self.collect_panes(&mut out);
        out
    }

    fn collect_panes(&self, out: &mut Vec<u64>) {
        match self {
            PaneNode::Leaf { pane } => out.push(*pane),
            PaneNode::Split { a, b, .. } => {
                a.collect_panes(out);
                b.collect_panes(out);
            }
        }
    }

    pub fn contains(&self, pane: u64) -> bool {
        match self {
            PaneNode::Leaf { pane: p } => *p == pane,
            PaneNode::Split { a, b, .. } => a.contains(pane) || b.contains(pane),
        }
    }

    pub fn descend_mut(&mut self, path: &[Side]) -> Option<&mut PaneNode> {
        match path.split_first() {
            None => Some(self),
            Some((side, rest)) => match self {
                PaneNode::Leaf { .. } => None,
                PaneNode::Split { a, b, .. } => match side {
                    Side::A => a.descend_mut(rest),
                    Side::B => b.descend_mut(rest),
                },
            },
        }
    }

    pub fn split_leaf(&mut self, pane: u64, new: u64, axis: Axis, ratio: f32, first: bool) -> bool {
        match self {
            PaneNode::Leaf { pane: p } if *p == pane => {
                let old = PaneNode::Leaf { pane };
                let added = PaneNode::Leaf { pane: new };
                let (a, b) = if first { (added, old) } else { (old, added) };
                *self = PaneNode::Split {
                    axis,
                    ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                };
                true
            }
            PaneNode::Leaf { .. } => false,
            PaneNode::Split { a, b, .. } => {
                a.split_leaf(pane, new, axis, ratio, first)
                    || b.split_leaf(pane, new, axis, ratio, first)
            }
        }
    }

    pub fn remove_leaf(&mut self, pane: u64) -> Option<bool> {
        match self {
            PaneNode::Leaf { pane: p } => {
                if *p == pane {
                    None
                } else {
                    Some(false)
                }
            }
            PaneNode::Split { a, b, .. } => {
                if matches!(&**a, PaneNode::Leaf { pane: p } if *p == pane) {
                    *self = (**b).clone();
                    return Some(true);
                }
                if matches!(&**b, PaneNode::Leaf { pane: p } if *p == pane) {
                    *self = (**a).clone();
                    return Some(true);
                }
                match a.remove_leaf(pane) {
                    Some(true) => Some(true),
                    Some(false) => b.remove_leaf(pane),
                    None => Some(false),
                }
            }
        }
    }

    pub fn replace_leaf(&mut self, old: u64, new: u64) -> bool {
        match self {
            PaneNode::Leaf { pane } if *pane == old => {
                *pane = new;
                true
            }
            PaneNode::Leaf { .. } => false,
            PaneNode::Split { a, b, .. } => a.replace_leaf(old, new) || b.replace_leaf(old, new),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneRecord {
    pub id: u64,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub title: String,
    /// The title the terminal itself reports (OSC 0/2): a shell writes
    /// `user@host:~/dir` from its integration, an agent overwrites it with what
    /// it is working on. Distinct from `title` above, which is the foreground
    /// process name.
    ///
    /// This is the name the window that owns the pane puts on its tab, so it is
    /// also what anyone else has to read to agree with that window — the
    /// switcher listing a workspace it does not own, `anterm tab ls` across a
    /// socket. `None` until the pane emits one; a pane that resets its title
    /// clears it back.
    #[serde(default)]
    pub osc_title: Option<String>,
    #[serde(default)]
    pub ssh_spec: Option<Box<NativeSshSpec>>,
    #[serde(default)]
    pub agent: Option<AgentFacts>,
    /// What the pane is actually running, resolved: the spawn's override if it
    /// had one, otherwise the shell the config named at the time.
    ///
    /// Without it a pane rebuilt from this tree comes back on whatever the
    /// default shell is now, so a daemon restart silently turns a bash pane
    /// into a PowerShell one. The rest of the record describes where a pane is
    /// and what is running in it; this is the part that says what it *is*.
    #[serde(default)]
    pub shell: Option<ShellSpec>,
    #[serde(default)]
    pub live: bool,
}

impl PaneRecord {
    pub fn new(id: u64) -> PaneRecord {
        PaneRecord {
            id,
            cwd: None,
            title: String::new(),
            osc_title: None,
            ssh_spec: None,
            agent: None,
            shell: None,
            live: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentFacts {
    pub agent: CLIAgent,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub launch_argv: Option<Vec<String>>,
    #[serde(default)]
    pub status: Option<crate::cli_agent::AgentStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneSeed {
    pub pane: u64,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub ssh_spec: Option<Box<NativeSshSpec>>,
    #[serde(default)]
    pub agent: Option<AgentFacts>,
    /// See [`PaneRecord::shell`]. Carried here too so a pane that reaches the
    /// tree as a seed — a split, a `anterm` CLI call — names its shell from the
    /// start rather than only once the daemon has observed it.
    #[serde(default)]
    pub shell: Option<ShellSpec>,
}

impl PaneSeed {
    pub fn bare(pane: u64) -> PaneSeed {
        PaneSeed {
            pane,
            cwd: None,
            ssh_spec: None,
            agent: None,
            shell: None,
        }
    }

    /// The record `register_pane` mints for this seed. Public because the
    /// window that pushed the seed mirrors the machine's tree, and this
    /// record reaches it in nothing it is sent — a client is left out of the
    /// deltas its own ops raise — so it puts the same record in its mirror
    /// itself (#612).
    pub fn into_record(self, live: bool) -> PaneRecord {
        PaneRecord {
            id: self.pane,
            cwd: self.cwd,
            title: String::new(),
            osc_title: None,
            ssh_spec: self.ssh_spec.map(|s| Box::new(s.without_secrets())),
            agent: self.agent,
            shell: self.shell,
            live,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutDelta {
    ActiveTabChanged {
        tab: TabId,
    },
    TabCreated {
        at: usize,
        tab: Tab,
    },
    TabClosed {
        tab: TabId,
    },
    TabRenamed {
        tab: TabId,
        name: Option<String>,
    },
    TabMoved {
        tab: TabId,
        to: usize,
    },
    TabTagsChanged {
        tab: TabId,
        tags: Vec<TagId>,
    },
    TagsChanged {
        tags: SidebarTags,
    },
    TabRestructured {
        tab: Tab,
        pane: Option<PaneRecord>,
    },
    RatioChanged {
        tab: TabId,
        path: Vec<Side>,
        ratio: f32,
    },
    PaneFacts {
        pane: PaneRecord,
    },
}
