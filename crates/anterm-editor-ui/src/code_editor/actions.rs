use gpui::prelude::*;
use gpui::{AnyElement, Context, MouseButton, SharedString, Window, div, px, rems};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Editor, Input};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};

use super::{
    BarKind, BufferId, DiskConflict, OpenFile, TabCode, language_label, strip,
};
use anterm_ui_kit::window_chrome::{
    CONTENT_INSET, RAIL_TILE, RAIL_TILE_GLYPH, RAIL_TILE_RADIUS, TITLE_BAR_BORDER, TITLE_BAR_LEAD,
    TitleStripRow as _, chrome_tile_sized, overlay_surface_layers, title_bar_drag,
};
use crate::CodeEditorWorkspace;
use crate::document_column::DocumentChrome;
use anterm_i18n::{L10nKey, t, t_fmt};

impl CodeEditorWorkspace {
    pub fn render_code_overlay(
        &mut self,
        chrome: DocumentChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.code_panel_visible() {
            return None;
        }
        // The last file closed with the focus in it: keep the focus in the
        // panel, or gpui reports it lost and it falls back to the terminal.
        let tab = self.active_tab_id();
        if tab.is_some() && self.editor.had_focus == tab && self.active_buffer().is_none() {
            self.editor.empty_focus.focus(window, cx);
        }
        self.editor.had_focus = tab.filter(|_| self.editor_panel_has_focus(window, cx));
        self.editor_split_follow_focus(window, cx);
        self.editor_gutter_sync(cx);
        self.editor_nav_tick(cx);
        if let Some(code) = self.tab_code_mut() {
            code.note_front();
        }
        let breadcrumbs = self.render_editor_breadcrumbs(window, cx);
        let body = self.render_editor_body(
            self.tab_code().and_then(TabCode::active_id),
            self.tab_code().map_or(0, TabCode::focused_slot),
            cx,
        );
        let conflict_banner = self
            .active_buffer()
            .and_then(|f| f.conflict.map(|c| (f.id(), c)))
            .map(|(id, c)| self.render_editor_conflict_banner(id, c, cx));
        let bar = self.render_editor_bar(cx);

        let header = chrome
            .renders_own_header()
            .then(|| self.render_editor_header(chrome, window, cx));
        let editor_col = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .children(header)
            .when_some(conflict_banner, |this, b| this.child(b))
            .children(bar)
            .child(self.render_editor_groups(breadcrumbs, body, window, cx))
            .children(self.render_editor_problems(window, cx));

        // The panel's own paint is the same either way; only the box is not.
        // Filling the workspace means stopping the window's translucency and
        // repainting the theme image the root's copy now sits under; docking
        // means sitting in the same plane as the right panel, which the column
        // wrapper has already painted.
        let shell = v_flex().id("code-panel");
        let shell = match chrome {
            DocumentChrome::Fill => shell
                .absolute()
                .inset_0()
                .occlude()
                // Opaque on purpose: this overlay covers the whole workspace
                // (everything but the detail panel) and an open file must never
                // let the window translucency / backdrop material show through
                // it. The preset's gradient fill is preserved, just with
                // alpha 1 — the same paint the settings overlay uses. The
                // theme background image is repainted on top of it, since the
                // root's copy now sits below this fill.
                .bg(anterm_ui_kit::theme::overlay_background(cx))
                .children(overlay_surface_layers(cx)),
            DocumentChrome::Dock | DocumentChrome::DockHoisted => shell.size_full().min_w_0(),
        };
        let shell = self.editor_nav_actions(shell, cx);
        let shell = self.editor_split_command_sync(shell, cx);
        let shell = Self::with_editor_text_commands(shell, cx);
        Some(
            shell
                .on_action(cx.listener(
                    |this, _: &anterm_ui_kit::actions::EditorGoToLine, window, cx| {
                        this.editor_go_to_line(window, cx);
                    },
                ))
                .capture_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                    this.editor_handle_helix_key(ev, window, cx);
                }))
                .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                    if ev.keystroke.key != "escape" {
                        return;
                    }
                    if this.editor_gutter_close_peek(cx) {
                        cx.stop_propagation();
                        return;
                    }
                    // Escape in the go-to-line or Save As box dismisses the
                    // box, not the whole editor.
                    if this.editor.bar.is_some() {
                        this.editor_close_bar(window, cx);
                        cx.stop_propagation();
                        return;
                    }
                    // When Helix mode is enabled, Escape never closes the code
                    // editor panel; it belongs to modal editing.
                    if cx.global::<anterm_ui_kit::config::Config>().helix_mode {
                        cx.stop_propagation();
                        return;
                    }
                    this.toggle_code_panel(window, cx);
                }))
                .child(h_flex().flex_1().min_h_0().w_full().child(editor_col))
                .child(self.render_code_status_bar(window, cx))
                .into_any_element(),
        )
    }

    /// One group's text: the file `id`, rendered or as source, or the empty
    /// panel. `slot` tells the two groups' elements apart.
    pub fn render_editor_body(
        &self,
        id: Option<BufferId>,
        slot: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let helix_on = cx.global::<anterm_ui_kit::config::Config>().helix_mode;
        let is_focused_slot = slot == self.tab_code().map_or(0, TabCode::focused_slot);
        match id.and_then(|id| self.buffer(id)) {
            None => self.render_editor_empty(cx).into_any_element(),
            Some(f) if f.preview => {
                let markdown = f.input.read(cx).text().to_string();
                let scroll = f.preview_scroll.clone();
                let minor = f.helix.minor.filter(|_| helix_on && is_focused_slot);
                // The bar's wrapper takes its height from `flex_1`, so it needs
                // a column with a definite height to grow inside — hand it one
                // rather than dropping it straight into the overlay, or the
                // pane sizes to its content and there is nothing left to
                // scroll.
                v_flex()
                    .relative()
                    .size_full()
                    .when(is_focused_slot, |this| {
                        this.track_focus(&self.editor.empty_focus).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.editor.empty_focus.focus(window, cx);
                            }),
                        )
                    })
                    .child(anterm_ui_kit::scrollbar::with_vertical_scrollbar(
                        ("editor-md-preview-scrollbar", slot),
                        div()
                            .id(("editor-md-preview", slot))
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&scroll)
                            .px_4()
                            .py_3()
                            .child(
                                gpui_kit::component::text::TextView::markdown(
                                    ("editor-md-preview-body", slot),
                                    markdown,
                                )
                                .style(anterm_ui_kit::theme::markdown_style(cx)),
                            ),
                        &scroll,
                    ))
                    .when_some(minor, |this, m| {
                        this.child(Self::render_helix_minor_popup(m, cx))
                    })
                    .into_any_element()
            }
            Some(f) => {
                let input = f.input.clone();
                let id = f.id();
                let app = cx.entity().downgrade();
                let block_cursor = (helix_on && is_focused_slot && f.helix.mode.is_block_cursor())
                    .then(|| self.render_helix_block_cursor(f, cx))
                    .flatten();
                let minor = f.helix.minor.filter(|_| helix_on && is_focused_slot);
                div()
                    .id(("editor-body", slot))
                    .key_context(super::text_actions::CODE_EDITOR_CONTEXT)
                    .relative()
                    .size_full()
                    .child(
                        Editor::new(&input)
                            .appearance(false)
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(cx.theme().mono_font_size)
                            .size_full(),
                    )
                    .children(block_cursor)
                    .children(self.render_editor_gutter_peek(id, cx))
                    .when_some(minor, |this, m| {
                        this.child(Self::render_helix_minor_popup(m, cx))
                    })
                    .context_menu(move |menu, _window, cx| {
                        Self::editor_body_menu(menu, &app, id, cx)
                    })
                    .into_any_element()
            }
        }
    }

    fn render_helix_block_cursor(&self, f: &OpenFile, cx: &Context<Self>) -> Option<AnyElement> {
        let state = f.input.read(cx);
        let text = state.text();
        let len = text.len();
        let raw_head = if f.helix.anchor == f.helix.head && state.selected_range().is_empty() {
            state.cursor().min(len)
        } else {
            f.helix.head.min(len)
        };
        let range = anterm_ui_kit::helix_bridge::block_cursor_byte_range(text, raw_head);
        let bounds = state.range_to_bounds(&range)?;
        let width = bounds.size.width.max(px(8.));
        let height = bounds.size.height.max(px(16.));
        let caret_color = cx.theme().caret;
        Some(
            gpui::anchored()
                .position(bounds.origin)
                .child(
                    div()
                        .w(width)
                        .h(height)
                        .bg(caret_color.opacity(0.42))
                        .border_1()
                        .border_color(caret_color),
                )
                .into_any_element(),
        )
    }

    fn render_helix_minor_popup(
        minor: anterm_editor::helix::MinorMode,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let mono = theme.mono_font_family.clone();
        v_flex()
            .id("helix-minor-popup")
            .occlude()
            .absolute()
            .bottom(px(12.))
            .right(px(16.))
            .min_w(px(230.))
            .max_w(px(320.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius)
            .shadow_lg()
            .overflow_hidden()
            .child(
                h_flex()
                    .px_3()
                    .py_1p5()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.foreground)
                    .child(minor.title()),
            )
            .child(
                v_flex()
                    .px_3()
                    .py_2()
                    .gap_1()
                    .children(minor.hints().iter().map(|&(key, desc)| {
                        h_flex()
                            .items_center()
                            .gap_3()
                            .text_xs()
                            .child(
                                div()
                                    .flex_none()
                                    .min_w(px(56.))
                                    .font_family(mono.clone())
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.primary)
                                    .child(key),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_ellipsis()
                                    .text_color(theme.muted_foreground)
                                    .child(desc),
                            )
                    })),
            )
            .into_any_element()
    }

    /// The editor header alone, for the strip above a docked column.
    pub fn render_editor_header_only(
        &self,
        chrome: DocumentChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.render_editor_header(chrome, window, cx)
            .into_any_element()
    }

    fn render_editor_header(
        &self,
        chrome: DocumentChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        // `TITLE_BAR_LEAD` is the room macOS's traffic lights need. Only a
        // header that starts at the left edge of the window has them to clear,
        // and a docked column never does.
        let lead = if self.left_panel_open(cx) || chrome.is_dock() {
            CONTENT_INSET
        } else {
            TITLE_BAR_LEAD
        };
        let row = h_flex().id("editor-header");
        let row = if chrome.header_is_title_strip() {
            title_bar_drag(row, "editor-header", window, cx)
        } else {
            row
        };
        // v6 chrome: no rule under the bar — the header, the breadcrumbs and
        // the text are one plane — and only as many file cells as the column
        // has room for, the rest behind a `+N` list (see `strip`).
        let (tile, glyph) = (RAIL_TILE, RAIL_TILE_GLYPH);
        let cap = self.editor_strip_cap(chrome, cx);
        let (shown, hidden) = self
            .tab_code()
            .map(|c| c.strip_split(cap))
            .unwrap_or_default();
        let active_id = self.tab_code().and_then(TabCode::active_id);
        let files: Vec<(usize, SharedString, String, bool, bool)> = shown
            .into_iter()
            .filter_map(|(pos, id)| {
                let f = self.buffer(id)?;
                let tip = if f.untitled.is_some() {
                    f.label().to_string()
                } else {
                    f.path.display().to_string()
                };
                Some((pos, f.label(), tip, Some(id) == active_id, f.dirty))
            })
            .collect();
        let strip = if files.is_empty() {
            div()
                .min_w_0()
                .px_2()
                .text_ellipsis()
                .text_size(gpui::rems(strip::CELL_TEXT))
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(t(L10nKey::EditorNoFileOpen)))
                .into_any_element()
        } else {
            h_flex()
                .id("editor-file-tabs")
                .min_w_0()
                .gap_0p5()
                .overflow_hidden()
                .children(files.into_iter().map(|(pos, name, tip, active, dirty)| {
                    self.render_file_tab(pos, name, tip, active, dirty, cx)
                }))
                .into_any_element()
        };
        let back = self.render_document_terminal_button(chrome, window, cx);
        row.flex_none()
            // The border is spent on the box, not on the paint: this header
            // shows no rule, but its tiles sit in the band the traffic lights
            // hang over and have to keep the title bar's line.
            .title_strip_row(px(TITLE_BAR_BORDER), cx.theme().transparent)
            .gap_0p5()
            // The whole reserve, not the reserve less the first control's own
            // inset. The filled editor leads with the back pill, whose 8pt of
            // inner padding is *inside* its hover box — carving it out of here
            // starts that box 8pt early and consumes part of the gap reserved
            // after the traffic lights, even when the lead follows their inset.
            // Docked, the row leads with the file tabs instead, and the same
            // subtraction put their first
            // glyph at 4pt rather than the `CONTENT_INSET` this bar is built on.
            .pl(px(lead))
            // The glyph, not the tile, lands on `CONTENT_INSET`, the column
            // the file name starts on at the other end of the bar.
            .pr(px(CONTENT_INSET - (tile - glyph) / 2.))
            .children(back)
            .child(strip)
            .children(self.render_strip_overflow(&hidden, cap, cx))
            .child(
                div().occlude().flex_none().child(
                    chrome_tile_sized(
                        Button::new("editor-new-file").icon(Icon::new(IconName::Plus)),
                        tile,
                        glyph,
                        false,
                        cx,
                    )
                    .rounded(px(RAIL_TILE_RADIUS))
                    .tooltip(t(L10nKey::EditorNewFile))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.editor_new_file(window, cx);
                    })),
                ),
            )
            // Whatever is left of the bar stays a place to drag the window by.
            .child(div().flex_1().h_full())
            .child(self.document_layout_switch(chrome, cx))
            .child(
                div().occlude().flex_none().ml_1().child(
                    chrome_tile_sized(
                        Button::new("editor-panel-close").icon(Icon::new(IconName::Close)),
                        tile,
                        glyph,
                        false,
                        cx,
                    )
                    .rounded(px(RAIL_TILE_RADIUS))
                    .tooltip(t(L10nKey::EditorBackToTerminal))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_code_panel(window, cx);
                    })),
                ),
            )
    }

    /// One file in the header's strip: its name, and a slot that shows the
    /// unsaved dot at rest and the close button under the pointer — the dot
    /// says there is something to lose before the × offers to lose it.
    fn render_file_tab(
        &self,
        pos: usize,
        name: SharedString,
        tip: String,
        active: bool,
        dirty: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group: SharedString = format!("editor-file-tab-{pos}").into();
        let slot = 16.;
        let muted = cx.theme().muted_foreground;
        let close = div()
            .id(("editor-file-tab-close", pos))
            .role(gpui::Role::Button)
            .aria_label(format!("{} {name}", t(L10nKey::Close)))
            .flex_none()
            .size(px(slot))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .hover(|s| s.bg(cx.theme().muted))
            .child(Icon::new(IconName::Close).xsmall().text_color(muted))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.editor_close_file(pos, window, cx);
            }));
        // Unsaved is a state, not an alarm: the dot is in caption ink, the
        // same as the `+N` button's, not the warning hue.
        let slot_el = div()
            .flex_none()
            .size(px(slot))
            .flex()
            .items_center()
            .justify_center()
            .when(dirty, |d| {
                d.child(
                    div()
                        .group_hover(group.clone(), |s| s.invisible())
                        .size(px(6.))
                        .rounded_full()
                        .bg(muted),
                )
            });
        div()
            .id(("editor-file-tab", pos))
            .role(gpui::Role::Tab)
            .aria_label(match dirty {
                true => format!("{name}, {}", t(L10nKey::SettingsUnsaved)),
                false => name.to_string(),
            })
            .aria_selected(active)
            .group(group.clone())
            .occlude()
            .flex_none()
            .min_w_0()
            .max_w(px(strip::CELL_MAX_W))
            .h(px(strip::CELL_H))
            .flex()
            .items_center()
            .gap_1p5()
            .pl(gpui::rems(10. / 16.))
            .pr_1p5()
            .rounded(px(strip::CELL_RADIUS))
            .text_size(gpui::rems(strip::CELL_TEXT))
            .map(|d| match active {
                true => d
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(cx.theme().foreground)
                    .bg(cx.theme().sidebar_accent),
                false => d.text_color(muted).hover(|s| {
                    s.bg(gpui::rgb(
                        cx.global::<anterm_ui_kit::presets::Surfaces>().sidebar.hover,
                    ))
                }),
            })
            .child(
                div()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(name),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .size(px(slot))
                    .child(div().absolute().inset_0().child(slot_el))
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .when(!active, |d| d.invisible())
                            .when(dirty, |d| d.invisible())
                            .group_hover(group, |s| s.visible())
                            .child(close),
                    ),
            )
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.editor_activate(pos, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| {
                    this.editor_close_file(pos, window, cx);
                }),
            )
            .context_menu({
                let app = cx.entity().downgrade();
                move |menu, _window, cx| Self::editor_tab_menu(menu, &app, pos, cx)
            })
            .into_any_element()
    }

    fn render_editor_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bar = self.editor.bar.as_ref()?;
        let label = match bar.kind {
            BarKind::GoToLine => t(L10nKey::EditorGoToLine),
            BarKind::SaveAs { .. } => t(L10nKey::EditorSaveAs),
            BarKind::Rename { .. } => t(L10nKey::LspRenameSymbol),
            BarKind::Command => ":",
            BarKind::HelixSearch => "/",
            BarKind::HelixSplitSelect => "s",
        };
        Some(
            h_flex()
                .flex_none()
                .w_full()
                .items_center()
                .gap_2()
                .px(px(CONTENT_INSET))
                .py_1()
                .border_b_1()
                .border_color(cx.theme().sidebar_border)
                .text_sm()
                .child(
                    div()
                        .flex_none()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&bar.input).small()),
                )
                .into_any_element(),
        )
    }

    fn render_code_status_bar(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        // No path here: the breadcrumbs over the text show where the file is.
        let muted = cx.theme().muted_foreground;
        let active = self.active_buffer();
        let cursor: Option<SharedString> = active.map(|f| {
            let input = f.input.read(cx);
            let pos = input.cursor_position();
            let ln_col = t_fmt(
                L10nKey::EditorLnCol,
                &[
                    ("line", &(pos.line + 1).to_string()),
                    ("column", &(pos.character + 1).to_string()),
                ],
            );
            // Multi-cursor: the position is the primary caret's, the count
            // says there are more.
            match 1 + f.helix.extra_selections.len() {
                1 => ln_col.into(),
                n => {
                    let n = n.to_string();
                    format!(
                        "{ln_col} {}",
                        t_fmt(L10nKey::EditorSelections, &[("n", &n)])
                    )
                    .into()
                }
            }
        });
        let wrap: Option<bool> = active.map(|f| f.wrap);
        let is_markdown = active.is_some_and(|f| f.language() == "markdown");
        let preview = active.is_some_and(|f| f.preview);
        let indent: Option<SharedString> = active.map(|f| {
            let key = if f.indent.hard_tabs {
                L10nKey::EditorIndentTabs
            } else {
                L10nKey::EditorIndentSpaces
            };
            t_fmt(key, &[("n", &f.indent.size.to_string())]).into()
        });
        let line_ending: Option<&'static str> = active.map(|f| f.format.line_ending.label());
        let encoding: Option<SharedString> = active.map(|f| f.format.encoding_label().into());
        let language: Option<&'static str> = active.and_then(|f| language_label(f.language()));
        let (lsp_lead, lsp_tail) = match self.render_lsp_status(cx) {
            Some((el, true)) => (Some(el), None),
            Some((el, false)) => (None, Some(el)),
            None => (None, None),
        };
        let helix_on = cx.global::<anterm_ui_kit::config::Config>().helix_mode;
        let helix_badge = active.filter(|_| helix_on).map(|f| {
            let mode = f.helix.mode;
            let (bg, fg) = match mode {
                anterm_editor::helix::HelixMode::Normal => {
                    (cx.theme().primary.opacity(0.18), cx.theme().primary)
                }
                anterm_editor::helix::HelixMode::Insert => {
                    (cx.theme().success.opacity(0.18), cx.theme().success)
                }
                anterm_editor::helix::HelixMode::Select => {
                    (cx.theme().warning.opacity(0.20), cx.theme().warning)
                }
            };
            let pending = f.helix.pending_display();
            h_flex()
                .flex_none()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id("status-helix-mode")
                        .px_1p5()
                        .py(px(1.))
                        .rounded_sm()
                        .bg(bg)
                        .text_color(fg)
                        .font_family(cx.theme().mono_font_family.clone())
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(mode.badge()),
                )
                .when_some(pending, |this, p| {
                    this.child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(cx.theme().foreground)
                            .child(p),
                    )
                })
                .into_any_element()
        });

        // Metadata, not a toolbar: caption ink on the plane's own fill, set
        // off by a hairline in the divider tone rather than a control border.
        // The readouts that do something answer the pointer with a soft fill
        // and body ink; the rest are plain text in the same row.
        let fg = cx.theme().foreground;
        let hover = cx.theme().muted;
        let item = |id: &'static str, label: SharedString| {
            div()
                .id(id)
                .flex_none()
                .h(px(20.))
                .px_1()
                .mx(px(-4.))
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .hover(move |s| s.bg(hover).text_color(fg))
                .child(label)
        };
        h_flex()
            .flex_none()
            .w_full()
            .h(px(28.))
            .items_center()
            .gap(gpui::rems(14. / 16.))
            .px(px(CONTENT_INSET))
            .border_t(anterm_ui_kit::theme::hairline(window))
            .border_color(cx.theme().sidebar_border)
            .text_size(gpui::rems(11.5 / 16.))
            .text_color(muted)
            .whitespace_nowrap()
            .children(helix_badge)
            .children(lsp_lead)
            .child(div().flex_1())
            // The counts open the Problems list.
            .children(lsp_tail.map(|status| {
                div()
                    .id("status-problems")
                    .flex_none()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_editor_problems(cx)))
                    .child(status)
            }))
            .when(is_markdown, |this| {
                let label = if preview {
                    t(L10nKey::EditorEdit)
                } else {
                    t(L10nKey::EditorPreview)
                };
                this.child(
                    item("status-md-preview", label.into())
                        .on_click(cx.listener(|this, _, _w, cx| this.toggle_document_preview(cx))),
                )
            })
            .when_some(wrap, |this, wrap| {
                let label = if wrap {
                    t(L10nKey::EditorWrapOn)
                } else {
                    t(L10nKey::EditorWrapOff)
                };
                this.child(item("status-wrap", label.into()).on_click(
                    cx.listener(|this, _, window, cx| this.toggle_document_wrap(window, cx)),
                ))
            })
            // Tabular figures, so the position does not jitter sideways as
            // the caret walks from line 9 to line 10. A click asks for a line
            // to go to.
            .when_some(cursor, |this, t| {
                this.child(
                    item("status-cursor", t)
                        .font_features(anterm_ui_kit::theme::tabular_figures())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.editor_go_to_line(window, cx)),
                        ),
                )
            })
            .when_some(indent, |this, t| this.child(div().flex_none().child(t)))
            .when_some(encoding, |this, t| this.child(div().flex_none().child(t)))
            .when_some(line_ending, |this, eol| {
                this.child(
                    item("status-eol", eol.into())
                        .on_click(cx.listener(|this, _, _w, cx| this.toggle_line_ending(cx))),
                )
            })
            .when_some(language, |this, l| this.child(div().flex_none().child(l)))
    }

    pub(super) fn render_editor_empty(&self, cx: &Context<Self>) -> gpui::Div {
        v_flex()
            .track_focus(&self.editor.empty_focus)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.editor.empty_focus.focus(window, cx)),
            )
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                Icon::new(IconName::File)
                    .large()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(anterm_i18n::t(
                        anterm_i18n::L10nKey::OpenFileFromTree,
                    )),
            )
    }

    fn render_editor_conflict_banner(
        &self,
        id: BufferId,
        conflict: DiskConflict,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use anterm_ui_kit::dialog::{self, Tone};
        let theme = cx.theme();
        let rungs = cx.global::<anterm_ui_kit::presets::Surfaces>().window;
        // The floating notices' grammar, laid flat: a neutral strip over a
        // hairline, and the state carried by one amber dot. A strip tinted
        // amber end to end, with a white outlined button from the component
        // library beside a bare-text one, was three visual languages in one
        // 32px row.
        //
        // The filled answer is always the safe one: Keep mine / Save hold on
        // to the unsaved edits, Reload / Close throw them away, so those are
        // offered, not pressed on the reader.
        let row = h_flex()
            .flex_none()
            .w_full()
            .items_center()
            .gap_2()
            .pl(px(dialog::INSET))
            .pr_1p5()
            .h(px(dialog::FOOTER_H))
            .border_b_1()
            .border_color(theme.border)
            .text_size(rems(anterm_ui_kit::panel_tokens::TAB_TEXT))
            .child(
                div()
                    .flex_none()
                    .size(px(6.))
                    .rounded_full()
                    .bg(theme.warning),
            );
        let message = |key| div().flex_1().min_w_0().truncate().child(t(key));
        match conflict {
            DiskConflict::Changed(observed) => row
                .child(message(L10nKey::FileChangedOnDisk))
                .child(dialog::button(
                    "editor-conflict-reload",
                    t(L10nKey::Reload),
                    Tone::Secondary,
                    true,
                    rungs,
                    cx,
                    cx.listener(move |this, _, window, cx| {
                        this.editor_reload_from_disk(id, window, cx);
                    }),
                ))
                .child(dialog::button(
                    "editor-conflict-keep",
                    t(L10nKey::KeepMine),
                    Tone::Primary,
                    true,
                    rungs,
                    cx,
                    cx.listener(move |this, _, _w, cx| {
                        if let Some(f) = this.buffer_mut(id) {
                            // The version seen on disk is now the one a
                            // save is allowed to replace.
                            f.disk_mtime = observed;
                            f.conflict = None;
                            cx.notify();
                        }
                    }),
                ))
                .into_any_element(),
            DiskConflict::Deleted => row
                .child(message(L10nKey::EditorFileDeletedOnDisk))
                .child(dialog::button(
                    "editor-deleted-close",
                    t(L10nKey::Close),
                    Tone::Secondary,
                    true,
                    rungs,
                    cx,
                    cx.listener(move |this, _, _w, cx| {
                        this.editor_drop_buffer(id, cx);
                    }),
                ))
                .child(dialog::button(
                    "editor-deleted-save",
                    t(L10nKey::Save),
                    Tone::Primary,
                    true,
                    rungs,
                    cx,
                    cx.listener(move |this, _, window, cx| {
                        this.editor_save_file(id, false, true, window, cx);
                    }),
                ))
                .into_any_element(),
        }
    }
}
