mod appearance;
mod migration;
mod model;
mod store;
#[cfg(test)]
mod tests;

pub use crate::util::unix_now;

pub use appearance::{APPEARANCE_FILE, Appearance, appearance, appearance_path, note_appearance};
pub use migration::{DATA_DIR_ENV, adopt_legacy_data_dir};
pub use model::{
    AgentFacts, Axis, CLOSED_TAB_TTL, ClosedTab, LayoutDelta, MAX_CLOSED_TABS, MAX_PANES, Machine,
    PaneNode, PaneRecord, PaneSeed, Remembered, ReopenedTab, Side, Tab, TabId,
};
pub use store::{
    FACT_FLUSH_INTERVAL, LivenessProbe, MACHINE_FILE, MachineStore, Notify, SubscriberId,
    Subscription, default_machine_path, observe_pane, observed_store, publish_observations,
};
#[cfg(test)]
pub(crate) use store::{OBSERVE_SLOT, withdraw_observations};
