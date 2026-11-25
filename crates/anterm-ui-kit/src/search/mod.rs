//! Shared Search Everywhere / Command Palette data models, fuzzy scorer,
//! catalog sources, and project file index.

pub mod command;
pub mod files;
pub mod score;
pub mod sources;

pub use command::{
    Avatar, ChromeState, CommandGroup, CommandKind, Item, PaletteAction, PickerTarget,
};
pub use files::{FileIndex, FileIndexStore, FileList, IndexedFile};
pub use sources::{Catalog, LiveQuery, Section, Source, host_items};

use anterm_i18n::{L10nKey, t};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchTab {
    Actions,
    Tabs,
    Tags,
    Shells,
    Hosts,
    Files,
    Symbols,
    Locations,
}

impl SearchTab {
    pub const EDITOR_ORDER: [SearchTab; 2] = [SearchTab::Files, SearchTab::Symbols];

    pub fn in_editor_row(self) -> bool {
        Self::EDITOR_ORDER.contains(&self)
    }

    pub fn title(self) -> &'static str {
        t(match self {
            SearchTab::Actions => L10nKey::SearchTabActions,
            SearchTab::Tabs => L10nKey::SearchTabTabs,
            SearchTab::Tags => L10nKey::SearchTabTags,
            SearchTab::Shells => L10nKey::SettingsShell,
            SearchTab::Hosts => L10nKey::SearchTabHosts,
            SearchTab::Files => L10nKey::SearchTabFiles,
            SearchTab::Symbols => L10nKey::SearchTabSymbols,
            SearchTab::Locations => L10nKey::SearchTabLocations,
        })
    }

    pub fn placeholder(self) -> &'static str {
        t(match self {
            SearchTab::Actions => L10nKey::SearchPlaceholderActions,
            SearchTab::Tabs => L10nKey::SearchAllOpenTabsPlaceholder,
            SearchTab::Tags => L10nKey::SearchPlaceholderSwitchTag,
            SearchTab::Shells => L10nKey::SearchAllShellsPlaceholder,
            SearchTab::Hosts => L10nKey::SearchPlaceholderHosts,
            SearchTab::Files => L10nKey::SearchPlaceholderFiles,
            SearchTab::Symbols => L10nKey::SearchPlaceholderSymbols,
            SearchTab::Locations => L10nKey::SearchPlaceholderLocations,
        })
    }
}
