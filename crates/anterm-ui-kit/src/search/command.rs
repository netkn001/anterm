//! What a palette or picker row runs, cleanly split between static global
//! commands ([`PaletteAction`], every one guaranteed a stable frecency id) and
//! dynamic picker row payloads ([`PickerTarget`]).

use gpui::{App, SharedString};

use uuid::Uuid;

use anterm_core::cli_agent::{AgentStatus, CLIAgent};
use crate::config::{Config, RightPanelTab, TabBarPosition};
use anterm_core::tag::{TagColor, TagId};
use anterm_i18n::{L10nKey, alias_translations, t};

/// Static global commands offered by the Command Palette. Every variant is
/// guaranteed at compile time to carry a stable frecency [`Self::id`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaletteAction {
    NewTab,
    NewWindow,
    CloseWindow,
    SplitRight,
    SplitLeft,
    SplitDown,
    SplitUp,
    ClosePane,
    RenameTab,
    EditTabTags,
    CloseOtherTabs,
    CloseTabsToTheRight,
    CopyWorkingDirectory,
    MarkTabUnread,
    HibernateTab,
    ForkAgentSession,
    CopyAgentSessionId,
    FocusNextAgentNeedingAttention,
    ResetFontSize,
    NextPane,
    PrevPane,
    FocusPaneLeft,
    FocusPaneRight,
    FocusPaneUp,
    FocusPaneDown,
    ResizePaneLeft,
    ResizePaneRight,
    ResizePaneUp,
    ResizePaneDown,
    SwapPaneNext,
    SwapPanePrev,
    SelectNextTab,
    SelectPrevTab,
    MoveTabLeft,
    MoveTabRight,
    ToggleMaximizePane,
    ToggleFullscreen,
    ToggleTabSidebar,
    ToggleLeftPanel,
    ToggleRightPanel,
    ShowRightPanel(RightPanelTab),
    ClearTerminal,
    FindInTerminal,
    ToggleComposer,
    FindNext,
    FindPrevious,
    CopyText,
    CopyRaw,
    CutText,
    PasteText,
    SelectAllText,
    ReopenClosedTab,
    OpenSettings,
    ShowKeyboardShortcuts,
    About,
    CheckForUpdates,
    OpenDocumentation,
    OpenDiscord,
    ReportIssue,
    Quit,
    RestartDaemon,
    UpdateLocalServer,
    ToggleSftp,
    ShowSshForwards,
    ToggleCodePanel,
    ToggleDocumentFill,
    DocumentWidthThird,
    DocumentWidthHalf,
    DocumentWidthTwoThirds,
    ToggleDocumentPreview,
    ToggleDocumentWrap,
    /// The code editor's text commands, run on the active buffer.
    EditorTransformUppercase,
    EditorTransformLowercase,
    EditorTransformTitleCase,
    EditorTrimTrailingWhitespace,
    EditorJoinLines,
    EditorRemoveSurroundingBrackets,
    RestartSshSession,
    ScmCommit,
    ScmStageAll,
    ScmUnstageAll,
    ScmDiscardAll,
    ScmPush,
    ScmPull,
    ScmFetch,
    ScmSync,
    ScmCreateBranch,
    OpenBranchPicker,
    ToggleDiffViewMode,
    SendSelectionToAgent,
    SendGitDiffToAgent,
    OpenThemePicker,
    /// Moves the palette to the open-tab switcher (`Switch Tab…`).
    OpenTabPicker,
    /// Moves the palette to the tag switcher (`Switch Tag…`).
    OpenTagPicker,
    /// Moves the palette to its Hosts picker, where typing an address connects.
    SearchHosts,
    /// Moves the palette to its Files picker — quick open by name.
    QuickOpenFile,
    /// The code editor's navigation, from the palette.
    EditorGoToSymbol,
    EditorNavigateBack,
    EditorNavigateForward,
    EditorSplitRight,
    SaveSshSessionAsHost,
    OpenSshProfiles,
}

impl PaletteAction {
    /// Stable frecency key for this command. Exhaustive by construction.
    pub const fn id(self) -> &'static str {
        use PaletteAction::*;
        match self {
            NewTab => "new-tab",
            NewWindow => "new-window",
            CloseWindow => "close-window",
            SplitRight => "split-right",
            SplitLeft => "split-left",
            SplitDown => "split-down",
            SplitUp => "split-up",
            ClosePane => "close-pane",
            RenameTab => "rename-tab",
            EditTabTags => "edit-tab-tags",
            CloseOtherTabs => "close-other-tabs",
            CloseTabsToTheRight => "close-tabs-right",
            CopyWorkingDirectory => "copy-cwd",
            MarkTabUnread => "mark-tab-unread",
            HibernateTab => "hibernate-tab",
            ForkAgentSession => "fork-agent-session",
            CopyAgentSessionId => "copy-agent-session-id",
            FocusNextAgentNeedingAttention => "focus-next-agent-needing-attention",
            ResetFontSize => "reset-font-size",
            NextPane => "next-pane",
            PrevPane => "prev-pane",
            FocusPaneLeft => "focus-pane-left",
            FocusPaneRight => "focus-pane-right",
            FocusPaneUp => "focus-pane-up",
            FocusPaneDown => "focus-pane-down",
            ResizePaneLeft => "resize-pane-left",
            ResizePaneRight => "resize-pane-right",
            ResizePaneUp => "resize-pane-up",
            ResizePaneDown => "resize-pane-down",
            SwapPaneNext => "swap-pane-next",
            SwapPanePrev => "swap-pane-prev",
            SelectNextTab => "next-tab",
            SelectPrevTab => "prev-tab",
            MoveTabLeft => "move-tab-left",
            MoveTabRight => "move-tab-right",
            ToggleMaximizePane => "zoom-pane",
            ToggleFullscreen => "full-screen",
            ToggleTabSidebar => "tab-bar-position",
            ToggleLeftPanel => "left-sidebar",
            ToggleRightPanel => "right-panel",
            ShowRightPanel(RightPanelTab::Info) => "right-panel-info",
            // Frecency is keyed by this string, so it stays `right-panel-changes`
            // even though the panel is now called Source Control.
            ShowRightPanel(RightPanelTab::Scm) => "right-panel-changes",
            ShowRightPanel(RightPanelTab::Files) => "right-panel-files",
            ShowRightPanel(RightPanelTab::Search) => "right-panel-search",
            ClearTerminal => "clear-scrollback",
            FindInTerminal => "find",
            ToggleComposer => "composer",
            FindNext => "find-next",
            FindPrevious => "find-previous",
            CopyText => "copy",
            CopyRaw => "copy-raw",
            CutText => "cut",
            PasteText => "paste",
            SelectAllText => "select-all",
            ReopenClosedTab => "reopen-closed-tab",
            OpenSettings => "settings",
            ShowKeyboardShortcuts => "keyboard-shortcuts",
            About => "about",
            CheckForUpdates => "check-for-updates",
            OpenDocumentation => "documentation",
            OpenDiscord => "discord",
            ReportIssue => "report-issue",
            Quit => "quit",
            RestartDaemon => "restart-daemon",
            UpdateLocalServer => "update-local-server",
            ToggleSftp => "ssh-remote-files",
            ShowSshForwards => "ssh-port-forwarding",
            ToggleCodePanel => "code-panel",
            ToggleDocumentFill => "document-fill",
            DocumentWidthThird => "document-width-third",
            DocumentWidthHalf => "document-width-half",
            DocumentWidthTwoThirds => "document-width-two-thirds",
            ToggleDocumentPreview => "document-preview",
            ToggleDocumentWrap => "document-wrap",
            EditorTransformUppercase => "editor-uppercase",
            EditorTransformLowercase => "editor-lowercase",
            EditorTransformTitleCase => "editor-title-case",
            EditorTrimTrailingWhitespace => "editor-trim-trailing-whitespace",
            EditorJoinLines => "editor-join-lines",
            EditorRemoveSurroundingBrackets => "editor-remove-brackets",
            EditorGoToSymbol => "editor-go-to-symbol",
            EditorNavigateBack => "editor-go-back",
            EditorNavigateForward => "editor-go-forward",
            EditorSplitRight => "editor-split-right",
            RestartSshSession => "ssh-reconnect",
            ScmCommit => "git-commit",
            ScmStageAll => "git-stage-all",
            ScmUnstageAll => "git-unstage-all",
            ScmDiscardAll => "git-discard-all",
            ScmPush => "git-push",
            ScmPull => "git-pull",
            ScmFetch => "git-fetch",
            ScmSync => "git-sync",
            ScmCreateBranch => "git-create-branch",
            OpenBranchPicker => "git-checkout",
            ToggleDiffViewMode => "diff-view-mode",
            SendSelectionToAgent => "agent-send-selection",
            SendGitDiffToAgent => "agent-send-diff",
            OpenThemePicker => "change-theme",
            OpenTabPicker => "switch-tab",
            OpenTagPicker => "switch-tag",
            SearchHosts => "ssh-add-connection",
            QuickOpenFile => "go-to-file",
            OpenSshProfiles => "ssh-manage-profiles",
            SaveSshSessionAsHost => "ssh-save-connection",
        }
    }

    pub fn key_spec(self, cx: &App) -> Option<String> {
        use PaletteAction::*;
        let inline =
            |spec: &str| -> Option<String> { cfg!(target_os = "macos").then(|| spec.to_string()) };
        match self {
            CopyText => return inline("secondary-c"),
            CutText => return inline("secondary-x"),
            PasteText => return inline("secondary-v"),
            SelectAllText => return inline("secondary-a"),
            // A fixed `CodeEditor` binding, not one of the keymap's slots.
            EditorJoinLines => return inline("ctrl-j"),
            _ => {}
        }
        let action = match self {
            NewTab => "NewTab",
            NewWindow => "NewWindow",
            CloseWindow => "CloseWindow",
            SplitRight => "SplitRight",
            SplitLeft => "SplitLeft",
            SplitDown => "SplitDown",
            SplitUp => "SplitUp",
            ClosePane => "CloseActiveTab",
            RenameTab => "RenameTab",
            EditTabTags => "EditTabTags",
            OpenTabPicker => "SwitchTab",
            OpenTagPicker => "SwitchTag",
            CloseOtherTabs => "CloseOtherTabs",
            CloseTabsToTheRight => "CloseTabsToTheRight",
            CopyWorkingDirectory => "CopyWorkingDirectory",
            MarkTabUnread => "MarkTabUnread",
            HibernateTab => "HibernateTab",
            ForkAgentSession => "ForkAgentSession",
            CopyAgentSessionId => "CopyAgentSessionId",
            FocusNextAgentNeedingAttention => "FocusNextAgentNeedingAttention",
            ResetFontSize => "ResetFontSize",
            NextPane => "FocusNextPane",
            PrevPane => "FocusPrevPane",
            FocusPaneLeft => "FocusPaneLeft",
            FocusPaneRight => "FocusPaneRight",
            FocusPaneUp => "FocusPaneUp",
            FocusPaneDown => "FocusPaneDown",
            ResizePaneLeft => "ResizePaneLeft",
            ResizePaneRight => "ResizePaneRight",
            ResizePaneUp => "ResizePaneUp",
            ResizePaneDown => "ResizePaneDown",
            SwapPaneNext => "SwapPaneNext",
            SwapPanePrev => "SwapPanePrev",
            // What the search runs is the plain next/previous step, not the
            // MRU switcher `NextTab` opens, so its chord hint is that one's.
            SelectNextTab => "SelectNextTab",
            SelectPrevTab => "SelectPrevTab",
            MoveTabLeft => "MoveTabLeft",
            MoveTabRight => "MoveTabRight",
            ToggleMaximizePane => "ToggleMaximizePane",
            ToggleFullscreen => "ToggleFullscreen",
            ToggleTabSidebar => "ToggleTabSidebar",
            ToggleLeftPanel => "ToggleLeftPanel",
            ToggleRightPanel => "ToggleRightPanel",
            ShowRightPanel(tab) => match tab {
                RightPanelTab::Info => "ShowRightPanelInfo",
                RightPanelTab::Scm => "ShowRightPanelChanges",
                RightPanelTab::Files => "ShowRightPanelFiles",
                RightPanelTab::Search => "ShowRightPanelSearch",
            },
            ClearTerminal => "ClearScrollback",
            FindInTerminal => "FindInTerminal",
            ToggleComposer => "ToggleComposer",
            FindNext => "FindNext",
            FindPrevious => "FindPrevious",
            ReopenClosedTab => "ReopenClosedTab",
            OpenSettings => "OpenSettings",
            ShowKeyboardShortcuts => "ShowKeyboardShortcuts",
            About => "About",
            CheckForUpdates => "CheckForUpdates",
            OpenDocumentation => "OpenDocumentation",
            OpenDiscord => "OpenDiscord",
            ReportIssue => "ReportIssue",
            Quit => "Quit",
            RestartDaemon => "RestartDaemon",
            ToggleSftp => "ToggleSftp",
            ShowSshForwards => "ShowSshForwards",
            ToggleCodePanel => "ToggleCodePanel",
            ToggleDocumentFill => "ToggleDocumentFill",
            DocumentWidthThird => "DocumentWidthThird",
            DocumentWidthHalf => "DocumentWidthHalf",
            DocumentWidthTwoThirds => "DocumentWidthTwoThirds",
            ToggleDocumentPreview => "ToggleDocumentPreview",
            ToggleDocumentWrap => "ToggleDocumentWrap",
            EditorTransformUppercase => "EditorTransformUppercase",
            EditorTransformLowercase => "EditorTransformLowercase",
            EditorTransformTitleCase => "EditorTransformTitleCase",
            EditorTrimTrailingWhitespace => "EditorTrimTrailingWhitespace",
            EditorJoinLines => "EditorJoinLines",
            EditorRemoveSurroundingBrackets => "EditorRemoveSurroundingBrackets",
            EditorGoToSymbol => "EditorGoToSymbol",
            EditorNavigateBack => "EditorNavigateBack",
            EditorNavigateForward => "EditorNavigateForward",
            EditorSplitRight => "EditorSplitRight",
            RestartSshSession => "RestartSshSession",
            OpenSshProfiles => "OpenSshProfiles",
            ScmCommit => "ScmCommit",
            ScmStageAll => "ScmStageAll",
            ScmUnstageAll => "ScmUnstageAll",
            ScmDiscardAll => "ScmDiscardAll",
            ScmPush => "ScmPush",
            ScmPull => "ScmPull",
            ScmFetch => "ScmFetch",
            ScmSync => "ScmSync",
            ScmCreateBranch => "ScmCreateBranch",
            OpenBranchPicker => "ScmCheckoutBranch",
            ToggleDiffViewMode => "ToggleDiffViewMode",
            QuickOpenFile => "QuickOpenFile",
            CopyRaw => "CopyRaw",
            CopyText
            | CutText
            | PasteText
            | SelectAllText
            | SendSelectionToAgent
            | SendGitDiffToAgent
            | UpdateLocalServer
            | OpenThemePicker
            | SearchHosts
            | SaveSshSessionAsHost => return None,
        };
        crate::keymap::effective_key(action, cx)
    }
}

/// Dynamic row payload produced by a focused picker (`Tabs`, `Tags`, `Themes`,
/// `Files`, `Symbols`, `Locations`, `Shells`, `Hosts`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PickerTarget {
    /// Jump to an open tab by its stable tree id.
    GoToTab(anterm_core::machine::TabId),
    /// Switch the active tag filter (`None` shows all tabs) and focus a matching tab.
    SelectTag(Option<TagId>),
    /// Apply a theme preset by index.
    SetTheme(usize),
    /// A file the Files picker found, opened in the editor — on `line` and
    /// `column` when the query named them (`main.rs:120:5`).
    OpenFile {
        path: std::path::PathBuf,
        line: Option<u32>,
        column: Option<u32>,
    },
    /// A Go to Symbol row: this 0-based place in the file in front.
    GoToSymbol { line: u32, column: u32 },
    /// A place a language server found: 0-based, the column in chars.
    GoToLocation {
        path: std::path::PathBuf,
        line: u32,
        column: u32,
    },
    /// Open the shell the window's inventory lists under this label.
    OpenShell(String),
    /// Connect with a typed `ssh` command line (`-p`, `-J`, an alias…).
    OpenSshConnect(String),
    ConnectSavedProfile(Uuid),
    EditSavedProfile(Uuid),
    QuickConnect(String),
    SaveQuickConnect(String),
}

impl PickerTarget {
    /// Whether this row can be opened in a split pane alongside the focused pane.
    pub fn can_split(&self) -> bool {
        matches!(
            self,
            PickerTarget::OpenShell(_) | PickerTarget::ConnectSavedProfile(_)
        )
    }

    pub fn edit_variant(&self) -> Option<PickerTarget> {
        match self {
            PickerTarget::ConnectSavedProfile(id) => Some(PickerTarget::EditSavedProfile(*id)),
            PickerTarget::QuickConnect(s) => Some(PickerTarget::SaveQuickConnect(s.clone())),
            _ => None,
        }
    }
}

/// Everything a row can carry once it is picked. `AntermApp::run_command` is
/// the one place these are carried out, whichever picker the row came from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CommandKind {
    Action(PaletteAction),
    Pick(PickerTarget),
}

impl From<PaletteAction> for CommandKind {
    fn from(action: PaletteAction) -> Self {
        CommandKind::Action(action)
    }
}

impl From<PickerTarget> for CommandKind {
    fn from(target: PickerTarget) -> Self {
        CommandKind::Pick(target)
    }
}

impl CommandKind {
    pub fn can_split(&self) -> bool {
        matches!(self, CommandKind::Pick(t) if t.can_split())
    }

    pub fn edit_variant(&self) -> Option<CommandKind> {
        match self {
            CommandKind::Pick(t) => t.edit_variant().map(CommandKind::Pick),
            CommandKind::Action(_) => None,
        }
    }

    pub fn id(&self) -> Option<&'static str> {
        match self {
            CommandKind::Action(a) => Some(a.id()),
            CommandKind::Pick(_) => None,
        }
    }

    pub fn key_spec(&self, cx: &App) -> Option<String> {
        match self {
            CommandKind::Action(a) => a.key_spec(cx),
            CommandKind::Pick(_) => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommandGroup {
    TabsPanes,
    View,
    Git,
    Terminal,
    Ssh,
    Agents,
    Application,
}

impl CommandGroup {
    pub const ORDER: [CommandGroup; 7] = [
        CommandGroup::TabsPanes,
        CommandGroup::View,
        CommandGroup::Git,
        CommandGroup::Terminal,
        CommandGroup::Ssh,
        CommandGroup::Agents,
        CommandGroup::Application,
    ];

    pub fn title(self) -> &'static str {
        match self {
            CommandGroup::TabsPanes => t(L10nKey::CmdGroupTabsPanes),
            CommandGroup::View => t(L10nKey::CmdGroupView),
            CommandGroup::Git => t(L10nKey::CmdGroupGit),
            CommandGroup::Terminal => t(L10nKey::CmdGroupTerminal),
            CommandGroup::Ssh => t(L10nKey::CmdGroupSsh),
            CommandGroup::Agents => t(L10nKey::CmdGroupAgents),
            CommandGroup::Application => t(L10nKey::CmdGroupApplication),
        }
    }
}

#[derive(Clone, Default)]
pub struct ChromeState {
    pub rail_collapsed: bool,
    pub right_panel_visible: bool,
    /// Whether the *active tab's* document is filling the window.
    pub document_filled: bool,
    /// Whether a code / document panel or diff overlay is currently open.
    pub code_panel_open: bool,
    /// Whether the active pane/window is in a Git repository.
    pub has_git_repo: bool,
    /// Whether the focused pane is an active SSH session.
    pub has_ssh_session: bool,
    /// Whether any tab currently runs a coding agent.
    pub has_running_agent: bool,
}

#[cfg(test)]
impl ChromeState {
    pub fn all_contexts() -> Self {
        Self {
            rail_collapsed: false,
            right_panel_visible: false,
            document_filled: false,
            code_panel_open: true,
            has_git_repo: true,
            has_ssh_session: true,
            has_running_agent: true,
        }
    }
}

/// A terminal row's avatar: the same disc, brand and status dot the tab
/// strip draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Avatar {
    pub agent: Option<CLIAgent>,
    pub status: Option<AgentStatus>,
    pub unread: usize,
    pub ssh: Option<u32>,
}

/// One row of the command palette or a focused quick-picker.
#[derive(Clone)]
pub struct Item {
    pub title: String,
    pub subtitle: Option<String>,
    /// Muted text at the row's right edge (e.g. `line 120` in Go to File).
    pub note: Option<String>,
    /// Text the query may match but the row never shows: other locales' labels
    /// and the stable command id.
    pub aliases: Vec<SharedString>,
    pub kind: CommandKind,
    /// Actions only: the group that orders commands in browse mode.
    pub group: Option<CommandGroup>,
    /// Optional section header for pickers that group rows.
    pub section: Option<SharedString>,
    /// Optional semantic tag dot color rendered beside the row title.
    pub tag_color: Option<TagColor>,
}

impl Item {
    pub fn new(title: impl Into<String>, kind: impl Into<CommandKind>) -> Self {
        let kind = kind.into();
        Self {
            title: title.into(),
            subtitle: None,
            note: None,
            aliases: kind.id().into_iter().map(SharedString::from).collect(),
            kind,
            group: None,
            section: None,
            tag_color: None,
        }
    }

    pub fn with_tag_color(mut self, color: TagColor) -> Self {
        self.tag_color = Some(color);
        self
    }

    /// A command whose label comes out of the locale table, and which can
    /// therefore also be found by the wording any other locale would show.
    pub fn localized(key: L10nKey, action: PaletteAction) -> Self {
        Self::new(t(key), CommandKind::Action(action)).with_aliases_of(key)
    }

    /// Also findable by the wording every other locale gives `key`.
    pub fn with_aliases_of(mut self, key: L10nKey) -> Self {
        self.aliases
            .extend(alias_translations(key).into_iter().map(SharedString::from));
        self
    }

    pub fn with_alias(mut self, alias: impl Into<SharedString>) -> Self {
        self.aliases.push(alias.into());
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn with_subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn in_group(mut self, group: CommandGroup) -> Self {
        self.group = Some(group);
        self.section = Some(group.title().into());
        self
    }

    /// The actions every window offers, in group order. The window adds the
    /// ones only it can judge (`AntermApp::search_catalog`).
    pub fn actions(cx: &App, chrome: ChromeState) -> Vec<Item> {
        use PaletteAction::*;
        let cfg = cx.global::<Config>();
        let tab_bar_left = cfg.tab_bar_position == TabBarPosition::Left;
        let sidebar_hidden = chrome.rail_collapsed || !tab_bar_left;
        let right_panel_open = chrome.right_panel_visible;
        let document_filled = chrome.document_filled;

        let mut tabs = vec![
            Item::localized(L10nKey::CmdNewTab, NewTab),
            Item::localized(L10nKey::CmdSwitchTab, OpenTabPicker)
                .with_subtitle(t(L10nKey::CmdSwitchTabSubtitle)),
            Item::localized(L10nKey::CmdSwitchTag, OpenTagPicker)
                .with_subtitle(t(L10nKey::CmdSwitchTagSubtitle)),
            Item::localized(L10nKey::CmdRenameTab, RenameTab),
            Item::localized(L10nKey::CmdEditTabTags, EditTabTags)
                .with_subtitle(t(L10nKey::CmdEditTabTagsSubtitle))
                .with_aliases_of(L10nKey::SidebarRenameTag)
                .with_aliases_of(L10nKey::SidebarDeleteTag)
                .with_aliases_of(L10nKey::SidebarTagColor)
                .with_alias("manage tags")
                .with_alias("管理标签"),
            Item::localized(L10nKey::CmdSplitRight, SplitRight),
            Item::localized(L10nKey::CmdSplitLeft, SplitLeft),
            Item::localized(L10nKey::CmdSplitDown, SplitDown),
            Item::localized(L10nKey::CmdSplitUp, SplitUp),
            Item::localized(L10nKey::CmdZoomPane, ToggleMaximizePane),
            Item::localized(L10nKey::CmdNextPane, NextPane),
            Item::localized(L10nKey::CmdPreviousPane, PrevPane),
            Item::localized(L10nKey::CmdFocusPaneLeft, FocusPaneLeft),
            Item::localized(L10nKey::CmdFocusPaneRight, FocusPaneRight),
            Item::localized(L10nKey::CmdFocusPaneUp, FocusPaneUp),
            Item::localized(L10nKey::CmdFocusPaneDown, FocusPaneDown),
            Item::localized(L10nKey::CmdResizePaneLeft, ResizePaneLeft),
            Item::localized(L10nKey::CmdResizePaneRight, ResizePaneRight),
            Item::localized(L10nKey::CmdResizePaneUp, ResizePaneUp),
            Item::localized(L10nKey::CmdResizePaneDown, ResizePaneDown),
            Item::localized(L10nKey::CmdSwapPaneNext, SwapPaneNext),
            Item::localized(L10nKey::CmdSwapPanePrevious, SwapPanePrev),
            Item::localized(L10nKey::CmdNextTab, SelectNextTab),
            Item::localized(L10nKey::CmdPreviousTab, SelectPrevTab),
            Item::localized(L10nKey::CmdMoveTabLeft, MoveTabLeft),
            Item::localized(L10nKey::CmdMoveTabRight, MoveTabRight),
            Item::localized(L10nKey::CmdCopyWorkingDirectory, CopyWorkingDirectory),
        ];
        if chrome.has_running_agent {
            tabs.push(
                Item::localized(L10nKey::CmdCopySessionId, CopyAgentSessionId)
                    .with_subtitle(t(L10nKey::CmdCopySessionIdSubtitle)),
            );
            tabs.push(
                Item::localized(L10nKey::CmdForkSession, ForkAgentSession)
                    .with_subtitle(t(L10nKey::CmdForkSessionSubtitle)),
            );
        }
        tabs.extend([
            Item::localized(L10nKey::CmdMarkTabAsUnread, MarkTabUnread),
            Item::localized(L10nKey::CmdHibernateTab, HibernateTab)
                .with_subtitle(t(L10nKey::CmdHibernateTabSubtitle)),
            Item::localized(L10nKey::CmdClosePaneTab, ClosePane),
            Item::localized(L10nKey::CmdCloseOtherTabs, CloseOtherTabs),
            Item::localized(L10nKey::CmdCloseTabsToTheRight, CloseTabsToTheRight),
            Item::localized(L10nKey::CmdReopenClosedTab, ReopenClosedTab),
        ]);

        let mut view = vec![
            Item::localized(
                if sidebar_hidden {
                    L10nKey::CmdShowLeftSidebar
                } else {
                    L10nKey::CmdHideLeftSidebar
                },
                ToggleLeftPanel,
            ),
            Item::localized(
                if right_panel_open {
                    L10nKey::CmdHideRightPanel
                } else {
                    L10nKey::CmdShowRightPanel
                },
                ToggleRightPanel,
            ),
            Item::localized(L10nKey::CmdShowCodePanel, ToggleCodePanel),
            Item::localized(L10nKey::CmdGoToFile, QuickOpenFile),
        ];
        if chrome.code_panel_open {
            view.extend([
                Item::localized(
                    if document_filled {
                        L10nKey::CmdDocumentDock
                    } else {
                        L10nKey::CmdDocumentFill
                    },
                    ToggleDocumentFill,
                ),
                Item::localized(L10nKey::CmdDocumentWidthThird, DocumentWidthThird),
                Item::localized(L10nKey::CmdDocumentWidthHalf, DocumentWidthHalf),
                Item::localized(L10nKey::CmdDocumentWidthTwoThirds, DocumentWidthTwoThirds),
                Item::localized(L10nKey::CmdToggleDocumentPreview, ToggleDocumentPreview),
                Item::localized(L10nKey::CmdToggleDocumentWrap, ToggleDocumentWrap),
            ]);
        }
        view.extend([
            Item::localized(
                if tab_bar_left {
                    L10nKey::CmdTabBarMoveToTop
                } else {
                    L10nKey::CmdTabBarMoveToLeftSidebar
                },
                ToggleTabSidebar,
            ),
            Item::localized(
                L10nKey::CmdRightPanelInfo,
                ShowRightPanel(RightPanelTab::Info),
            ),
            Item::localized(
                L10nKey::CmdRightPanelFiles,
                ShowRightPanel(RightPanelTab::Files),
            ),
            Item::localized(
                L10nKey::CmdRightPanelSearch,
                ShowRightPanel(RightPanelTab::Search),
            ),
            Item::localized(
                L10nKey::CmdRightPanelChanges,
                ShowRightPanel(RightPanelTab::Scm),
            ),
            Item::localized(L10nKey::CmdChangeTheme, OpenThemePicker),
            Item::localized(L10nKey::CmdResetFontSize, ResetFontSize),
            Item::localized(L10nKey::CmdEnterFullScreen, ToggleFullscreen),
        ]);
        if chrome.has_git_repo {
            view.push(Item::localized(
                L10nKey::CmdToggleDiffViewMode,
                ToggleDiffViewMode,
            ));
        }

        // Their own group rather than more entries under View: View is a list
        // of things to show and hide, and ten git verbs in it would drown that.
        let git: Vec<Item> = if chrome.has_git_repo {
            vec![
                Item::localized(L10nKey::CmdGitCommit, ScmCommit),
                Item::localized(L10nKey::CmdGitStageAll, ScmStageAll),
                Item::localized(L10nKey::CmdGitUnstageAll, ScmUnstageAll),
                Item::localized(L10nKey::CmdGitDiscardAll, ScmDiscardAll)
                    .with_subtitle(t(L10nKey::CmdGitDiscardAllSubtitle)),
                Item::localized(L10nKey::CmdGitCheckoutTo, OpenBranchPicker),
                Item::localized(L10nKey::CmdGitCreateBranch, ScmCreateBranch),
                Item::localized(L10nKey::CmdGitSync, ScmSync)
                    .with_subtitle(t(L10nKey::CmdGitSyncSubtitle)),
                Item::localized(L10nKey::CmdGitPush, ScmPush),
                Item::localized(L10nKey::CmdGitPull, ScmPull),
                Item::localized(L10nKey::CmdGitFetch, ScmFetch),
            ]
        } else {
            Vec::new()
        };

        let terminal = [
            Item::localized(L10nKey::CmdClearScrollback, ClearTerminal),
            Item::localized(L10nKey::CmdFindInTerminal, FindInTerminal),
            Item::localized(L10nKey::CmdToggleComposer, ToggleComposer),
            Item::localized(L10nKey::CmdFindNext, FindNext),
            Item::localized(L10nKey::CmdFindPrevious, FindPrevious),
            Item::localized(L10nKey::CmdCopy, CopyText),
            Item::localized(L10nKey::CmdCopyRaw, CopyRaw),
            Item::localized(L10nKey::CmdCut, CutText),
            Item::localized(L10nKey::CmdPaste, PasteText),
            Item::localized(L10nKey::CmdSelectAll, SelectAllText),
        ];

        let mut ssh = vec![
            Item::localized(L10nKey::CmdSshAddConnection, SearchHosts),
            Item::localized(L10nKey::CmdSshManageProfiles, OpenSshProfiles),
        ];
        if chrome.has_ssh_session {
            ssh.extend([
                Item::localized(L10nKey::CmdSshReconnect, RestartSshSession),
                // The panel is an SFTP browser, and that is the word people search.
                Item::localized(L10nKey::CmdSshRemoteFiles, ToggleSftp).with_alias("SFTP"),
                Item::localized(L10nKey::CmdSshPortForwarding, ShowSshForwards),
            ]);
        }

        let agents: Vec<Item> = if chrome.has_running_agent {
            vec![
                Item::localized(
                    L10nKey::CmdNextAgentNeedingAttention,
                    FocusNextAgentNeedingAttention,
                )
                .with_subtitle(t(L10nKey::CmdNextAgentNeedingAttentionSubtitle)),
                Item::localized(L10nKey::CmdAgentSendSelection, SendSelectionToAgent)
                    .with_subtitle(t(L10nKey::CmdAgentSendSelectionSubtitle)),
                Item::localized(L10nKey::CmdAgentSendGitDiffForReview, SendGitDiffToAgent)
                    .with_subtitle(t(L10nKey::CmdAgentSendGitDiffSubtitle)),
            ]
        } else {
            Vec::new()
        };

        let application = [
            Item::localized(L10nKey::CmdNewWindow, NewWindow),
            Item::localized(L10nKey::CmdSettings, OpenSettings),
            Item::localized(L10nKey::CmdKeyboardShortcuts, ShowKeyboardShortcuts),
            Item::localized(L10nKey::CmdAboutAnterm, About),
            Item::localized(L10nKey::CmdCheckForUpdates, CheckForUpdates),
            Item::localized(L10nKey::CmdDocumentation, OpenDocumentation),
            Item::localized(L10nKey::CmdJoinDiscord, OpenDiscord),
            Item::localized(L10nKey::CmdReportIssue, ReportIssue),
            Item::localized(L10nKey::CmdRestartServer, RestartDaemon)
                .with_subtitle(t(L10nKey::CmdRestartServerSubtitle)),
            Item::localized(L10nKey::CmdUpdateLocalServer, UpdateLocalServer)
                .with_subtitle(t(L10nKey::CmdUpdateLocalServerSubtitle)),
            // Beside Quit, because the pair is the whole point of the action:
            // both end the window you are looking at, and only one of them
            // takes your shells with it. Read together the subtitles say which.
            Item::localized(L10nKey::CmdCloseWindow, CloseWindow)
                .with_subtitle(t(L10nKey::CmdCloseWindowSubtitle)),
            Item::localized(L10nKey::CmdQuitAnterm, Quit)
                .with_subtitle(t(L10nKey::CmdQuitAntermSubtitle)),
        ];

        let mut out = Vec::new();
        let mut push = |cmds: Vec<Item>, group: CommandGroup| {
            out.extend(cmds.into_iter().map(|c| c.in_group(group)));
        };
        push(tabs, CommandGroup::TabsPanes);
        push(view, CommandGroup::View);
        push(git, CommandGroup::Git);
        push(terminal.into(), CommandGroup::Terminal);
        push(ssh, CommandGroup::Ssh);
        push(agents, CommandGroup::Agents);
        push(application.into(), CommandGroup::Application);
        out
    }

    /// The theme picker's rows, the one in use checked.
    pub fn themes(cx: &App) -> Vec<Item> {
        let active = crate::theme::effective_preset_id(cx);
        crate::presets::all(cx)
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let title = if p.id == active {
                    format!("{}  ✓", p.name)
                } else {
                    p.name.clone()
                };
                Item::new(title, PickerTarget::SetTheme(i))
            })
            .collect()
    }

    /// Row of the preset already in use, so the theme picker can open on it
    /// instead of previewing something else the moment it is opened.
    pub fn active_theme_index(cx: &App) -> Option<usize> {
        let active = crate::theme::effective_preset_id(cx);
        crate::presets::all(cx)
            .iter()
            .position(|p| p.id == active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for action in [
            PaletteAction::NewTab,
            PaletteAction::OpenTabPicker,
            PaletteAction::OpenTagPicker,
            PaletteAction::SplitRight,
            PaletteAction::SplitLeft,
            PaletteAction::SplitDown,
            PaletteAction::SplitUp,
            PaletteAction::ClearTerminal,
            PaletteAction::CopyText,
            PaletteAction::CopyRaw,
            PaletteAction::CutText,
            PaletteAction::PasteText,
            PaletteAction::SelectAllText,
            PaletteAction::FindInTerminal,
            PaletteAction::ToggleComposer,
            PaletteAction::FindNext,
            PaletteAction::FindPrevious,
            PaletteAction::OpenSettings,
            PaletteAction::ShowKeyboardShortcuts,
            PaletteAction::About,
            PaletteAction::Quit,
            PaletteAction::ShowRightPanel(RightPanelTab::Info),
            PaletteAction::ShowRightPanel(RightPanelTab::Files),
            PaletteAction::ShowRightPanel(RightPanelTab::Search),
        ] {
            let id = action.id();
            assert!(seen.insert(id), "duplicate command id {id:?}");
        }
    }

    /// The picker previews whatever row is highlighted, so it has to open on
    /// the row of the theme already in use — the one carrying the check mark.
    #[gpui::test]
    fn the_theme_picker_opens_on_the_theme_in_use(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let themes = crate::presets::all(cx);
            let last = themes.last().expect("built-in themes").id.clone();
            cx.set_global(Config(crate::config::CoreConfig {
                theme_follow_system: false,
                theme_preset: last,
                ..Default::default()
            }));

            let ix = Item::active_theme_index(cx).expect("the live preset is listed");
            assert_eq!(ix, themes.len() - 1);
            let rows = Item::themes(cx);
            assert!(
                rows[ix].title.ends_with('✓'),
                "row {ix} ({:?}) should be the checked one",
                rows[ix].title
            );
        });
    }

    #[test]
    fn dynamic_commands_have_no_id() {
        assert!(CommandKind::Pick(PickerTarget::SetTheme(0)).id().is_none());
        assert!(
            CommandKind::Pick(PickerTarget::QuickConnect("a@b".into()))
                .id()
                .is_none()
        );
        assert!(
            CommandKind::Pick(PickerTarget::OpenShell("zsh".into()))
                .id()
                .is_none()
        );
    }
}

#[cfg(test)]
mod gpui_tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn every_action_has_a_stable_id(cx: &mut TestAppContext) {
        crate::config::pin_test_config_dir();
        cx.update(|cx| {
            cx.set_global(Config::default());
            anterm_i18n::set_locale("en");
            let chrome = ChromeState::all_contexts();
            let mut seen = std::collections::HashSet::new();
            for cmd in Item::actions(cx, chrome) {
                let id = cmd
                    .kind
                    .id()
                    .unwrap_or_else(|| panic!("`{}` has no stable id", cmd.title));
                assert!(seen.insert(id), "two commands claim the id {id:?}");
            }
        });
    }

    #[gpui::test]
    fn the_git_group_is_its_own_section(cx: &mut TestAppContext) {
        crate::config::pin_test_config_dir();
        cx.update(|cx| {
            cx.set_global(Config::default());
            anterm_i18n::set_locale("en");
            let chrome = ChromeState::all_contexts();
            let cmds = Item::actions(cx, chrome);
            let git = cmds
                .iter()
                .filter(|c| c.group == Some(CommandGroup::Git))
                .count();
            assert_eq!(git, 10, "the git section should hold ten verbs");
            assert!(!cmds.iter().any(|c| c.group == Some(CommandGroup::View)
                && c.kind.id().unwrap_or("").starts_with("git-")),);

            // Bare local terminal omits inapplicable Git/SSH-session/Agent/Document commands.
            let bare = Item::actions(cx, ChromeState::default());
            assert!(bare.iter().all(|c| c.group != Some(CommandGroup::Git)));
            assert!(bare.iter().all(|c| c.group != Some(CommandGroup::Agents)));
            assert!(
                bare.iter()
                    .all(|c| c.kind != CommandKind::Action(PaletteAction::ToggleSftp))
            );
            assert!(
                bare.iter()
                    .all(|c| c.kind != CommandKind::Action(PaletteAction::ToggleDocumentWrap))
            );
            assert!(
                bare.iter()
                    .any(|c| c.kind == CommandKind::Action(PaletteAction::SearchHosts))
            );
            assert!(
                bare.iter()
                    .any(|c| c.kind == CommandKind::Action(PaletteAction::ToggleCodePanel))
            );
        });
    }
}
