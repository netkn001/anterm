//! Desktop UI organized into five domain trees:
//!
//! - [`workspace`] — windows, tabs, panes, switcher, sidebar, tab strip
//! - [`remote`] — SSH/WSL connections, SFTP, forwards, daemon tree sync
//! - [`editor`] — code editor views, document column, LSP
//! - [`files`] — diff viewer, file tree, SCM panel, right detail panel
//! - [`chrome`] — settings, search palette, tray

pub(crate) mod chrome;
pub(crate) mod editor;
pub(crate) mod files;
pub(crate) mod remote;
pub(crate) mod workspace;

pub(crate) use anterm_ui_kit::confirm_answers;
