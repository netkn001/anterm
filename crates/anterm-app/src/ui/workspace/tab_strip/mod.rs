use gpui::{
    Animation, AnimationExt as _, App, Axis, Bounds, Context, FontWeight, MouseButton,
    SharedString, Window, canvas, deferred, div, ease_out_quint, linear_color_stop,
    linear_gradient, prelude::*, px,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Side, Sizable as _, h_flex};

use crate::core::actions::{
    CloseActiveTab, CloseOtherTabs, CloseTabsToTheRight, CopyAgentSessionId, CopyWorkingDirectory,
    EditTabTags, ForkAgentSession, HibernateTab, MarkTabUnread, RenameTab,
    SplitDown, SplitLeft, SplitRight, SplitUp,
};
use crate::core::config::Config;
use crate::ui::workspace::app::{SpawnWhere, TILE_GLYPH, Tab, AntermApp, tile_trailing_inset};
use crate::ui::workspace::hints::tab_badge_label;
use anterm_i18n::{L10nKey, t};
use crate::ui::workspace::reorder::{self, Reorder, Surface};

mod widgets;

#[cfg(test)]
mod tests;

pub(crate) use widgets::{
    DragTab, GRAB_HANDLE_W, MIN_TARGET, RAIL_TILE_GLYPH,
    RAIL_TILE_RADIUS, REORDER_SLIDE_MS, ROW_STATUS_DOT, ROW_STATUS_SLOT,
    TRANSITION_MS, abbreviate_home, agent_status_label,
    chord_tooltip, chrome_tile, chrome_tile_sized, elide_end_clusters, elide_label,
    elide_path_keep_tail, hit_target, label_of, measure_text,
    shells_by_frecency, strip_host_prefix, trailing_chrome_tiles_w, trailing_chrome_w,
};
#[cfg(test)]
pub(crate) use widgets::short_title;
#[cfg(test)]
use widgets::{MENU_HOSTS, MENU_SHELLS, elide_keep_edges};
use widgets::{
    CHIP_GAP, CHIP_MIN_W, NewTabMenu, menu_hosts, menu_shells, shell_spec, strip_width,
    tooltip_of, visible_chips,
};

impl AntermApp {
    /// Persistent window controls, including when either sidebar is collapsed.
    pub(crate) fn window_chrome(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let trailing = match cfg!(target_os = "macos") {
            true => tile_trailing_inset(),
            false => 4.,
        };
        // Opening the panel moves this control to the panel's header on
        // macOS. Reuse the same tile and inset there: a different hit target
        // moves the centred glyph even when the icon itself keeps its size.
        let panel_open = self.right_panel_open(cx);
        h_flex()
            .flex_shrink_0()
            .items_center()
            .pr(px(trailing))
            .child(
                div().occlude().flex_shrink_0().child(
                    chrome_tile(
                        Button::new("titlebar-right-panel")
                            // Keep the outline fixed; only the panel region's
                            // fill changes to show whether the panel is visible.
                            .icon(Icon::empty().path(match panel_open {
                                true => "icons/panel-right.svg",
                                false => "icons/panel-right-collapsed.svg",
                            })),
                        false,
                        cx,
                    )
                    .rounded_lg()
                    .accessibility_label(match panel_open {
                        true => t(L10nKey::TabTooltipHideDetailPanel),
                        false => t(L10nKey::TabTooltipShowDetailPanel),
                    })
                    .tooltip_with_action(
                        match panel_open {
                            true => t(L10nKey::TabTooltipHideDetailPanel),
                            false => t(L10nKey::TabTooltipShowDetailPanel),
                        },
                        &crate::core::actions::ToggleRightPanel,
                        None,
                    )
                    // On macOS this tile is drawn inside the panel's own
                    // titlebar while the panel is open, so closing from it
                    // destroys the element holding the focus — and a keymap
                    // scoped to a focused thing goes quiet with it, leaving the
                    // ⌘J that would undo this doing nothing. Hand the terminal
                    // back what it lost.
                    .on_click(cx.listener(|this, _, window, cx| {
                        let closing = this.right_panel_open(cx);
                        this.toggle_right_panel(cx);
                        if closing {
                            this.focus_active(window, cx);
                        }
                    })),
                ),
            )
    }

    /// Working and Done differ only in hue (blue vs green), and Waiting vs Done
    /// — the pair that actually decides whether you go and look — is amber vs
    /// green, the pair red-green colour vision separates worst. Give Waiting a
    /// hole so it is a different *shape*, not just a different colour.
    fn status_dot(
        rgb: u32,
        unread: usize,
        size: f32,
        ring: gpui::Hsla,
        hollow: bool,
        faded: bool,
    ) -> gpui::AnyElement {
        let d = (size * 0.42).max(7.);
        // The halo was the surface itself, which is only a ring while the
        // surface is light — on a dark theme it went near-black and read as a
        // notch bitten out of the avatar rather than a badge sitting on it.
        // Light themes already ring the dot in white; give the dark ones the
        // same white edge, and the hollow Waiting dot the same white hole.
        let bg = match crate::ui::chrome::presets::surface_is_dark(ring) {
            true => gpui::white(),
            false => ring,
        };
        // The dim beat of a blink is a paler fill, still opaque: fading the
        // whole badge let the avatar show through the dot and its ring.
        let fill: gpui::Hsla = match faded {
            true => bg.blend(gpui::Hsla::from(gpui::rgb(rgb)).opacity(0.4)),
            false => gpui::rgb(rgb).into(),
        };
        if unread > 0 {
            let nd = (size * 0.72).max(13.0);
            let label = unread.min(9).to_string();
            div()
                .absolute()
                .right(px(-(nd - d) / 2.0 - d * 0.22))
                .bottom(px(-(nd - d) / 2.0 - d * 0.22))
                .size(px(nd))
                .rounded_full()
                .border_1()
                .border_color(bg)
                .bg(fill)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px((nd * 0.62).round()))
                .font_weight(FontWeight::BOLD)
                .text_color(gpui::white())
                .child(label)
                .into_any_element()
        } else {
            div()
                .absolute()
                .right(px(-(d * 0.22)))
                .bottom(px(-(d * 0.22)))
                .size(px(d))
                .rounded_full()
                .border_2()
                .border_color(bg)
                .bg(fill)
                .when(hollow, |dot| {
                    dot.flex()
                        .items_center()
                        .justify_center()
                        .child(div().size(px((d * 0.36).max(2.5))).rounded_full().bg(bg))
                })
                .into_any_element()
        }
    }

    pub(crate) fn tab_avatar(
        &self,
        id: impl Into<gpui::ElementId>,
        agent: Option<crate::core::cli_agent::CLIAgent>,
        status: Option<crate::core::cli_agent::AgentStatus>,
        unread: usize,
        ssh: Option<u32>,
        size: f32,
        cx: &App,
    ) -> gpui::AnyElement {
        self.tab_avatar_badged(id, agent, status, unread, ssh, size, true, cx)
    }

    /// The avatar without the agent's status badge, for a row that says the
    /// status at its trailing end instead (see [`row_status_dot`]). The
    /// tooltip still names the state; the SSH dot on a shell stays, since it
    /// describes the connection rather than an agent's run.
    ///
    /// [`row_status_dot`]: Self::row_status_dot
    pub(crate) fn tab_avatar_plain(
        &self,
        id: impl Into<gpui::ElementId>,
        agent: Option<crate::core::cli_agent::CLIAgent>,
        status: Option<crate::core::cli_agent::AgentStatus>,
        ssh: Option<u32>,
        size: f32,
        cx: &App,
    ) -> gpui::AnyElement {
        self.tab_avatar_badged(id, agent, status, 0, ssh, size, false, cx)
    }

    /// A row's trailing status mark: a 5px dot in the state's colour, hollow
    /// while the agent waits on the user, blinking while it works — or the
    /// unread count in a small pill once there is something to read. `None`
    /// for a row with nothing to report.
    pub(crate) fn row_status_dot(
        &self,
        status: Option<crate::core::cli_agent::AgentStatus>,
        unread: usize,
        surface: gpui::Hsla,
    ) -> Option<gpui::AnyElement> {
        let rgb = status.and_then(|s| s.dot_rgb())?;
        let hollow = status == Some(crate::core::cli_agent::AgentStatus::Waiting);
        let faded =
            status == Some(crate::core::cli_agent::AgentStatus::Working) && !self.working_dot_on;
        let ink: gpui::Hsla = gpui::rgb(rgb).into();
        let fill = match faded {
            true => surface.blend(ink.opacity(0.4)),
            false => ink,
        };
        let slot = div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .min_w(px(ROW_STATUS_SLOT));
        Some(
            match unread > 0 {
                true => slot.child(
                    div()
                        .h(px(14.))
                        .min_w(px(14.))
                        .px(px(3.))
                        .rounded_full()
                        .bg(fill)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(9.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(gpui::white())
                        .child(unread.min(9).to_string()),
                ),
                false => slot.child(
                    div()
                        .size(px(ROW_STATUS_DOT))
                        .rounded_full()
                        .when(!hollow, |d| d.bg(fill))
                        .when(hollow, |d| d.border_1().border_color(fill)),
                ),
            }
            .into_any_element(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn tab_avatar_badged(
        &self,
        id: impl Into<gpui::ElementId>,
        agent: Option<crate::core::cli_agent::CLIAgent>,
        status: Option<crate::core::cli_agent::AgentStatus>,
        unread: usize,
        ssh: Option<u32>,
        size: f32,
        badge: bool,
        cx: &App,
    ) -> gpui::AnyElement {
        avatar_disc(
            id,
            crate::ui::chrome::search::Avatar {
                agent,
                status,
                unread,
                ssh,
            },
            size,
            self.working_dot_on,
            badge,
            cx,
        )
    }

    /// The mark a tab wears while one of its panes is zoomed over the others
    /// (#752). Without it a zoomed tab is pixel-for-pixel a tab that only ever
    /// had one pane, and the only way to tell was to toggle the zoom off.
    ///
    /// Drawn in the tab entry rather than on the pane so it reads from either
    /// tab surface, and so it says something about the tabs you are *not*
    /// looking at — the zoom outlives a switch away from them.
    /// The mark a sleeping tab carries (#762), in the slot the zoom mark uses:
    /// leading, beside the other state marks, where the pointer that comes to
    /// read it does not cover it.
    pub(crate) fn sleep_mark(&self, id: impl Into<gpui::ElementId>, cx: &App) -> gpui::AnyElement {
        let tip = SharedString::from(t(L10nKey::TabTooltipAsleep));
        div()
            .id(id)
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .size(px(16.))
            .text_color(cx.theme().muted_foreground)
            .child(Icon::new(IconName::Moon).size(px(11.)))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .into_any_element()
    }

    pub(crate) fn zoom_mark(&self, id: impl Into<gpui::ElementId>, cx: &App) -> gpui::AnyElement {
        let tip = chord_tooltip(t(L10nKey::TabTooltipZoomed), "ToggleMaximizePane", cx);
        div()
            .id(id)
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .size(px(16.))
            .text_color(cx.theme().muted_foreground)
            .child(Icon::new(IconName::Maximize).size(px(11.)))
            // Not a `Button`, so this one goes through gpui's own `tooltip`
            // rather than `tooltip_element` — but it is the same builder, and
            // the chord lands in the same slot the chrome tiles use.
            .tooltip(move |window, cx| tip(window, cx).into())
            .into_any_element()
    }

    /// The full title behind a shortened one, for the row to name on hover.
    ///
    /// `tab_label` hands back a path elided to its last three segments and then
    /// capped, and the chip truncates whatever is left over — so a tab could
    /// read `…/a/b/c` with no way to find out which `a` that was. `None` when
    /// nothing was dropped, so tabs that already show their whole name stay
    /// quiet under the pointer.
    ///
    /// It has to unshorten whatever the label was *made of*, which is why it
    /// reads the same [`TabView`](crate::ui::remote::machine_mirror::TabView) the label
    /// did: a tab named after its directory wants that directory spelled out,
    /// not the title it never had. See [`tooltip_of`].
    pub(crate) fn tab_title_tooltip(
        &self,
        tab: &Tab,
        index: usize,
        window: Option<&Window>,
        cx: &App,
    ) -> Option<SharedString> {
        let (view, home) = tab.label_view(window, cx);
        tooltip_of(&view, index, home.as_deref())
    }

    /// What this window puts on a tab of its own — the same ladder, through the
    /// same renderer, as the switcher uses for a tab of somebody else's window.
    /// See [`label_of`].
    pub(crate) fn tab_label(
        &self,
        tab: &Tab,
        index: usize,
        window: Option<&Window>,
        cx: &App,
    ) -> String {
        let (view, home) = tab.label_view(window, cx);
        label_of(&view, index, home.as_deref())
    }

    /// The same ladder as [`tab_label`](Self::tab_label), left whole: a path
    /// is only abbreviated under the home and stripped of its `user@host:`,
    /// never cut down to its last segments. For the surfaces with room to
    /// spare — the rail's rows and the centred title — where only the width
    /// they have may decide what gets elided, not a segment count picked for
    /// the chips. `None` when there is nothing to say and the caller should
    /// fall back to its own placeholder.
    pub(crate) fn full_tab_label(
        &self,
        tab: &Tab,
        window: Option<&Window>,
        cx: &App,
    ) -> Option<String> {
        use crate::ui::remote::machine_mirror::TabLabel;

        if let Some(name) = tab.name.as_ref().filter(|n| !n.trim().is_empty()) {
            return Some(name.trim().to_string());
        }
        let (view, home) = tab.label_view(window, cx);
        let raw = match view.label() {
            TabLabel::Osc(title) | TabLabel::Cwd(title) => {
                abbreviate_home(strip_host_prefix(title.trim()), home.as_deref()).into_owned()
            }
            TabLabel::Agent(agent) => agent.display_name().to_string(),
            TabLabel::Named(name) => name.to_string(),
            TabLabel::Process(title) => title.to_string(),
            TabLabel::Unknown => String::new(),
        };
        (!raw.trim().is_empty()).then_some(raw)
    }

    /// The New Tab control: one `+` that drops the list of everything it could
    /// open — the installed shells, and the saved SSH hosts.
    ///
    /// It was briefly split in two, a `+` that opened a tab outright next to a
    /// caret for the list, the way Windows Terminal and VS Code split theirs.
    /// That buys back the click the menu costs on the most common action, and
    /// it costs a second mark in a row of single ones. This row is four icons
    /// wide and reads as four icons; a fifth that was half of the fourth had
    /// to earn its place by looking like a pair, and a pair is exactly what a
    /// chrome row of plain icons has no vocabulary for.
    ///
    /// So: one tile, drawn and hovered like every other tile beside it, and
    /// the click it costs is answered by ⌘T rather than by a second button.
    pub(crate) fn new_tab_button(
        &self,
        id: &'static str,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        self.new_tab_button_sized(id, crate::ui::workspace::app::TILE_SIZE, cx)
    }

    /// [`new_tab_button`](Self::new_tab_button) at another tile size — the
    /// document header can use a smaller tile without changing the title bar.
    pub(crate) fn new_tab_button_sized(
        &self,
        id: &'static str,
        tile: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let app = cx.entity().downgrade();
        let glyph = match tile < crate::ui::workspace::app::TILE_SIZE {
            true => RAIL_TILE_GLYPH,
            false => TILE_GLYPH,
        };
        chrome_tile_sized(
            Button::new(id).icon(Icon::new(IconName::Plus)),
            tile,
            glyph,
            false,
            cx,
        )
        .rounded(px(RAIL_TILE_RADIUS))
        // Every other tile in this row names itself on hover — Switch
        // Workspace, More, Hide Sidebar. The three New Tab buttons that
        // come through here were the ones left silent. The chord is worth
        // more here than anywhere else in the row: it is the way back to
        // opening a tab without reading a menu first.
        .accessibility_label(t(L10nKey::AppMenuNewTab))
        .tooltip_with_action(t(L10nKey::AppMenuNewTab), &crate::core::actions::NewTab, None)
        // Built when the menu opens, not when the strip draws: this
        // closure runs once per press, and again after each dismissal.
        .dropdown_menu(move |menu, window, cx| {
            let Some(this) = app.upgrade() else {
                return menu;
            };
            this.read(cx)
                .new_tab_menu_rows(app.clone(), cx)
                .build(menu, window)
        })
    }

    /// What the menu offers, read off the app as the menu opens — the builder
    /// runs on the popup's own entity, so the rows carry a weak handle back.
    fn new_tab_menu_rows(&self, app: gpui::WeakEntity<Self>, cx: &App) -> NewTabMenu {
        let default_shell = self.default_shell_label(cx);
        let usage = &cx.global::<Config>().shell_frecency;
        let now = crate::core::config::unix_now();
        let sorted = shells_by_frecency(&self.shells.shells, &default_shell, usage, now);
        let shells = menu_shells(&sorted, &default_shell, usage, now);
        NewTabMenu {
            app,
            more_shells: shells.len() < sorted.len(),
            shells: shells
                .into_iter()
                .map(|s| SharedString::from(s.label.clone()))
                .collect(),
            default_shell: SharedString::from(default_shell),
            hosts: menu_hosts(crate::ui::remote::ssh_connect::ssh_profiles_by_frecency(cx)),
        }
    }

    /// Open the inventory's shell by the label it is listed under, wherever
    /// `at` says — the New Tab menu's rows and the palette's `Shell:` commands
    /// both land here, so both count toward the frecency that orders them.
    pub(crate) fn open_listed_shell(
        &mut self,
        label: &str,
        at: SpawnWhere,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(spec) = self
            .shells
            .shells
            .iter()
            .find(|s| s.label == label)
            .map(shell_spec)
        else {
            return;
        };
        self.update_config(cx, |cfg| {
            let entry = cfg.shell_frecency.entry(label.to_string()).or_default();
            entry.count = entry.count.saturating_add(1);
            entry.last_used = crate::core::config::unix_now();
        });
        self.open_shell(Some(spec), at, window, cx);
    }

    pub(crate) fn tab_context_menu(
        menu: PopupMenu,
        index: usize,
        below_wording: bool,
        app: &gpui::WeakEntity<Self>,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let Some(entity) = app.upgrade() else {
            return menu;
        };
        let this = entity.read(cx);
        // Taken before `this` is let go for the Tags submenu below.
        let tab_tags: Vec<crate::core::tag::TagId> = this
            .tabs
            .get(index)
            .map(|t| t.tags.borrow().clone())
            .unwrap_or_default();
        let tag_items: Vec<(crate::core::tag::TagId, String, crate::core::tag::TagColor, bool)> =
            this.sidebar_tags
                .tags
                .iter()
                .map(|tag| (tag.id, tag.name.clone(), tag.color, tab_tags.contains(&tag.id)))
                .collect();
        let tab_count = this.tabs.len();
        let cwd = this.tab_cwd_text(index, window, cx);
        let has_cwd = cwd.is_some();
        let mut menu = menu.min_w(px(200.));

        // Every item here acts on *this* tab, so the work is done by the click
        // handler and the action is carried only so `PopupMenu` can look its
        // chord up and print it. The handler wins when both are set. Without
        // this the tab menu was the one context menu in the app that taught no
        // shortcuts — right-clicking a pane offered "Split Right ⌘D" while
        // right-clicking its tab offered a bare "Split Right".
        menu = menu.item(
            PopupMenuItem::new(t(L10nKey::AppMenuRenameTab))
                .action(Box::new(RenameTab))
                .on_click({
                    let app = app.clone();
                    move |_, window, cx| {
                        let _ = app.update(cx, |this, cx| this.start_rename(index, window, cx));
                    }
                }),
        );

        let tab = this.tabs.get(index);
        if tab.is_some_and(|t| t.agent(cx).is_some()) {
            let done = tab.and_then(|t| t.agent_status(cx))
                == Some(crate::core::cli_agent::AgentStatus::Done);
            menu = menu.item(
                PopupMenuItem::new(t(L10nKey::TabContextMarkUnread))
                    .action(Box::new(MarkTabUnread))
                    .disabled(!done)
                    .on_click({
                        let app = app.clone();
                        move |_, _window, cx| {
                            let _ = app.update(cx, |this, cx| this.mark_tab_unread(index, cx));
                        }
                    }),
            );
        }

        // Sleep and wake (#762). Waking is also what selecting the tab does;
        // the item is here for waking one without leaving the tab on screen.
        // Hibernate is offered only where the machine can do it, and greyed
        // out for a tab it cannot be done to right now — the last one awake,
        // or one still connecting — rather than vanishing from under the
        // pointer that came looking for it.
        if tab.is_some_and(|t| t.is_asleep()) {
            menu = menu.item(PopupMenuItem::new(t(L10nKey::TabContextWake)).on_click({
                let app = app.clone();
                move |_, window, cx| {
                    let _ = app.update(cx, |this, cx| this.wake_tab(index, window, cx));
                }
            }));
        } else if crate::ui::remote::tree_sync::can_hibernate(cx) {
            menu = menu.item(
                PopupMenuItem::new(t(L10nKey::TabContextHibernate))
                    .action(Box::new(HibernateTab))
                    .disabled(!this.can_hibernate_tab(index, cx))
                    .on_click({
                        let app = app.clone();
                        move |_, window, cx| {
                            let _ =
                                app.update(cx, |this, cx| this.hibernate_tab(index, window, cx));
                        }
                    }),
            );
        }

        // Tags submenu: toggle any existing tag on this tab, or open the tag
        // popover to search/create one.
        {
            let app = app.clone();
            menu = menu.separator().submenu(
                t(L10nKey::SidebarTabTags),
                window,
                cx,
                move |sub, _window, _cx| {
                    let mut sub = sub.check_side(Side::Right);
                    for (tag_id, name, color, attached) in &tag_items {
                        let app = app.clone();
                        let tag_id = *tag_id;
                        let color = *color;
                        let label = name.clone();
                        let item = match crate::ui::workspace::tab_sidebar::tag_color_dot(color, _cx)
                        {
                            Some(dot) => PopupMenuItem::element(move |_window, _cx| {
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .size(px(8.))
                                            .rounded_full()
                                            .bg(dot),
                                    )
                                    .child(label.clone())
                            }),
                            None => PopupMenuItem::new(label),
                        };
                        sub = sub.item(item.checked(*attached).on_click(move |_, _window, cx| {
                            let _ = app.update(cx, |this, cx| {
                                this.toggle_tab_tag(index, tag_id, cx);
                            });
                        }));
                    }
                    if !tag_items.is_empty() {
                        sub = sub.separator();
                    }
                    let open = app.clone();
                    sub.item(
                        PopupMenuItem::new(t(L10nKey::SidebarNewTag))
                            .action(Box::new(EditTabTags))
                            .on_click(move |_, window, cx| {
                                let _ = open.update(cx, |this, cx| {
                                    this.open_tag_popover(index, window, cx);
                                });
                            }),
                    )
                },
            );
        }
        // Read again: the submenu above needed `cx` mutably.
        let this = entity.read(cx);

        let agent_session = this.tab_agent_session(index, window, cx);
        if let Some((source, session)) = &agent_session
            && let Some(label) = session.fork_label
        {
            menu = menu.separator();
            let forkable = session.forkable();
            menu = menu.item(
                PopupMenuItem::new(label)
                    .action(Box::new(ForkAgentSession))
                    .disabled(!forkable)
                    .on_click({
                        let app = app.clone();
                        let source = source.clone();
                        move |_, window, cx| {
                            let source = source.clone();
                            let _ = app.update(cx, |this, cx| {
                                this.fork_agent_session(
                                    index,
                                    source,
                                    crate::ui::workspace::app::ForkPlacement::NewTab,
                                    window,
                                    cx,
                                )
                            });
                        }
                    }),
            );
        }

        // The connection this tab is on, editable from the tab itself. A
        // hostname or password typed wrong used to be fixable only by finding
        // the same host again in Settings, and right-clicking the connection —
        // the gesture that asks "change this" — offered nothing (#438). The row
        // is the switcher machine menu's, word for word: the saved host when
        // there is one, an offer to keep the address when it was dialled by
        // hand, and nothing at all for a tab with no host form behind it.
        if let Some((form, label)) = this.tab_ssh_host_form(index, window, cx) {
            menu = menu.separator().item(PopupMenuItem::new(label).on_click({
                let app = app.clone();
                move |_, window, cx| {
                    let form = form.clone();
                    let _ = app.update(cx, |this, cx| {
                        this.open_tab_ssh_host_form(&form, window, cx)
                    });
                }
            }));
        }

        menu = menu
            .separator()
            .item(
                PopupMenuItem::new(t(L10nKey::AppMenuSplitRight))
                    .action(Box::new(SplitRight))
                    .on_click({
                        let app = app.clone();
                        move |_, window, cx| {
                            let _ = app.update(cx, |this, cx| {
                                this.activate(index, window, cx);
                                this.split_in(Axis::Horizontal, false, window, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(t(L10nKey::AppMenuSplitLeft))
                    .action(Box::new(SplitLeft))
                    .on_click({
                        let app = app.clone();
                        move |_, window, cx| {
                            let _ = app.update(cx, |this, cx| {
                                this.activate(index, window, cx);
                                this.split_in(Axis::Horizontal, true, window, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(t(L10nKey::AppMenuSplitDown))
                    .action(Box::new(SplitDown))
                    .on_click({
                        let app = app.clone();
                        move |_, window, cx| {
                            let _ = app.update(cx, |this, cx| {
                                this.activate(index, window, cx);
                                this.split_in(Axis::Vertical, false, window, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(t(L10nKey::AppMenuSplitUp))
                    .action(Box::new(SplitUp))
                    .on_click({
                        let app = app.clone();
                        move |_, window, cx| {
                            let _ = app.update(cx, |this, cx| {
                                this.activate(index, window, cx);
                                this.split_in(Axis::Vertical, true, window, cx);
                            });
                        }
                    }),
            );

        menu = menu.separator().item(
            PopupMenuItem::new(t(L10nKey::AppMenuCopyWorkingDirectory))
                .action(Box::new(CopyWorkingDirectory))
                .disabled(!has_cwd)
                .on_click(move |_, _window, cx| {
                    if let Some(text) = cwd.as_ref() {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                    }
                }),
        );

        if let Some(session_id) = agent_session.map(|(_, s)| s.session_id) {
            menu = menu.item(
                PopupMenuItem::new(t(L10nKey::AppMenuCopySessionId))
                    .action(Box::new(CopyAgentSessionId))
                    .disabled(session_id.is_none())
                    .on_click(move |_, _window, cx| {
                        if let Some(id) = session_id.as_ref() {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(id.clone()));
                        }
                    }),
            );
        }

        menu.separator()
            .item(
                PopupMenuItem::new(t(L10nKey::TabContextCloseTab))
                    .action(Box::new(CloseActiveTab))
                    .on_click({
                        let app = app.clone();
                        move |_, window, cx| {
                            let _ = app.update(cx, |this, cx| this.close_tab(index, window, cx));
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(t(L10nKey::AppMenuCloseOtherTabs))
                    .action(Box::new(CloseOtherTabs))
                    .disabled(tab_count <= 1)
                    .on_click({
                        let app = app.clone();
                        move |_, window, cx| {
                            let _ =
                                app.update(cx, |this, cx| this.close_other_tabs(index, window, cx));
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(if below_wording {
                    t(L10nKey::TabContextCloseTabsBelow)
                } else {
                    t(L10nKey::AppMenuCloseTabsRight)
                })
                .action(Box::new(CloseTabsToTheRight))
                .disabled(index + 1 >= tab_count)
                .on_click({
                    let app = app.clone();
                    move |_, window, cx| {
                        let _ =
                            app.update(cx, |this, cx| this.close_tabs_right_of(index, window, cx));
                    }
                }),
            )
    }

    pub(crate) fn tab_strip(
        &self,
        show_chips: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active = self.active;
        let show_badges = self.mod_hints.visible;
        // On macOS an open detail panel draws its own chrome in the title bar,
        // so the strip stops at the panel's edge rather than running the width
        // of the window. Sizing it to the whole viewport made it overrun that
        // edge, and what got pushed out past it was the New Tab button.
        let panel_w = match cfg!(target_os = "macos") && self.right_panel_open(cx) {
            true => self.right_panel_px(window, cx),
            false => 0.,
        };
        // A docked document column has the same claim on this strip the detail
        // panel does, and it is answered in the same two ways. On macOS the
        // strip lives *inside* the terminal column, so the column's width comes
        // off `strip_w` the way the panel's does — sizing the strip to more
        // than it gets is what pushed the New Tab button out before.
        // Everywhere else the strip spans the workspace and the column's header
        // is drawn over its trailing end, so the width is reserved as a corner
        // instead: that header carries no fill of its own, and a chip left
        // under it showed through the file name while staying clickable.
        let document_w = self.document_dock_px(window, cx).unwrap_or(0.);
        let controls_w = crate::ui::workspace::app::window_controls_w(window.is_fullscreen());
        let pad = gpui_kit::component::window_paddings(window);
        let strip_w = if cfg!(target_os = "macos") {
            strip_width(
                window.viewport_size().width,
                pad,
                crate::ui::workspace::app::TITLE_BAR_LEAD + panel_w + document_w,
                160.,
            )
        } else {
            strip_width(
                window.viewport_size().width,
                pad,
                crate::ui::workspace::app::TITLE_BAR_LEAD + controls_w,
                140.,
            )
        };
        let chrome_band_w = (!cfg!(target_os = "macos") && self.right_panel_open(cx))
            .then(|| (self.right_panel_px(window, cx) - controls_w - 1.).max(0.));
        // `corner_w` reserves the trailing window chrome. With the panel open on
        // macOS that chrome belongs to the panel's own header, which the strip
        // now stops short of, so reserving for it here would charge the chips
        // for it twice.
        // On macOS the trailing tiles sit at the end of the terminal column's
        // strip only while nothing stands to the right of it. A docked document
        // takes the window's right edge the way the panel does, so the tiles
        // give way to it instead of hanging in the middle of the window beside
        // the document's own header. ⌘J and the palette still reach both.
        let strip_chrome = !cfg!(target_os = "macos") || (panel_w <= 0. && document_w <= 0.);
        let corner_w = if !strip_chrome || panel_w > 0. {
            0.
        } else {
            chrome_band_w.unwrap_or_else(trailing_chrome_tiles_w)
        } + if cfg!(target_os = "macos") {
            // Already taken out of `strip_w` above; charging it here too would
            // narrow the chips by a column's width twice over.
            0.
        } else {
            document_w
        };
        let fixed_w = 3. * CHIP_GAP + crate::ui::workspace::app::TILE_SIZE + corner_w;
        let chips_avail = (strip_w - px(fixed_w + GRAB_HANDLE_W)).max(px(80.));
        let mut chips = h_flex()
            .items_center()
            .gap(px(CHIP_GAP))
            .min_w_0()
            .max_w(chips_avail)
            .overflow_hidden();

        // Held by the app rather than by the frame: a pane dropped up here has
        // to read the gaps between the chips, and it is asking a frame later
        // than the one that drew them. Blanked here and written again from
        // paint, so a chip that is not drawn this time — the strip is hidden,
        // or the chip scrolled out of it — leaves nothing behind to aim at.
        let slots = self.strip_slots.clone();
        *slots.borrow_mut() = vec![Bounds::default(); self.tabs.len()];
        let preview = reorder::preview(
            &self.reorder,
            &Surface::Strip,
            self.tabs.len(),
            window.mouse_position(),
        );
        let display: Vec<usize> = match &preview {
            Some(p) => {
                reorder::set_pending(&self.reorder, &Surface::Strip, p.order.clone());
                p.order.clone()
            }
            None => (0..self.tabs.len()).collect(),
        };
        let display = visible_chips(&display, active, f32::from(chips_avail));

        for i in display {
            if !show_chips {
                break;
            }
            let dragged = preview.as_ref().is_some_and(|p| p.from == i);
            let tab = &self.tabs[i];
            let is_active = i == active;
            let label = self.tab_label(tab, i, Some(window), cx);
            let full_title = self.tab_title_tooltip(tab, i, Some(window), cx);
            let ssh_dot = self.tab_ssh_dot(tab, cx);
            let asleep = tab.is_asleep();
            let agent = tab.agent(cx).or_else(|| tab.asleep_agent());
            let agent_status = tab.agent_status(cx);
            let agent_unread = tab.agent_unread_count(cx);
            let zoomed = self.tab_is_zoomed(i);

            let rename_input = self
                .renaming
                .as_ref()
                .filter(|r| r.tab == tab.tree_id.get())
                .map(|r| r.input.clone());
            let label_region = match rename_input {
                Some(input) => div()
                    .id(("tab-rename", i))
                    .flex_1()
                    .min_w_0()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // The chip switches tabs on the *release* now, so holding
                    // the press back is no longer enough: a click landing in
                    // the field would reach the chip behind it and switch away
                    // from the name being typed, taking the focus with it.
                    .on_click(|_, _, cx| cx.stop_propagation())
                    // No inset of its own: the label it replaces starts
                    // flush, and the field's 12px padding jumped the name
                    // sideways the moment rename began.
                    .child(Input::new(&input).appearance(false).px_0())
                    .into_any_element(),
                None => div()
                    .id(("tab-label", i))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .when(is_active, |d| d.font_weight(FontWeight::MEDIUM))
                    .when_some(full_title, |d, title| {
                        d.tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(title.clone()).build(window, cx)
                        })
                    })
                    .child(label)
                    .into_any_element(),
            };

            let chip = h_flex()
                .id(("tab-chip", i))
                .on_drag(DragTab, {
                    let state = self.reorder.clone();
                    let slots = slots.clone();
                    let id = tab.tree_id.get();
                    move |_drag, grab, _window, cx| {
                        cx.stop_propagation();
                        *state.borrow_mut() = Some(
                            Reorder::new(
                                Surface::Strip,
                                i,
                                slots.borrow().clone(),
                                Axis::Horizontal,
                                px(CHIP_GAP),
                                grab,
                            )
                            .of_tab(id),
                        );
                        cx.new(|_| DragTab)
                    }
                })
                .occlude()
                .group(SharedString::from(format!("tab-chip-{i}")))
                .cursor_pointer()
                .items_center()
                .justify_between()
                .gap_1p5()
                .h(px(30.))
                .min_w(px(CHIP_MIN_W))
                .flex_shrink(1.)
                .pl_3()
                .pr_1p5()
                .rounded_lg()
                .when(is_active, |s| {
                    s.bg(cx.theme().secondary).text_color(cx.theme().foreground)
                })
                .when(!is_active, |s| {
                    s.text_color(cx.theme().muted_foreground)
                        .hover(|s| s.bg(cx.theme().muted))
                })
                // Faded as well as marked: a row of chips is read at a glance,
                // and the tabs holding nothing should be the quiet ones.
                .when(asleep, |s| s.opacity(0.6))
                .when(dragged, |s| s.opacity(0.75))
                .child(
                    canvas(
                        {
                            let slots = slots.clone();
                            move |bounds, _window, _cx| {
                                if let Some(slot) = slots.borrow_mut().get_mut(i) {
                                    *slot = bounds;
                                }
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                // The press is kept from the title bar under it, but it is the
                // release that switches tabs: a press that turns into a drag is
                // the tab being picked up, and picking a tab up to drop it into
                // another one must not first put it on screen.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                    cx.stop_propagation();
                    let double =
                        matches!(ev, gpui::ClickEvent::Mouse(e) if e.down.click_count >= 2);
                    if double {
                        window.titlebar_double_click();
                    } else {
                        this.activate(i, window, cx);
                    }
                }))
                .when_some(ssh_dot, |c, rgb| {
                    c.child(
                        div()
                            .flex_shrink_0()
                            .size(px(6.))
                            .rounded_full()
                            .bg(gpui::rgb(rgb)),
                    )
                })
                .when_some(agent, |chip, agent| {
                    chip.child(self.tab_avatar(
                        ("tab-avatar", i),
                        Some(agent),
                        agent_status,
                        agent_unread,
                        None,
                        18.,
                        cx,
                    ))
                })
                // Leading, beside the other state marks: the trailing end of a
                // chip belongs to the badge and to the close button that fades
                // in over it, and a mark parked there would vanish under the
                // pointer that came to read it.
                .when(zoomed, |chip| {
                    chip.child(self.zoom_mark(("tab-zoom", i), cx))
                })
                .when(asleep, |chip| {
                    chip.child(self.sleep_mark(("tab-asleep", i), cx))
                })
                .child(label_region)
                .when(show_badges && i < 9, |chip| {
                    chip.child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(20.))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if is_active {
                                cx.theme().foreground
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(tab_badge_label(i)),
                    )
                })
                .when(!(show_badges && i < 9), |chip| {
                    let backing = if is_active {
                        cx.theme().secondary
                    } else {
                        cx.theme().muted
                    };
                    let mut fade_from = backing;
                    fade_from.a = 0.;
                    chip.child(
                        h_flex()
                            .absolute()
                            // 3 + MIN_TARGET + 3 centres the button in the 30px chip.
                            .top(px(3.))
                            .right(px(6.))
                            .opacity(0.)
                            .group_hover(SharedString::from(format!("tab-chip-{i}")), |s| {
                                s.opacity(1.)
                            })
                            .child(div().w(px(10.)).h(px(MIN_TARGET)).bg(linear_gradient(
                                90.,
                                linear_color_stop(fade_from, 0.),
                                linear_color_stop(backing, 1.),
                            )))
                            .child(
                                div().bg(backing).child(
                                    hit_target(
                                        Button::new(("tab-close", i))
                                            .icon(IconName::Close)
                                            .ghost()
                                            .xsmall(),
                                    )
                                    .tooltip(t(L10nKey::TabContextCloseTab))
                                    // Held here, because the chip behind it
                                    // switches tabs on the release too: without
                                    // this the same click closes tab `i` and
                                    // then activates whichever tab slid into
                                    // its place.
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.close_tab(i, window, cx);
                                        },
                                    )),
                                ),
                            ),
                    )
                });

            let menu_app = cx.entity().downgrade();
            let chip = chip.context_menu(move |menu, window, cx| {
                Self::tab_context_menu(menu, i, false, &menu_app, window, cx)
            });
            chips = chips.child(match &preview {
                Some(p) if p.from == i => deferred(chip.relative().left(p.held)).into_any_element(),
                Some(p) => {
                    let offset = p.offsets[i].as_f32();
                    chip.with_animation(
                        (
                            SharedString::from(format!("chip-slide-{}", p.generation)),
                            i,
                        ),
                        Animation::new(std::time::Duration::from_millis(REORDER_SLIDE_MS))
                            .with_easing(ease_out_quint()),
                        move |el, delta| el.left(px(offset * (1. - delta))),
                    )
                    .into_any_element()
                }
                None => chip.into_any_element(),
            });
        }

        let add_button = div()
            .occlude()
            .flex_shrink_0()
            .child(self.new_tab_button("tab-add", cx));

        // Same bargain the rail's own tiles keep when the Appearance switch is
        // on: present in the layout, painted only while the pointer is on the
        // bar. The New Tab button that trails the chips is left alone — it is
        // part of the row of tabs, not of the window's chrome.
        let auto_hide_chrome = cx.global::<Config>().auto_hide_titlebar_buttons;
        let strip_chrome_shown =
            crate::ui::workspace::app::titlebar_chrome_shown(auto_hide_chrome, self.strip_chrome_hover.get());
        let rail_collapsed = !show_chips && !self.left_panel_open(cx);
        let left_group = rail_collapsed.then(|| {
            h_flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(2.))
                .ml(px(crate::ui::workspace::app::title_bar_hug_offset()))
                .when_some(crate::ui::workspace::app::window_mark(), |group, mark| {
                    group.child(
                        div()
                            .flex_shrink_0()
                            .pl(px(crate::ui::workspace::app::CONTENT_INSET
                                - crate::ui::workspace::app::tile_trailing_inset()))
                            .pr(px(4.))
                            .child(mark),
                    )
                })
                // The logo above stays put: it is the window's mark, not a
                // control, and a window that loses its identity when nobody is
                // pointing at it reads as a different window.
                .child(
                    crate::ui::workspace::app::resting_chrome(
                        div().occlude().flex_shrink_0(),
                        strip_chrome_shown,
                    )
                    .child(self.new_tab_button("titlebar-add-collapsed", cx)),
                )
                .child(
                    crate::ui::workspace::app::resting_chrome(
                        div().occlude().flex_shrink_0(),
                        strip_chrome_shown,
                    )
                    .child(
                        chrome_tile(
                            Button::new("titlebar-expand-sidebar")
                                .icon(Icon::empty().path("icons/panel-left-collapsed.svg")),
                            false,
                            cx,
                        )
                        .rounded_lg()
                        .accessibility_label(t(L10nKey::TabTooltipShowSidebar))
                        .tooltip_with_action(
                            t(L10nKey::TabTooltipShowSidebar),
                            &crate::core::actions::ToggleLeftPanel,
                            None,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| this.toggle_left_panel(cx))),
                    ),
                )
        });

        // macOS places these controls in the open panel's own title bar, and
        // drops them while a docked document holds the right edge.
        //
        // Elsewhere, with the panel open, they stand in the band above it,
        // beside the panel's own tab row — which is painted whenever the panel
        // is, so a band that grew a button on hover read as a glitch next to
        // it. Once the panel is open they belong to its chrome, and stay.
        let right_chrome_shown = strip_chrome_shown || self.right_panel_open(cx);
        let right_chrome = strip_chrome.then(|| {
            crate::ui::workspace::app::resting_chrome(div().flex_shrink_0(), right_chrome_shown)
                .child(self.window_chrome(cx))
        });

        h_flex()
            .id("tab-strip")
            .relative()
            .items_center()
            .gap_1p5()
            .when(show_chips, |this| this.w(strip_w))
            .when(!show_chips, |this| this.w_full())
            .pl_0()
            .min_w_0()
            .when_some(left_group, |this, g| this.child(g))
            .child(chips)
            .when(show_chips, move |this| this.child(add_button))
            .child(div().flex_1().min_w(px(GRAB_HANDLE_W)))
            .when_some(right_chrome, |this, chrome| match chrome_band_w {
                Some(w) => this.child(
                    h_flex()
                        .flex_none()
                        .w(px(w))
                        .items_center()
                        .pl(px(tile_trailing_inset()))
                        .child(chrome),
                ),
                None => this.child(chrome),
            })
            .when(auto_hide_chrome, |this| {
                this.child(crate::ui::workspace::app::hover_sheet(
                    "strip-chrome-hover",
                    &self.strip_chrome_hover,
                ))
            })
    }
}

/// The tab menu's SSH row, against real tabs in a real window.
///
/// `PopupMenu` keeps its items to itself — nothing outside `gpui_component` can
/// read back what a built menu says — so these drive the predicate the menu
/// branches on instead, which is where every decision about the row is made.
///
/// Ungated: `test_window::harness` and `quiet_test_pane` both run on Windows,
/// and a `unix` gate here would skip the one platform this was written on.
fn avatar_disc(
    id: impl Into<gpui::ElementId>,
    crate::ui::chrome::search::Avatar {
        agent,
        status,
        unread,
        ssh,
    }: crate::ui::chrome::search::Avatar,
    size: f32,
    blink_on: bool,
    badge: bool,
    cx: &App,
) -> gpui::AnyElement {
    // The wrapper positions; the disc below carries the radius.
    // `status_dot` hangs itself off the edge with negative offsets — that
    // overhang is what makes it a badge on the avatar rather than a notch
    // in it — and as a child of the rounded element the overhang was
    // clipped along the arc, leaving a crescent.
    let base = div().id(id).flex_shrink_0().relative().size(px(size));
    // Fill, hairline and mark all live here, so the radius only ever clips
    // the disc's own paint.
    let disc = || {
        div()
            .size(px(size))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
    };
    match agent {
        Some(agent) => {
            let hollow = status == Some(crate::core::cli_agent::AgentStatus::Waiting);
            let dot = status
                .filter(|_| badge)
                .and_then(|s| s.dot_rgb())
                .map(|rgb| {
                    // A working agent's dot blinks, so a column of tabs
                    // says at a glance which ones are still going.
                    let faded =
                        status == Some(crate::core::cli_agent::AgentStatus::Working) && !blink_on;
                    AntermApp::status_dot(rgb, unread, size, cx.theme().background, hollow, faded)
                });
            // Which agent this is, and what it wants, were carried entirely
            // by a brand hue and a nine-pixel dot. Say it in words too.
            let tip = match agent_status_label(status) {
                Some(state) => format!("{} — {state}", agent.display_name()),
                None => agent.display_name().to_string(),
            };
            // The disc is a solid fill of the agent's brand on every
            // row, lit or not: a tint reads as a disabled tab, and the
            // colour is how the eye tells one agent from another down a
            // column of twenty.
            let accent = agent.accent_rgb();
            let surface = cx.theme().background;
            base.child(
                disc()
                    .bg(gpui::rgb(accent))
                    // Codex and Grok are both pure black, which is the
                    // window fill on a dark theme — the disc dissolves and
                    // leaves the glyph floating. A hairline keeps it a disc
                    // in any theme.
                    .when(crate::ui::chrome::presets::needs_edge(accent, surface), |d| {
                        d.border_1().border_color(cx.theme().border)
                    })
                    .child(
                        gpui::svg()
                            .path(agent.icon_path())
                            .size(px(size * 0.54))
                            // SVG assets render as a single-colour mask, so
                            // the mark's colour comes from the agent rather
                            // than from the file. The tray icon reads the
                            // same answer.
                            .text_color(gpui::rgb(agent.icon_rgb())),
                    ),
            )
            .when_some(dot, |b, dot| b.child(dot))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .into_any_element()
        }
        None => base
            .child(
                disc().bg(cx.theme().muted).child(
                    gpui::svg()
                        .path("icons/terminal.svg")
                        .size(px(size * 0.56))
                        .text_color(cx.theme().foreground.opacity(0.65)),
                ),
            )
            .when_some(ssh, |b, rgb| {
                b.child(AntermApp::status_dot(
                    rgb,
                    0,
                    size,
                    cx.theme().background,
                    false,
                    false,
                ))
            })
            .into_any_element(),
    }
}
