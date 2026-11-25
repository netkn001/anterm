use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::daemon::protocol::NativeSshSpec;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SessionAxis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionPane {
    Leaf {
        #[serde(default)]
        cwd: Option<PathBuf>,
        #[serde(default)]
        pane_id: Option<u64>,
        /// The shell this pane was running. Without it a pane that has to be
        /// respawned — its daemon restarted, or this is a cold start — comes
        /// back on whatever the default shell is, which is how a bash pane
        /// turns into a PowerShell one.
        #[serde(default)]
        shell: Option<crate::daemon::protocol::ShellSpec>,
        #[serde(default)]
        ssh_spec: Option<Box<NativeSshSpec>>,
        #[serde(default)]
        agent: Option<crate::cli_agent::CLIAgent>,
        #[serde(default)]
        agent_session_id: Option<String>,
        #[serde(default)]
        agent_launch_argv: Option<Vec<String>>,
    },
    Split {
        axis: SessionAxis,
        #[serde(default = "default_ratio")]
        ratio: f32,
        a: Box<SessionPane>,
        b: Box<SessionPane>,
    },
}

fn default_ratio() -> f32 {
    0.5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTab {
    #[serde(default)]
    pub name: Option<String>,
    pub pane: SessionPane,
    /// User-defined tags attached to this tab.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<crate::tag::TagId>,
    #[serde(skip)]
    pub tree_id: Option<crate::machine::TabId>,
    /// Asleep in the tree: comes back as a placeholder holding `pane`, and
    /// nothing is spawned for it until it is woken. See
    /// [`Tab::hibernated`](crate::machine::Tab::hibernated).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hibernated: bool,
    /// What a sleeping tab is called while nothing in it runs to say — read
    /// off the tree's pane records, the same way the switcher names a tab of
    /// a window it does not own.
    #[serde(skip)]
    pub asleep_view: Option<crate::tab_view::TabView>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub active: usize,
    pub tabs: Vec<SessionTab>,
}
