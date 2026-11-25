//! Central GPUI action declarations and key-context constants shared across
//! `keymap`, `theme`, `anterm-terminal`, `anterm-editor-ui`, `anterm-files-ui`,
//! `anterm-settings-ui`, and `anterm-app`.
//!
//! Action namespaces (`anterm`, `terminal`, `code_editor`) are preserved
//! verbatim so every persisted user keybinding on disk stays byte-compatible.

use std::sync::LazyLock;

use anterm_core::cli_agent::CLIAgent;
use gpui::actions;

/// Key context applied to the code editor container so editor-only shortcuts
/// outrank app-wide chords without affecting single-line inputs or terminals.
pub const CODE_EDITOR_CONTEXT: &str = "CodeEditor";

/// Key context active while the code editor owns navigation commands.
pub const NAV_KEY_CONTEXT: &str = "Input";

/// Key context active while the Search Everywhere / Command Palette modal is open.
pub const SEARCH_KEY_CONTEXT: &str = "Search";

actions!(
    anterm,
    [
        NewTab,
        NewWindow,
        CloseWindow,
        CloseActiveTab,
        RenameTab,
        EditTabTags,
        SwitchTab,
        SwitchTag,
        CloseOtherTabs,
        CloseTabsToTheRight,
        CopyWorkingDirectory,
        OpenLinkUnderPointer,
        OpenLinkWithDefaultApp,
        RevealLinkUnderPointer,
        CopyLinkPathUnderPointer,
        MarkTabUnread,
        HibernateTab,
        ForkAgentSession,
        ForkAgentSessionRight,
        ForkAgentSessionLeft,
        ForkAgentSessionDown,
        ForkAgentSessionUp,
        CopyAgentSessionId,
        NewAgentTab,
        FocusNextAgentNeedingAttention,
        SaveAgentLaunchArgs,
        SplitRight,
        SplitLeft,
        SplitDown,
        SplitUp,
        FocusNextPane,
        FocusPrevPane,
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
        NextTab,
        PrevTab,
        SelectNextTab,
        SelectPrevTab,
        MoveTabLeft,
        MoveTabRight,
        ActivateTab1,
        ActivateTab2,
        ActivateTab3,
        ActivateTab4,
        ActivateTab5,
        ActivateTab6,
        ActivateTab7,
        ActivateTab8,
        ActivateTab9,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
        TogglePalette,
        QuickOpenFile,
        ReopenClosedTab,
        ToggleMaximizePane,
        ToggleFullscreen,
        ToggleTabSidebar,
        ToggleLeftPanel,
        ToggleRightPanel,
        ShowRightPanelInfo,
        // Keeps its old name even though the panel is now "Source Control":
        // `Config::keybindings` is keyed by action name, so renaming it would
        // orphan every custom binding already on disk.
        ShowRightPanelChanges,
        ShowRightPanelFiles,
        ShowRightPanelSearch,
        ScmCommit,
        ScmCommitAmend,
        ScmStageAll,
        ScmUnstageAll,
        ScmDiscardAll,
        ScmRefresh,
        ScmSync,
        ScmPush,
        ScmPull,
        ScmFetch,
        ScmCheckoutBranch,
        ScmCreateBranch,
        ScmToggleGraph,
        ToggleDiffViewMode,
        OpenSettings,
        ShowKeyboardShortcuts,
        About,
        CheckForUpdates,
        HideApp,
        HideOthers,
        ShowAll,
        MinimizeWindow,
        ZoomWindow,
        OpenDocumentation,
        OpenDiscord,
        ReportIssue,
        RestartDaemon,
        ToggleSftp,
        ShowSshForwards,
        ToggleCodePanel,
        ToggleDocumentFill,
        DocumentWidthThird,
        DocumentWidthHalf,
        DocumentWidthTwoThirds,
        ToggleDocumentPreview,
        ToggleDocumentWrap,
        EditorSave,
        EditorSaveAs,
        EditorGoToLine,
        EditorGoToSymbol,
        EditorNavigateBack,
        EditorNavigateForward,
        EditorSplitRight,
        EditorFocusLeftGroup,
        EditorFocusRightGroup,
        EditorNewFile,
        EditorGoToDefinition,
        EditorQuickFix,
        EditorRenameSymbol,
        EditorFormatDocument,
        EditorTransformUppercase,
        EditorTransformLowercase,
        EditorTransformTitleCase,
        EditorTrimTrailingWhitespace,
        EditorJoinLines,
        EditorRemoveSurroundingBrackets,
        EditorFindReferences,
        EditorNextChange,
        EditorPrevChange,
        EditorRevertChange,
        ToggleEditorGitGutter,
        EditorPeekChange,
        ToggleEditorProblems,
        OpenSshProfiles,
        RestartSshSession,
        SendTab,
        SendBackTab,
        SearchNextTab,
        SearchPrevTab,
        Quit
    ]
);

/// Open a new tab running one particular coding agent. The keymap names one of
/// these per agent (`LaunchAgent:claude`, …).
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = anterm, no_json)]
pub struct LaunchAgent {
    pub agent: CLIAgent,
}

/// Terminal pane actions (`terminal` namespace).
pub mod terminal_actions {
    gpui::actions!(
        terminal,
        [
            CopyText,
            CopyRaw,
            CutText,
            PasteText,
            AlternatePaste,
            SelectAll,
            UndoEdit,
            RedoEdit,
            FindInTerminal,
            FindNext,
            FindPrevious,
            ClearScrollback,
            InsertNewline,
            InsertNewlineFallback,
            ToggleComposer,
            RunReverseSearchSelection
        ]
    );
}

/// Code-editor line, selection, and text-transform actions (`code_editor` namespace).
pub mod editor_text_actions {
    pub use super::CODE_EDITOR_CONTEXT;
    gpui::actions!(
        code_editor,
        [
            AddCursorAbove,
            AddCursorBelow,
            CopyLineDown,
            CopyLineUp,
            DeleteLine,
            InsertLineAbove,
            InsertLineBelow,
            JoinLines,
            MoveLineDown,
            MoveLineUp,
            MoveToMatchingBracket,
            RemoveSurroundingBrackets,
            SelectAllOccurrences,
            SelectLine,
            SelectNextOccurrence,
            SkipOccurrence,
            ToggleBlockComment,
            ToggleLineComment,
            TransformToLowercase,
            TransformToTitleCase,
            TransformToUppercase,
            TrimTrailingWhitespace,
        ]
    );
}

/// How the keymap spells "launch this agent": `LaunchAgent:claude`.
const LAUNCH_ACTION_PREFIX: &str = "LaunchAgent:";

/// One keymap action name per agent, in [`CLIAgent::ALL`] order.
pub fn launch_action_names() -> &'static [(CLIAgent, String)] {
    static NAMES: LazyLock<Vec<(CLIAgent, String)>> = LazyLock::new(|| {
        CLIAgent::ALL
            .into_iter()
            .map(|agent| (agent, format!("{LAUNCH_ACTION_PREFIX}{}", agent.slug())))
            .collect()
    });
    &NAMES
}

pub fn launch_action_name(agent: CLIAgent) -> &'static str {
    launch_action_names()
        .iter()
        .find(|(a, _)| *a == agent)
        .map(|(_, name)| name.as_str())
        .expect("every agent has a launch action")
}

pub fn agent_for_launch_action(action: &str) -> Option<CLIAgent> {
    action
        .strip_prefix(LAUNCH_ACTION_PREFIX)
        .and_then(CLIAgent::from_slug)
}

/// Turns a CamelCase action name like `"SplitRight"` into `"Split Right"`.
pub fn humanize_action(action: &str) -> String {
    let mut out = String::new();
    for (i, ch) in action.chars().enumerate() {
        if i > 0 && ch.is_uppercase() {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}
