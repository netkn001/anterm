//! Lightweight tab tags shared across the wire protocol, daemon, CLI, and GUI.
//!
//! Tabs stay in a flat list; tags are user-managed labels with an optional
//! semantic color that can be attached to any number of tabs and used to filter
//! the sidebar.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Stable identity for a tag definition. Renaming a tag preserves membership
/// on every tab pointing to its [`TagId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TagId(uuid::Uuid);

impl TagId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for TagId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TagId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Optional low-saturation accent color for a tag dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagColor {
    #[default]
    None,
    Red,
    Orange,
    Amber,
    Green,
    Teal,
    Blue,
    Purple,
    Pink,
}

impl TagColor {
    pub const ALL: [Self; 9] = [
        Self::None,
        Self::Red,
        Self::Orange,
        Self::Amber,
        Self::Green,
        Self::Teal,
        Self::Blue,
        Self::Purple,
        Self::Pink,
    ];

    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Red => "red",
            Self::Orange => "orange",
            Self::Amber => "amber",
            Self::Green => "green",
            Self::Teal => "teal",
            Self::Blue => "blue",
            Self::Purple => "purple",
            Self::Pink => "pink",
        }
    }
}

/// One user-defined tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagDef {
    pub id: TagId,
    pub name: String,
    #[serde(default, skip_serializing_if = "TagColor::is_none")]
    pub color: TagColor,
}

impl TagDef {
    pub fn new(name: impl Into<String>, color: TagColor) -> Self {
        Self {
            id: TagId::new(),
            name: name.into().trim().to_string(),
            color,
        }
    }
}

/// The machine's catalog of user-defined tab tags, in display order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarTags {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<TagDef>,
}

impl SidebarTags {
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }

    pub fn get(&self, id: TagId) -> Option<&TagDef> {
        self.tags.iter().find(|t| t.id == id)
    }

    pub fn get_mut(&mut self, id: TagId) -> Option<&mut TagDef> {
        self.tags.iter_mut().find(|t| t.id == id)
    }

    pub fn contains(&self, id: TagId) -> bool {
        self.tags.iter().any(|t| t.id == id)
    }

    /// Case-insensitive lookup by trimmed tag name.
    pub fn find_by_name(&self, name: &str) -> Option<&TagDef> {
        let needle = name.trim();
        if needle.is_empty() {
            return None;
        }
        self.tags
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(needle))
    }

    /// Appends a new tag if `name` is non-empty after trimming. If a tag with
    /// the same name (case-insensitive) already exists, returns its existing id.
    pub fn create(&mut self, name: impl Into<String>, color: TagColor) -> Option<TagId> {
        let trimmed = name.into().trim().to_string();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(existing) = self.find_by_name(&trimmed) {
            return Some(existing.id);
        }
        let def = TagDef {
            id: TagId::new(),
            name: trimmed,
            color,
        };
        let id = def.id;
        self.tags.push(def);
        Some(id)
    }

    /// Renames `id` to `name`. Rejects blank names or collisions with another tag.
    pub fn rename(&mut self, id: TagId, name: impl Into<String>) -> bool {
        let trimmed = name.into().trim().to_string();
        if trimmed.is_empty() {
            return false;
        }
        if self
            .tags
            .iter()
            .any(|t| t.id != id && t.name.eq_ignore_ascii_case(&trimmed))
        {
            return false;
        }
        let Some(tag) = self.get_mut(id) else {
            return false;
        };
        if tag.name == trimmed {
            return false;
        }
        tag.name = trimmed;
        true
    }

    /// Updates `id`'s color. Returns `true` if the color changed.
    pub fn set_color(&mut self, id: TagId, color: TagColor) -> bool {
        let Some(tag) = self.get_mut(id) else {
            return false;
        };
        if tag.color == color {
            return false;
        }
        tag.color = color;
        true
    }

    /// Removes `id` from the catalog. Returns `true` if it was present.
    pub fn remove(&mut self, id: TagId) -> bool {
        let before = self.tags.len();
        self.tags.retain(|t| t.id != id);
        self.tags.len() != before
    }

    /// Drops unknown and duplicate tag ids from `ids`, preserving first-seen order.
    pub fn retain_valid(&self, ids: &mut Vec<TagId>) -> bool {
        let before = ids.len();
        let mut seen = HashSet::with_capacity(ids.len());
        ids.retain(|id| self.contains(*id) && seen.insert(*id));
        ids.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_dedups_case_insensitively_and_trims() {
        let mut catalog = SidebarTags::default();
        assert_eq!(catalog.create("   ", TagColor::None), None);
        let id = catalog.create("  Prod ", TagColor::Red).unwrap();
        assert_eq!(catalog.get(id).unwrap().name, "Prod");
        assert_eq!(catalog.get(id).unwrap().color, TagColor::Red);
        assert_eq!(catalog.create("prod", TagColor::Blue), Some(id));
        assert_eq!(catalog.tags.len(), 1);
    }

    #[test]
    fn rename_and_color_and_remove() {
        let mut catalog = SidebarTags::default();
        let a = catalog.create("Frontend", TagColor::Blue).unwrap();
        let b = catalog.create("Backend", TagColor::Green).unwrap();

        assert!(!catalog.rename(a, "   "));
        assert!(!catalog.rename(a, "backend"));
        assert!(catalog.rename(a, "Web"));
        assert_eq!(catalog.get(a).unwrap().name, "Web");

        assert!(catalog.set_color(a, TagColor::Purple));
        assert!(!catalog.set_color(a, TagColor::Purple));
        assert_eq!(catalog.get(a).unwrap().color, TagColor::Purple);

        assert!(catalog.remove(b));
        assert!(!catalog.contains(b));
        assert_eq!(catalog.tags.len(), 1);
    }

    #[test]
    fn retain_valid_drops_unknown_and_duplicates_in_order() {
        let mut catalog = SidebarTags::default();
        let a = catalog.create("A", TagColor::None).unwrap();
        let b = catalog.create("B", TagColor::None).unwrap();
        let missing = TagId::new();

        let mut ids = vec![b, missing, a, b, a];
        assert!(catalog.retain_valid(&mut ids));
        assert_eq!(ids, vec![b, a]);
        assert!(!catalog.retain_valid(&mut ids));
    }

    #[test]
    fn default_serializes_compactly_and_round_trips() {
        let empty = SidebarTags::default();
        assert_eq!(serde_json::to_string(&empty).unwrap(), "{}");

        let mut catalog = SidebarTags::default();
        catalog.create("Ops", TagColor::None);
        catalog.create("Urgent", TagColor::Red);
        let json = serde_json::to_string(&catalog).unwrap();
        assert!(!json.contains("\"none\""));
        let decoded: SidebarTags = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, catalog);
    }
}
