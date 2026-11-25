//! Pure diff data model shared between git diff producers (`anterm-core`),
//! row layout engines (`anterm-editor`), and UI diff views (`anterm-app`).

use serde::{Deserialize, Serialize};

/// How the diff overlay lays a file out. Side-by-side is the default because
/// that is what everyone already sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffViewMode {
    #[default]
    Split,
    Unified,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub text: String,
}
