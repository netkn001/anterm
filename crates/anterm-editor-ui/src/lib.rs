//! Built-in code editor UI feature crate: `CodeEditorWorkspace`, split groups,
//! document column, editor session persistence, and LSP client UI bridge.

extern crate gpui_kit as gpui;

pub mod code_editor;
pub mod document_column;
pub mod editor_session;
pub mod lsp;
pub mod workspace;

pub use workspace::{
    CodeEditorEvent, CodeEditorWorkspace, DocumentRenderMode, EditorWorkspaceContext,
    TabDocumentState,
};
