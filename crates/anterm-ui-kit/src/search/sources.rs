//! What each picker holds (`Actions`, `Tabs`, `Tags`, `Shells`, `Hosts`,
//! `Files`, `Symbols`, `Locations`) and how it ranks rows against a query.

use gpui::{App, SharedString};

use super::SearchTab;
use super::command::{CommandGroup, Item, PickerTarget};
use super::files::{FileList, Files};
use super::score::{frecency_bonus, item_score};
use crate::config::Config;
use anterm_core::ssh_profile::parse_quick_connect;
use anterm_i18n::{L10nKey, t, t_fmt};

/// Leading prefix routing when typing inside the root `Actions` command
/// palette (`@` → Tabs, `#` → Tags, `/` → Files, `>` → Actions passthrough).
///
/// Returns `(effective_tab, stripped_query, routed_via_prefix)`.
pub fn route_actions_query(raw: &str) -> (SearchTab, &str, bool) {
    let trimmed = raw.trim_start();
    if let Some(rest) = trimmed.strip_prefix('@') {
        (SearchTab::Tabs, rest.trim_start(), true)
    } else if let Some(rest) = trimmed.strip_prefix('#') {
        (SearchTab::Tags, rest.trim_start(), true)
    } else if let Some(rest) = trimmed.strip_prefix('/') {
        (SearchTab::Files, rest.trim_start(), true)
    } else if let Some(rest) = trimmed.strip_prefix('>') {
        (SearchTab::Actions, rest.trim_start(), false)
    } else {
        (SearchTab::Actions, raw, false)
    }
}

/// How many recently used commands float to the top of the empty Command Palette.
const RECENT_ROWS: usize = 5;

/// Rows under one optional heading. A section never holds zero rows.
#[derive(Clone)]
pub struct Section {
    pub title: Option<SharedString>,
    pub rows: Vec<Item>,
}

/// One picker's backing source.
pub trait Source {
    /// Rows shown when nothing is typed yet.
    fn browse(&self, cx: &App) -> Vec<Section>;

    /// Rows matching `query`, best first, each paired with its score.
    fn search(&self, query: &str, cx: &App) -> Vec<(i32, Item)>;
}

/// Everything the palette and its focused sub-pickers can search, gathered once
/// when the modal opens (`AntermApp::search_catalog`).
#[derive(Clone, Default)]
pub struct Catalog {
    /// Commands in group order.
    pub actions: Vec<Item>,
    /// Open tabs for `Switch Tab…`, most recently used first.
    pub tabs: Vec<Item>,
    /// Sidebar tags plus `All` for `Switch Tag…`.
    pub tags: Vec<Item>,
    /// Detected shells for the New Tab menu's `Other Shells…` picker.
    pub shells: Vec<Item>,
    /// Saved SSH hosts, most used first.
    pub hosts: Vec<Item>,
    /// Project files for Go to File (`AntermApp::refresh_file_index`).
    pub files: FileList,
    /// Active editor file symbols with nesting depth (Go to Symbol).
    pub symbols: Vec<(usize, Item)>,
    /// Language-server locations (Find References / Definitions).
    pub locations: Vec<Item>,
    /// Workspace symbols returned asynchronously by the language server.
    pub project_symbols: Vec<Item>,
    /// Live query callback into the language server for `Symbols`.
    pub live_query: Option<LiveQuery>,
    /// Active tab `(TabId, current_label)` used to prefill the in-palette
    /// `Rename Tab` prompt and `Edit Tab Tags` page.
    pub active_tab_rename: Option<(anterm_core::machine::TabId, String)>,
    /// Display labels for all open tabs (used by `Switch Tab…` → `⌘E` Rename).
    pub tab_labels: std::collections::HashMap<anterm_core::machine::TabId, String>,
    /// Live tag catalog snapshot for the unified in-palette `Edit Tab Tags` page.
    pub sidebar_tags: anterm_core::tag::SidebarTags,
    /// Per-tab attached tags snapshot for `Edit Tab Tags`.
    pub tab_tags: std::collections::HashMap<anterm_core::machine::TabId, Vec<anterm_core::tag::TagId>>,
}

/// Asynchronous live query callback (language server workspace symbols).
pub type LiveQuery = std::rc::Rc<dyn Fn(&str, &mut App)>;

impl Catalog {
    pub fn new(mut actions: Vec<Item>, shells: Vec<Item>, hosts: Vec<Item>) -> Self {
        actions.sort_by_key(|item| {
            item.group
                .and_then(|g| CommandGroup::ORDER.iter().position(|o| *o == g))
                .unwrap_or(usize::MAX)
        });
        Self {
            actions,
            tabs: Vec::new(),
            tags: Vec::new(),
            shells,
            hosts,
            files: FileList::default(),
            symbols: Vec::new(),
            locations: Vec::new(),
            project_symbols: Vec::new(),
            live_query: None,
            active_tab_rename: None,
            tab_labels: std::collections::HashMap::new(),
            sidebar_tags: anterm_core::tag::SidebarTags::default(),
            tab_tags: std::collections::HashMap::new(),
        }
    }

    pub fn source(&self, tab: SearchTab) -> Box<dyn Source + '_> {
        match tab {
            SearchTab::Actions => Box::new(Actions(&self.actions)),
            SearchTab::Tabs => Box::new(FlatItems(&self.tabs)),
            SearchTab::Tags => Box::new(FlatItems(&self.tags)),
            SearchTab::Shells => Box::new(FlatItems(&self.shells)),
            SearchTab::Hosts => Box::new(Hosts(&self.hosts)),
            SearchTab::Files => Box::new(Files(&self.files)),
            SearchTab::Symbols => Box::new(Symbols(&self.symbols)),
            SearchTab::Locations => Box::new(FlatItems(&self.locations)),
        }
    }

    /// Sections to display for `tab` and `query`.
    pub fn sections(&self, tab: SearchTab, query: &str, cx: &App) -> Vec<Section> {
        let query = query.trim();
        if tab == SearchTab::Symbols && !query.is_empty() && !self.project_symbols.is_empty() {
            return self.symbols_here_and_in_the_project(query, cx);
        }
        let source = self.source(tab);
        match query.is_empty() {
            true => source.browse(cx),
            false => untitled(source.search(query, cx)),
        }
    }

    fn symbols_here_and_in_the_project(&self, query: &str, cx: &App) -> Vec<Section> {
        let rows = |hits: Vec<(i32, Item)>| -> Vec<Item> {
            hits.into_iter().map(|(_, item)| item).collect()
        };
        let here = rows(Symbols(&self.symbols).search(query, cx));
        let everywhere = rows(rank(&self.project_symbols, query, |_| 0));
        let both = !here.is_empty() && !everywhere.is_empty();
        [
            (L10nKey::SearchSectionThisFile, here),
            (L10nKey::SearchSectionProject, everywhere),
        ]
        .into_iter()
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(title, rows)| Section {
            title: both.then(|| t(title).into()),
            rows,
        })
        .collect()
    }
}

/// Flat ranked list without section headers.
fn untitled(hits: Vec<(i32, Item)>) -> Vec<Section> {
    if hits.is_empty() {
        return Vec::new();
    }
    vec![Section {
        title: None,
        rows: hits.into_iter().map(|(_, item)| item).collect(),
    }]
}

/// Single flat section without headers.
fn flat(items: Vec<Item>) -> Vec<Section> {
    if items.is_empty() {
        return Vec::new();
    }
    vec![Section {
        title: None,
        rows: items,
    }]
}

/// Every matching row, best first (stable so equal scores keep browse order).
fn rank(items: &[Item], query: &str, bonus: impl Fn(&Item) -> i32) -> Vec<(i32, Item)> {
    let mut hits: Vec<(i32, Item)> = items
        .iter()
        .filter_map(|item| Some((item_score(query, item)? + bonus(item), item.clone())))
        .collect();
    hits.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    hits
}

fn action_usage(item: &Item, cx: &App) -> f64 {
    let cfg = cx.global::<Config>();
    item.kind
        .id()
        .and_then(|id| cfg.command_frecency.get(id))
        .map(|u| u.score(crate::config::unix_now()))
        .unwrap_or(0.0)
}

/// Zed-style pure flat command palette: recently used commands float to the
/// top without duplication, followed by all remaining commands in group order.
struct Actions<'a>(&'a [Item]);

impl Actions<'_> {
    fn recent(&self, cx: &App) -> Vec<&Item> {
        let mut used: Vec<(f64, &Item)> = self
            .0
            .iter()
            .map(|item| (action_usage(item, cx), item))
            .filter(|(score, _)| *score > 0.0)
            .collect();
        used.sort_by(|a, b| b.0.total_cmp(&a.0));
        used.truncate(RECENT_ROWS);
        used.into_iter().map(|(_, item)| item).collect()
    }
}

impl Source for Actions<'_> {
    fn browse(&self, cx: &App) -> Vec<Section> {
        let recent = self.recent(cx);
        let mut rows: Vec<Item> = recent.iter().map(|item| (*item).clone()).collect();
        rows.extend(
            self.0
                .iter()
                .filter(|item| !recent.iter().any(|r| r.kind == item.kind))
                .cloned(),
        );
        flat(rows)
    }

    fn search(&self, query: &str, cx: &App) -> Vec<(i32, Item)> {
        rank(self.0, query, |item| frecency_bonus(action_usage(item, cx)))
    }
}

/// Flat pre-ordered item picker (`Tabs`, `Tags`, `Shells`, `Locations`).
struct FlatItems<'a>(&'a [Item]);

impl Source for FlatItems<'_> {
    fn browse(&self, _cx: &App) -> Vec<Section> {
        flat(self.0.to_vec())
    }

    fn search(&self, query: &str, _cx: &App) -> Vec<(i32, Item)> {
        rank(self.0, query, |_| 0)
    }
}

/// A typed address outranks any saved host: typing one is saying where to go.
const TYPED_ADDRESS_SCORE: i32 = 10_000;

struct Symbols<'a>(&'a [(usize, Item)]);

const SYMBOL_INDENT: &str = "    ";

impl Source for Symbols<'_> {
    fn browse(&self, _cx: &App) -> Vec<Section> {
        if self.0.is_empty() {
            return Vec::new();
        }
        vec![Section {
            title: None,
            rows: self
                .0
                .iter()
                .map(|(depth, item)| {
                    let mut item = item.clone();
                    item.title = format!("{}{}", SYMBOL_INDENT.repeat(*depth), item.title);
                    item.subtitle = None;
                    item
                })
                .collect(),
        }]
    }

    fn search(&self, query: &str, _cx: &App) -> Vec<(i32, Item)> {
        let items: Vec<Item> = self.0.iter().map(|(_, item)| item.clone()).collect();
        rank(&items, query, |_| 0)
    }
}

struct Hosts<'a>(&'a [Item]);

impl Source for Hosts<'_> {
    fn browse(&self, _cx: &App) -> Vec<Section> {
        flat(self.0.to_vec())
    }

    fn search(&self, query: &str, _cx: &App) -> Vec<(i32, Item)> {
        let mut out: Vec<(i32, Item)> = typed_address_rows(query)
            .into_iter()
            .map(|item| (TYPED_ADDRESS_SCORE, item))
            .collect();
        out.extend(rank(self.0, query, |_| 0));
        out
    }
}

/// Quick-connect rows offered when a typed query parses as an SSH address or
/// full `ssh` command line.
pub fn typed_address_rows(query: &str) -> Vec<Item> {
    let query = query.trim();
    if query.contains(char::is_whitespace) {
        return match crate::ssh_helpers::parse_ssh_connect_input(query) {
            Ok(_) => vec![Item::new(
                t_fmt(L10nKey::CmdSshConnectWithInput, &[("input", query)]),
                PickerTarget::OpenSshConnect(query.to_string()),
            )],
            Err(_) => Vec::new(),
        };
    }
    if query.contains(['@', ':', '.']) && parse_quick_connect(query).is_some() {
        let target = query.to_string();
        return vec![
            Item::new(
                t_fmt(L10nKey::CmdQuickConnect, &[("target", &target)]),
                PickerTarget::QuickConnect(target.clone()),
            ),
            Item::new(
                t_fmt(L10nKey::CmdQuickConnectSaveProfile, &[("target", &target)]),
                PickerTarget::SaveQuickConnect(target),
            ),
        ];
    }
    Vec::new()
}

/// Every saved SSH host, most used first.
pub fn host_items(cx: &App) -> Vec<Item> {
    crate::ssh_helpers::ssh_profiles_by_frecency(cx)
        .into_iter()
        .map(|p| {
            let endpoint = anterm_core::ssh_profile::to_connect_string(&p);
            let title = match p.name.is_empty() {
                true => endpoint.clone(),
                false => p.name.clone(),
            };
            let mut item = Item::new(title, PickerTarget::ConnectSavedProfile(p.id));
            if item.title != endpoint {
                item = item.with_subtitle(endpoint);
            }
            item
        })
        .collect()
}

/// Flat sub-picker list (theme picker): all items in order when empty, ranked
/// flat once something is typed.
pub fn plain(items: &[Item], query: &str) -> Vec<Section> {
    let query = query.trim();
    match query.is_empty() {
        true => flat(items.to_vec()),
        false => untitled(rank(items, query, |_| 0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn titles(query: &str) -> Vec<String> {
        typed_address_rows(query)
            .into_iter()
            .map(|c| c.title)
            .collect()
    }

    #[test]
    fn a_bare_word_is_a_search_not_an_address() {
        assert!(titles("java").is_empty());
        assert!(titles("split").is_empty());
        assert!(titles("").is_empty());
    }

    #[test]
    fn an_address_offers_to_connect_and_to_save() {
        anterm_i18n::set_locale("en");
        for q in [
            "deploy@10.0.0.5",
            "host.example.com",
            "java:2222",
            "ssh://java",
            "[::1]:2222",
        ] {
            assert_eq!(
                titles(q),
                vec![
                    t_fmt(L10nKey::CmdQuickConnect, &[("target", q)]),
                    t_fmt(L10nKey::CmdQuickConnectSaveProfile, &[("target", q)]),
                ],
                "query {q:?}"
            );
        }
    }

    #[test]
    fn an_address_that_does_not_parse_offers_nothing() {
        assert!(titles("java:99999").is_empty());
        assert!(titles("@").is_empty());
    }

    #[test]
    fn an_ssh_command_line_offers_to_connect_with_it() {
        let rows = typed_address_rows("ssh -p 2222 deploy@box");
        let [row] = rows.as_slice() else {
            panic!("one row, got {}", rows.len());
        };
        assert_eq!(
            row.kind,
            PickerTarget::OpenSshConnect("ssh -p 2222 deploy@box".into()).into()
        );
    }

    #[test]
    fn route_actions_query_recognizes_prefixes() {
        assert_eq!(route_actions_query("@beta"), (SearchTab::Tabs, "beta", true));
        assert_eq!(route_actions_query("  #ops"), (SearchTab::Tags, "ops", true));
        assert_eq!(
            route_actions_query("/src/main.rs"),
            (SearchTab::Files, "src/main.rs", true)
        );
        assert_eq!(
            route_actions_query(">split"),
            (SearchTab::Actions, "split", false)
        );
        assert_eq!(
            route_actions_query("split"),
            (SearchTab::Actions, "split", false)
        );
    }

    fn with_config(cx: &mut TestAppContext) {
        crate::config::pin_test_config_dir();
        cx.update(|cx| {
            cx.set_global(Config::default());
            anterm_i18n::set_locale("en");
        });
    }

    fn row_titles(section: &Section) -> Vec<String> {
        section.rows.iter().map(|item| item.title.clone()).collect()
    }

    #[gpui::test]
    fn symbols_lists_the_file_then_the_project(cx: &mut TestAppContext) {
        with_config(cx);
        let here = |name: &str, line| {
            (
                0,
                Item::new(name, PickerTarget::GoToSymbol { line, column: 0 }),
            )
        };
        let there = |name: &str| {
            Item::new(
                name,
                PickerTarget::GoToLocation {
                    path: "/p/other.rs".into(),
                    line: 0,
                    column: 0,
                },
            )
        };
        let mut catalog = Catalog::new(Vec::new(), Vec::new(), Vec::new());
        catalog.symbols = vec![here("render", 3), here("Config", 9)];
        let shape = |catalog: &Catalog, query: &str, cx: &mut TestAppContext| {
            cx.update(|cx| {
                catalog
                    .sections(SearchTab::Symbols, query, cx)
                    .into_iter()
                    .map(|s| (s.title.as_ref().map(|t| t.to_string()), row_titles(&s)))
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(
            shape(&catalog, "render", cx),
            vec![(None, vec!["render".to_string()])]
        );
        catalog.project_symbols = vec![there("render_row"), there("unrelated")];
        assert_eq!(
            shape(&catalog, "render", cx),
            vec![
                (Some("In This File".into()), vec!["render".to_string()]),
                (Some("Project".into()), vec!["render_row".to_string()]),
            ]
        );
        assert_eq!(
            shape(&catalog, "unrelated", cx),
            vec![(None, vec!["unrelated".to_string()])]
        );
        assert!(
            shape(&catalog, "", cx)
                .iter()
                .all(|(_, rows)| !rows.contains(&"render_row".to_string()))
        );
    }

    #[gpui::test]
    fn actions_browse_is_a_single_flat_list_ordered_by_group(cx: &mut TestAppContext) {
        use super::super::command::PaletteAction;
        with_config(cx);
        let late = Item::new("Late", PaletteAction::EditTabTags).in_group(CommandGroup::TabsPanes);
        let first = Item::new("First", PaletteAction::NewTab).in_group(CommandGroup::TabsPanes);
        let app = Item::new("Quit", PaletteAction::Quit).in_group(CommandGroup::Application);
        let catalog = Catalog::new(vec![app, first, late], Vec::new(), Vec::new());
        cx.update(|cx| {
            let sections = catalog.sections(SearchTab::Actions, "", cx);
            assert_eq!(sections.len(), 1);
            assert_eq!(sections[0].title, None);
            assert_eq!(row_titles(&sections[0]), vec!["First", "Late", "Quit"]);
        });
    }

    #[gpui::test]
    fn the_hosts_tab_puts_a_typed_address_before_any_saved_host(cx: &mut TestAppContext) {
        use super::super::command::CommandKind;
        with_config(cx);
        let saved = Item::new(
            "prod",
            PickerTarget::ConnectSavedProfile(uuid::Uuid::new_v4()),
        )
        .with_subtitle("deploy@prod.example.com");
        let catalog = Catalog::new(Vec::new(), Vec::new(), vec![saved]);
        cx.update(|cx| {
            let sections = catalog.sections(SearchTab::Hosts, "deploy@prod.example.com", cx);
            let kinds: Vec<_> = sections[0].rows.iter().map(|i| i.kind.clone()).collect();
            assert!(matches!(
                kinds[0],
                CommandKind::Pick(PickerTarget::QuickConnect(_))
            ));
            assert!(matches!(
                kinds[1],
                CommandKind::Pick(PickerTarget::SaveQuickConnect(_))
            ));
            assert!(matches!(
                kinds[2],
                CommandKind::Pick(PickerTarget::ConnectSavedProfile(_))
            ));
        });
    }

    #[gpui::test]
    fn a_file_query_with_a_line_opens_on_that_line(cx: &mut TestAppContext) {
        with_config(cx);
        let mut catalog = Catalog::new(Vec::new(), Vec::new(), Vec::new());
        let root = std::path::PathBuf::from("/repo");
        catalog.files = FileList::Ready(std::sync::Arc::new(super::super::files::build_index(
            std::slice::from_ref(&root),
            vec![root.join("src/main.rs")],
            false,
        )));
        cx.update(|cx| {
            let sections = catalog.sections(SearchTab::Files, "main.rs:120:4", cx);
            let item = &sections[0].rows[0];
            assert_eq!(
                item.kind,
                PickerTarget::OpenFile {
                    path: "/repo/src/main.rs".into(),
                    line: Some(120),
                    column: Some(4),
                }
                .into()
            );
            assert_eq!(item.subtitle.as_deref(), Some("src"));
            assert_eq!(item.note.as_deref(), Some("line 120"));
        });
    }
}
