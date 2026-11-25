//! Command Palette (`Cmd+P`) and focused quick-pickers.
//! Pure data models, fuzzy scorer, catalog sources, and file index live in
//! `anterm_ui_kit::search`.

pub(crate) mod files;
mod view;

pub(crate) use anterm_ui_kit::search::{
    Avatar, Catalog, ChromeState, CommandGroup, CommandKind, FileList, Item,
    LiveQuery, PaletteAction, PickerTarget, SearchTab, command, host_items, score, sources,
};
pub(crate) use view::{KEY_CONTEXT, SearchEvent, SearchView};
#[cfg(test)]
pub(crate) use view::{TagPopoverRow, next_tag_color, popover_rows};
