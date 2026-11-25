use super::L10nKey;

pub fn translate_en(key: L10nKey) -> &'static str {
    match key {
        L10nKey::SettingsNoMatchesShort => "No matches",
        L10nKey::SettingsMatchCount => "{count} matches",
        L10nKey::SettingsKeyThen => "then",
        L10nKey::SettingsStartupRestore => "Startup & restore",
        L10nKey::SettingsSettingsFile => "Settings file",
        L10nKey::SettingsReveal => "Reveal",
        L10nKey::SettingsThemeModeSystem => "System",
        L10nKey::SettingsThemeModeSystemDesc => {
            "Match the system, switching between a light and a dark theme."
        }
        L10nKey::SettingsThemeModeLightDesc => "Always use a light theme.",
        L10nKey::SettingsThemeModeDarkDesc => "Always use a dark theme.",
        L10nKey::SettingsThemeSlotLightDesc => "Used while the system is in light mode.",
        L10nKey::SettingsThemeSlotDarkDesc => "Used while the system is in dark mode.",
        L10nKey::SettingsThemeSlotDesc => "Theme for the terminal and interface.",
        L10nKey::SettingsLightThemeLabel => "Light theme",
        L10nKey::SettingsDarkThemeLabel => "Dark theme",
        L10nKey::SettingsNoThemesMatch => "No themes match",
        L10nKey::SettingsRestoreChanged => "Restore {count} changed",
        L10nKey::SettingsShortcutsHint => {
            "Click a shortcut and press new keys. Press a second chord right after to make a sequence. Esc cancels, ⌫ clears."
        }
        L10nKey::SettingsShortcutsHintTmux => {
            "tmux preset: pane and tab actions use a prefix key. Click a shortcut to change it; Esc cancels, ⌫ clears."
        }
        L10nKey::SettingsShortcutConflict => {
            "{keys} is already used by “{action}”. Replacing takes it from there."
        }
        L10nKey::SettingsReplace => "Replace",
        L10nKey::SettingsNoActionsMatch => "No actions match “{query}”",
        L10nKey::SettingsSearchShortcuts => "Search actions or keys",
        L10nKey::SettingsHostsDesc => "Recently used hosts. Search to find any saved host.",
        L10nKey::SettingsAddHost => "Add host",
        L10nKey::SettingsNoHostsMatch => "No hosts match “{query}”",
        L10nKey::SettingsHostsFromFiles => "{count} hosts from {files} sources",
        L10nKey::SettingsMoreHosts => "{count} more hosts",
        L10nKey::SettingsShowRecentOnly => "Show recent only",
        L10nKey::SettingsShowAll => "Show all",
        L10nKey::SettingsUnsaved => "Unsaved",
        L10nKey::SettingsNever => "Never",
        L10nKey::SettingsDefinedIn => "Defined in",
        L10nKey::SettingsConnectInNewTab => "Connect in new tab",
        L10nKey::SettingsCopied => "Copied",
        L10nKey::SettingsCopySshCommand => "Copy ssh command",
        L10nKey::SettingsStoredInAnterm => "Stored in anterm settings",
        L10nKey::SettingsClickAgainToRemove => "Click again to remove",
        L10nKey::SettingsRemoveHost => "Remove host",
        L10nKey::SettingsEditHost => "Edit",
        L10nKey::SettingsLive => "Connected",
        L10nKey::SettingsPressKeysShort => "Press keys…",
        L10nKey::SettingsSearchAgents => "Search agents",
        L10nKey::SettingsConnectedMachines => "Connected",
        L10nKey::SettingsInstalledCount => "{count} installed",
        L10nKey::SettingsMachine => "Machine",
        L10nKey::SettingsMachineLocalDesc => {
            "Hooks are installed per machine. Remote machines appear here while connected."
        }
        L10nKey::SettingsMachineRemoteDesc => "Installing over the active connection to {name}.",
        L10nKey::SettingsAgentsInstalledSummary => "{count} of {total} installed",
        L10nKey::SettingsNoAgentsInstalled => "No agent hooks installed on this machine yet.",
        L10nKey::SettingsNoAgentsMatch => "No agents match “{query}”",
        L10nKey::SettingsMoreAgents => "{count} more agents available",
        L10nKey::SettingsShowInstalledOnly => "Show installed only",
        L10nKey::SettingsWorking => "Working…",
        L10nKey::SettingsUpdateAvailable => "Update available",
        L10nKey::SettingsRevealHookFile => "Reveal hook file",
        L10nKey::SettingsSaveError => "Changes could not be saved: {error}",
        L10nKey::SettingsRetrySave => "Retry saving",

        L10nKey::SettingsNavGeneral => "General",
        L10nKey::SettingsEditShortcuts => "Edit shortcuts…",
        L10nKey::SettingsModified => "Modified",
        L10nKey::SettingsSearchResults => "Search results",
        L10nKey::SettingsOpenSetting => "Open setting",
        L10nKey::SettingsTerminalFontGroup => "Terminal text",
        L10nKey::SettingsInterfaceFontGroup => "Interface text",
        L10nKey::SettingsUnsavedTitle => "Save changes before leaving?",
        L10nKey::SettingsUnsavedBody => "Save your changes, discard them, or continue editing.",
        L10nKey::SettingsSaveChanges => "Save changes",
        L10nKey::SettingsThemeDraft => "Theme changes are previewed until you save.",

        L10nKey::SearchFiles => "Search files…",
        L10nKey::PanelSearchWholeWord => "Match whole word",
        L10nKey::PanelSearchIdle => "Search the contents of every file under:",
        L10nKey::PanelSearchNoFolder => "No folder to search.",
        L10nKey::PanelSearchNoFolderHint => {
            "Search looks through the project the active tab is in."
        }
        L10nKey::PanelSearchSshPane => "Search can't look inside an SSH pane's files.",
        L10nKey::PanelSearchSshPaneHint => {
            "Open the host as a remote workspace to search it, or browse it in Files."
        }
        L10nKey::PanelSearchSearching => "Searching…",
        L10nKey::PanelSearchNoMatches => "No results for “{query}”.",
        L10nKey::PanelSearchBadPattern => "Not a valid regular expression: {e}",
        L10nKey::PanelSearchServerTooOld => {
            "The anterm-server on this machine is too old to search file contents."
        }
        L10nKey::PanelSearchServerTooOldHint => "Update the server on that host to use Search.",
        L10nKey::PanelSearchFailed => "Search failed: {e}",
        L10nKey::PanelSearchTruncated => {
            "Not every match is shown. Narrow the search to see the rest."
        }
        L10nKey::PanelSearchLineTooltip => "Line {line}, column {column}",
        L10nKey::PanelSearchHostGone => "The machine this project is on is not connected.",
        L10nKey::SearchSettings => "Search settings…",
        L10nKey::FilterHosts => "Filter hosts…",
        L10nKey::SearchTheme => "Search…",
        L10nKey::SearchTabActions => "Commands",
        L10nKey::SearchTabTabs => "Tabs",
        L10nKey::SearchTabTags => "Tags",
        L10nKey::SearchTabHosts => "Hosts",
        L10nKey::SearchTabSymbols => "Symbols",
        L10nKey::SearchPlaceholderActions => "Execute a command…",
        L10nKey::SearchPlaceholderSwitchTag => "Switch tag…",
        L10nKey::SearchPlaceholderRenameTab => "New tab name…",
        L10nKey::SearchRenameTabHint => "Press Enter to rename · Leave empty to reset",
        L10nKey::SearchRenameTabResetHint => "Press Enter to reset to default name",
        L10nKey::SearchHintSwitch => "Switch",
        L10nKey::SearchHintRename => "Rename",
        L10nKey::SearchHintTags => "Tags",
        L10nKey::SearchHintEditTag => "Edit tag",
        L10nKey::SearchTabsEmpty => "No open tabs",
        L10nKey::SearchTagsEmpty => "No tags yet",
        L10nKey::SearchTagsEmptyHint => "Use Edit Tab Tags to create tags for your tabs.",
        L10nKey::SearchTagTabsCount => "{count} tabs",
        L10nKey::SearchPlaceholderHosts => "Search hosts, or type user@host to connect…",
        L10nKey::SearchPlaceholderSymbols => "Go to a symbol in this file…",
        L10nKey::SearchSymbolsNone => "No symbols in this file",
        L10nKey::SearchSymbolsNoneHint => {
            "Symbols are listed for Rust, Go, Python, JavaScript, TypeScript, C, C++, Java, Ruby, shell and Markdown."
        }
        L10nKey::SearchTabFiles => "Files",
        L10nKey::SearchPlaceholderFiles => "Go to a file by name — add :line to jump to it…",
        L10nKey::SearchFilesNoRoots => "No project to search",
        L10nKey::SearchFilesNoRootsHint => {
            "Files are found in the project your terminal is in. cd into one to search it."
        }
        L10nKey::SearchFilesIndexing => "Indexing files…",
        L10nKey::SearchFilesFailed => "The project's files could not be listed.",
        L10nKey::SearchFilesGoToLine => "line {line}",
        L10nKey::SearchFilesCapped => "Large project — only the first {count} files are searched",
        L10nKey::CmdGoToFile => "Go to File…",
        L10nKey::SearchNoResults => "No results",
        L10nKey::SearchAllShellsPlaceholder => "Search all shells…",
        L10nKey::SearchAllOpenTabsPlaceholder => "Search open tabs…",
        L10nKey::SearchTabPanesCount => "{count} panes",
        L10nKey::SearchFind => "Find…",
        L10nKey::SearchMatchCase => "Match case",
        L10nKey::SearchUseRegex => "Use regular expression",
        L10nKey::NewFolderName => "New folder name",
        L10nKey::NewFileName => "New file name",
        L10nKey::HomeNewTab => "New Tab",
        L10nKey::HomeReopenClosedTab => "Reopen Closed Tab",
        L10nKey::HomeSearchEverywhere => "Search…",
        L10nKey::HomeSplitRight => "Split Right",
        L10nKey::HomeSplitDown => "Split Down",
        L10nKey::HomeSettings => "Settings…",
        L10nKey::Reconnect => "Reconnect",
        L10nKey::TryAgain => "Try Again",
        L10nKey::Refreshing => "refreshing…",
        L10nKey::Binary => "binary",
        L10nKey::Delete => "Delete",
        L10nKey::ConnectSshHint => "Type user@host to connect over SSH instead.",
        L10nKey::EditHint => "edit",
        L10nKey::OpenFileFromTree => "Open a file from the file tree",
        L10nKey::TreeDirLoading => "Reading…",
        L10nKey::TreeDirEmpty => "Empty",
        L10nKey::TreeDirHiddenOnly => "Only hidden files",
        L10nKey::TreeDirUnreadable => "Could not be read",
        L10nKey::TreeSearchCapped => "First {n} matches",
        L10nKey::TreeSearchFailed => "Search failed",
        L10nKey::FileChangedOnDisk => "File changed on disk",
        L10nKey::Reload => "Reload",
        L10nKey::KeepMine => "Keep mine",
        L10nKey::Dismiss => "Dismiss",
        L10nKey::StoredPasswordRejected => "The stored password was rejected. Enter a new one.",
        L10nKey::StoredPassphraseRejected => {
            "The saved passphrase did not unlock this key. Enter the right one."
        }
        L10nKey::Trust => "Trust",
        L10nKey::Abort => "Abort",
        L10nKey::HostKeyOverrideMessage => {
            "Type \"yes\" to override and trust the new key, or Esc to abort."
        }
        L10nKey::Override => "Override",
        L10nKey::RememberKeychain => "Remember (keychain)",
        L10nKey::Cancel => "Cancel",
        L10nKey::Close => "Close",
        L10nKey::QuitStopServerTitle => "Quit and stop anterm server?",
        L10nKey::QuitStopServerBody => {
            "This quits anterm and stops anterm server; anything running in your shells is terminated. Your tabs and layout reopen with fresh shells next launch. (Closing the window only retires anterm to the tray — the shells keep running.)"
        }
        L10nKey::QuitAndStop => "Quit and Stop",
        L10nKey::CloseSshConnectionTitle => "Close this SSH connection?",
        L10nKey::CloseSshConnectionBody => "The connection is live. Closing will end it.",
        L10nKey::ClosePaneBusyTitle => "Close this pane?",
        L10nKey::CloseTabBusyTitle => "Close this tab?",
        L10nKey::CloseBusyCommandBody => "{what} is still running. Closing ends it.",
        L10nKey::CloseBusyAgentBody => "{agent} is still working. Closing ends its turn.",
        L10nKey::CloseIdleBody => "Its shell ends with it.",
        L10nKey::CloseTabsTitle => "Close {count} tabs?",
        L10nKey::CloseTabsBody => "The shells in them end too.",
        L10nKey::Keep => "Keep",
        L10nKey::SettingsNavAppearance => "Appearance",
        L10nKey::SettingsNavTerminal => "Terminal",
        L10nKey::SettingsNavInput => "Keyboard & Mouse",
        L10nKey::SettingsNavSsh => "SSH",
        L10nKey::SettingsNavAgents => "Integrations",
        L10nKey::SettingsNavKeybindings => "Keyboard shortcuts",
        L10nKey::SettingsNavAbout => "About",
        L10nKey::SettingsWindowTitle => "Settings",
        L10nKey::Reset => "Reset",
        L10nKey::Save => "Save",
        L10nKey::Download => "Download",
        L10nKey::SettingsThemeIntroTitle => "Theme",
        L10nKey::SettingsFontSize => "Terminal font size",
        L10nKey::SettingsFontSizeDesc => "Terminal text size in points.",
        L10nKey::SettingsUiFontSize => "Interface font size",
        L10nKey::SettingsUiFontSizeDesc => "Text size for tabs, panels and settings.",
        L10nKey::SettingsUiFontFamily => "Interface font",
        L10nKey::SettingsUiFontFamilyDesc => "For tabs, sidebars, dialogs and settings.",
        L10nKey::SettingsLineHeight => "Line height",
        L10nKey::SettingsLineHeightDesc => "Row spacing as a multiple of the font size.",
        L10nKey::SettingsFontFamily => "Terminal font",
        L10nKey::SettingsFontFamilyDesc => "Pick from fonts installed on your system.",
        L10nKey::SettingsBoldFont => "Bold font",
        L10nKey::SettingsBoldFontDesc => "Bold face; Default derives it from the main font.",
        L10nKey::SettingsItalicFont => "Italic font",
        L10nKey::SettingsItalicFontDesc => "Italic face; Default derives it from the main font.",
        L10nKey::SettingsFontLigatures => "Font ligatures",
        L10nKey::SettingsFontLigaturesDesc => "Programming ligatures in terminal text.",
        L10nKey::SettingsFontThicken => "Thicken strokes",
        L10nKey::SettingsFontThickenDesc => "Draw text slightly bolder. Applies after a restart.",
        L10nKey::SettingsCursor => "Cursor",
        L10nKey::SettingsCursorShape => "Cursor shape",
        L10nKey::SettingsCursorShapeDesc => "How the terminal cursor is drawn.",
        L10nKey::SettingsPromptCursorShape => "Prompt cursor shape",
        L10nKey::SettingsPromptCursorShapeDesc => "The cursor at the shell prompt.",
        L10nKey::SettingsCursorBlink => "Cursor blink",
        L10nKey::SettingsCursorBlinkDesc => "Pulse the cursor while the terminal is focused.",
        L10nKey::SettingsLanguage => "Language",
        L10nKey::SettingsLanguageDesc => "Choose the language used for the anterm interface.",
        L10nKey::SettingsLanguageEnglish => "English",
        L10nKey::SettingsLanguageChinese => "简体中文",
        L10nKey::SettingsSearchLanguageKeywords => "language, locale, english, chinese",
        L10nKey::SettingsTransparency => "Transparency",
        L10nKey::SettingsOpacity => "Opacity",
        L10nKey::SettingsOpacityDesc => "Below 100%, the desktop shows through.",
        L10nKey::SettingsBlur => "Blur",
        L10nKey::SettingsBlurDesc => {
            if cfg!(target_os = "macos") {
                "Blur whatever is behind a translucent window."
            } else {
                "Blur whatever is behind a translucent window. Needs a compositor that offers it — KDE Plasma does; GNOME and plain X11 leave the window merely transparent."
            }
        }
        L10nKey::SettingsBlurAutoDesc => "Blur behind a translucent window. Auto material only.",
        L10nKey::SettingsBackdrop => "Background material",
        L10nKey::SettingsBackdropDesc => "Native backdrop behind a translucent window.",
        L10nKey::SettingsSearchBackdropKeywords => {
            "material backdrop mica acrylic blur frosted window background"
        }
        L10nKey::SettingsBackdropAuto => "Auto",
        L10nKey::SettingsBackdropBlur => "Blur",
        L10nKey::SettingsBackdropMica => "Mica",
        L10nKey::SettingsBackdropMicaAlt => "Mica Alt",
        L10nKey::SettingsBackdropAcrylic => "Acrylic",
        L10nKey::SettingsBackdropOff => "Off",
        L10nKey::FollowTheme => "Follow theme",
        L10nKey::SettingsDimInactivePanes => "Dim inactive panes",
        L10nKey::SettingsDimInactivePanesDesc => {
            "Fade unfocused panes so the active one stands out."
        }
        L10nKey::SettingsAutoHideTitlebarButtons => "Show title bar buttons on hover",
        L10nKey::SettingsAutoHideTitlebarButtonsDesc => {
            "Keep the new tab and sidebar buttons out of sight until the pointer is over the title bar."
        }
        L10nKey::SettingsOpenThemesFolder => "Open themes folder",
        L10nKey::SettingsChangeThemeImage => "Change…",
        L10nKey::SettingsChooseThemeImage => "Choose…",
        L10nKey::SettingsRemoveThemeImage => "Remove",
        L10nKey::SettingsImageOpacity => "Image opacity",
        L10nKey::SettingsImageOpacityDesc => {
            "How strongly the image shows over the background color."
        }
        L10nKey::SettingsEditTheme => "Edit theme",
        L10nKey::SettingsEditThemeIntro => {
            "Editing a copy. Changes apply live and save to its file."
        }
        L10nKey::SettingsBackgroundImage => "Background image",
        L10nKey::SettingsBackgroundImageDesc => {
            "Composited over the background color, under the text."
        }
        L10nKey::SettingsAnsiColors => "ANSI colors",
        L10nKey::SettingsCustomThemes => "Custom themes",
        L10nKey::SettingsThemesRejected => "Not loaded from the themes folder",
        L10nKey::ThemeDuplicateFailed => "Could not duplicate the theme",
        L10nKey::ThemeSaveFailed => "Could not save the theme",
        L10nKey::OpenInFileManagerFailed => "Could not open {path}",
        L10nKey::ExplorerMenuOpenIn => "Open in anterm",
        L10nKey::ExplorerMenuOpenHere => "Open anterm here",
        L10nKey::SettingsDuplicateToEdit => "Duplicate to edit",
        L10nKey::SettingsHosts => "Hosts",
        L10nKey::SettingsNothingMatches => "Nothing matches {query}.",
        L10nKey::SettingsInAnterm => "anterm settings",
        L10nKey::SettingsImportFromSshConfig => "Import from ~/.ssh/config",
        L10nKey::SettingsNoHostsYet => "No hosts yet",
        L10nKey::SettingsImportUnreadable => "Could not read {path} — nothing was imported.",
        L10nKey::SettingsImportNoHosts => {
            "{path} names no hosts to import — only wildcard or Match rules."
        }
        L10nKey::SettingsImportSummary => {
            "{count} hosts added — {updated} updated, {unchanged} already current"
        }
        L10nKey::SettingsImportIgnored => {
            "{count} options have no setting in anterm and were left in the file: {options}"
        }
        L10nKey::SettingsImportMoreOptions => "+{count} more",
        L10nKey::SettingsCouldntForgetPassword => {
            "Could not forget the saved password for {endpoint}: {error}"
        }
        L10nKey::SettingsSecurity => "Security",
        L10nKey::SettingsSecurityIntro => "Any host can override these under its Advanced.",
        L10nKey::SettingsVerifyHostKeys => "Verify host keys",
        L10nKey::SettingsVerifyHostKeysDesc => {
            "Check server keys against known_hosts. Off, a spoofed server goes unnoticed."
        }
        L10nKey::WarnBeforeClosing => "Warn before closing",
        L10nKey::SettingsWarnBeforeClosingDesc => {
            "Confirm before closing a tab with a live SSH session."
        }
        L10nKey::SettingsNewHost => "New host",
        L10nKey::SettingsKeepEditing => "Keep Editing",
        L10nKey::SettingsHostRequired => "Needs a host — won't be saved.",
        L10nKey::SettingsPortInvalid => "Port must be 1-65535 — blank means 22.",
        L10nKey::SettingsPassword => "Password",
        L10nKey::SettingsNameHint => "Optional label",
        L10nKey::SettingsHostHint => "hostname or IP",
        L10nKey::SettingsUserHint => "resolved at connect",
        L10nKey::SettingsPasswordHint => "Ask when connecting",
        L10nKey::SettingsKeyPassphrase => "Key passphrase",
        L10nKey::SettingsPassphraseNeedsKey => {
            "Name a key file first — a passphrase is stored against the key it unlocks."
        }
        L10nKey::SettingsCouldntSavePassword => {
            "Could not save the password for {endpoint}: {error}"
        }
        L10nKey::SettingsCouldntSavePassphrase => {
            "Could not save the passphrase for {key}: {error}"
        }
        L10nKey::SettingsJumpHostUnknown => "No host profile named {jump_name} — won't be saved.",
        L10nKey::SettingsJumpHostSelf => "A host can't be its own jump host — won't be saved.",
        L10nKey::SettingsFwdNeedsBoth => {
            "Needs a listen port and a target host:port — won't be saved."
        }
        L10nKey::SettingsFwdNeedsListen => "Needs a listen port — won't be saved.",
        L10nKey::SettingsProxyPortInvalid => {
            "Port must be 1-65535 — the host on its own takes the default port."
        }
        L10nKey::SettingsDefault => "Default",
        L10nKey::SettingsShell => "Shell",
        L10nKey::SettingsShellIntro => "The program new terminals launch. Empty uses {default}.",
        L10nKey::SettingsProgram => "Shell program",
        L10nKey::SettingsProgramDesc => "A name on PATH or an absolute path, e.g. zsh, fish.",
        L10nKey::SettingsArguments => "Shell arguments",
        L10nKey::SettingsArgumentsDesc => "Split like a command line, e.g. -l or -c \"echo hi\".",
        L10nKey::SettingsArgumentsInvalid => {
            "The quotes do not balance — this value was not saved."
        }
        L10nKey::SettingsStartIn => "Starting directory",
        L10nKey::SettingsStartInDesc => "Launch directory, home folder, or a fixed path.",
        L10nKey::SettingsCustomPath => "Custom path",
        L10nKey::SettingsCustomPathDesc => "The directory new shells start in.",
        L10nKey::SettingsWdInherit => "Inherit",
        L10nKey::SettingsWdHome => "Home",
        L10nKey::SettingsWdCustom => "Custom",
        L10nKey::SettingsWdPathInvalid => {
            "That directory does not exist — the value was not saved."
        }
        L10nKey::SettingsShellFooter => {
            "Only for shells with nothing to inherit, like a window's first tab. New tabs and splits take the active pane's directory."
        }
        L10nKey::SettingsScrolling => "Scrolling",
        L10nKey::SettingsScrollback => "Scrollback buffer",
        L10nKey::SettingsScrollbackDesc => "Lines of history kept per pane. Applies to new panes.",
        L10nKey::SettingsScrollSpeed => "Scroll speed",
        L10nKey::SettingsScrollSpeedDesc => "Multiplier applied to mouse-wheel scrolling.",
        L10nKey::SettingsSmoothScroll => "Smooth scrolling",
        L10nKey::SettingsSmoothScrollDesc => {
            "Ease each wheel notch into place. Trackpads are unaffected."
        }
        L10nKey::SettingsMouse => "Mouse",
        L10nKey::SettingsFocusFollowsMouse => "Focus follows mouse",
        L10nKey::SettingsFocusFollowsMouseDesc => "Hovering a pane focuses it without a click.",
        L10nKey::SettingsHideMouseWhileTyping => "Hide mouse while typing",
        L10nKey::SettingsHideMouseWhileTypingDesc => {
            "Hide the pointer while typing; moving brings it back."
        }
        L10nKey::SettingsMouseZoom => "Zoom with the wheel",
        L10nKey::SettingsMouseZoomDesc => "Hold it and scroll to resize the terminal font.",
        L10nKey::SettingsMouseZoomOff => "Off",
        L10nKey::SettingsReportMouseToApps => "Report mouse to apps",
        L10nKey::SettingsReportMouseToAppsDesc => {
            "Let vim, tmux and other apps handle clicks; Shift keeps it local."
        }
        L10nKey::SettingsBell => "Bell",
        L10nKey::SettingsTerminalBell => "Terminal bell",
        L10nKey::SettingsTerminalBellDesc => "How a bell (^G) is signalled.",
        L10nKey::SettingsLinks => "Links",
        L10nKey::DetectUrls => "Detect URLs",
        L10nKey::SettingsDetectUrlsDesc => "Underline links on hover; {modifier}-click opens them.",
        L10nKey::ForwardSshLoopbackLinks => "Forward remote ports",
        L10nKey::SettingsForwardSshLoopbackLinksDesc => {
            "Open a remote pane's localhost links here."
        }
        L10nKey::SettingsOpenFilesInternal => "Built-in editor",
        L10nKey::SettingsOpenFilesSystem => "Default app",
        L10nKey::SettingsOpenFilesCommand => "Command",
        L10nKey::SettingsOpenFilesModeDesc => {
            "What {modifier}-clicking a file link opens. Only the built-in editor jumps to a line or opens remote files."
        }
        L10nKey::LinkFileNotUnder => "{path} — nothing by that name under {dir}",
        L10nKey::LinkFileNoDirectory => {
            "{path} — this pane has not said which directory it is in, so a relative path has no base"
        }
        L10nKey::LinkFileMissing => "{path} — nothing at that path",
        L10nKey::LinkDirOutsideTree => {
            "{path} — on another machine, and outside every folder the Files panel has open"
        }
        L10nKey::OpenFilesWith => "Open files with",
        L10nKey::SettingsOpenFilesWithDesc => {
            "Use {path}, {line}, {column}. Empty uses the default app."
        }
        L10nKey::SettingsBellModeOff => "Off",
        L10nKey::SettingsBellModeVisual => "Visual",
        L10nKey::SettingsBellModeAudible => "Audible",
        L10nKey::SettingsBellModeBoth => "Both",
        L10nKey::SettingsPrompt => "Prompt & command history",
        L10nKey::SettingsPromptIntro => {
            "anterm's own editor and menus at the prompt. Turn one off to use the shell's."
        }
        L10nKey::SettingsPromptEditor => "anterm prompt editor",
        L10nKey::SettingsPromptEditorDesc => {
            "Selection, undo and menus on the line you type. Off hands it back to ZLE, readline or fish."
        }
        L10nKey::SettingsNeedsPromptEditor => {
            "Needs the prompt editor: with it off, this key already belongs to the shell."
        }
        L10nKey::SettingsTabCompletion => "Tab completion",
        L10nKey::SettingsTabCompletionDesc => {
            "Tab opens anterm's completion menu. Off, Tab goes to the shell."
        }
        L10nKey::SettingsHistorySearch => "Command history search",
        L10nKey::SettingsHistorySearchDesc => {
            "⌃R opens anterm's fuzzy history search. Off, ⌃R goes to the shell."
        }
        L10nKey::SettingsSelectionClipboard => "Selection & clipboard",
        L10nKey::SettingsSmartSelection => "Smart selection",
        L10nKey::SettingsSmartSelectionDesc => {
            "Double-click selects a whole URL, path or bracket pair."
        }
        L10nKey::SettingsCopyOnSelect => "Copy on select",
        L10nKey::SettingsCopyOnSelectDesc => {
            if cfg!(target_os = "macos") {
                "Selecting text with the mouse copies it to the clipboard right away, no ⌘C needed."
            } else {
                "Selecting text with the mouse copies it to the clipboard right away, no Ctrl+Shift+C needed."
            }
        }
        L10nKey::SettingsTrimTrailingSpaces => "Trim trailing spaces on copy",
        L10nKey::SettingsTrimTrailingSpacesDesc => {
            "Strip trailing whitespace from each copied line."
        }
        L10nKey::SettingsKeyboard => "Keyboard",
        L10nKey::SettingsOptionAsMeta => "Option (⌥) acts as Meta",
        L10nKey::SettingsOptionAsMetaDesc => "⌥+key sends Meta, e.g. ⌥B moves back a word.",
        L10nKey::SettingsAgentsIntro => "Agent hooks",
        L10nKey::SettingsAgentsIntroDesc => {
            "Hooks show each agent's status — working, waiting, done — in the tab bar."
        }
        L10nKey::SettingsReadingAgentConfig => "Reading this machine's agent config…",
        L10nKey::SettingsStatusInstalled => "Installed",
        L10nKey::SettingsInstall => "Install",
        L10nKey::SettingsReinstall => "Reinstall",
        L10nKey::SettingsUpdate => "Update",
        L10nKey::SettingsUninstall => "Uninstall",
        L10nKey::SettingsOfflineMachines => {
            "{count} more saved machines are not connected — open a workspace on one to install its hooks there."
        }
        L10nKey::SettingsSyncWithSystem => "Appearance",
        L10nKey::SettingsSyncWithSystemDesc => "Separate light and dark themes that follow the OS.",
        L10nKey::SettingsLegiblePalette => "Legible bright colors",
        L10nKey::SettingsLegiblePaletteDesc => "Fix bright ANSI colors that are hard to read.",
        L10nKey::SettingsCustom => "Custom",
        L10nKey::SettingsCustomValue => "Custom ({value})",
        L10nKey::SettingsDark => "Dark",
        L10nKey::SettingsLight => "Light",
        L10nKey::SettingsStartupWindow => "Startup window",
        L10nKey::SettingsStartupWindowDesc => "Window state when anterm launches.",
        L10nKey::SettingsRememberWindowSize => "Remember window size & position",
        L10nKey::SettingsRememberWindowSizeDesc => {
            "Reopen where the window was when anterm last quit."
        }
        L10nKey::SettingsRestoreLastLayout => "Restore last layout",
        L10nKey::SettingsRestoreLastLayoutDesc => {
            "Reopen last session's tabs, splits and directories."
        }
        L10nKey::SettingsShowTrayIcon => "Show tray icon",
        L10nKey::SettingsShowTrayIconDesc => "Alerts when an agent needs input; jumps to its pane.",
        L10nKey::SettingsTabs => "Tabs",
        L10nKey::SettingsNewTabPosition => "New tab position",
        L10nKey::SettingsNewTabPositionDesc => "Where a freshly opened tab is inserted.",
        L10nKey::SettingsConfirmClose => "Confirm before closing",
        L10nKey::SettingsConfirmCloseDesc => {
            "When closing a tab or pane asks first. SSH hosts set to warn before closing are always asked about."
        }
        L10nKey::ConfirmCloseNever => "Never",
        L10nKey::ConfirmCloseWhenBusy => "When busy",
        L10nKey::ConfirmCloseAlways => "Always",
        L10nKey::SettingsTabBarPosition => "Tab bar position",
        L10nKey::SettingsTabBarPositionDesc => "A strip on top or a sidebar on the left.",
        L10nKey::DocumentDock => "Dock beside terminal",
        L10nKey::DocumentFill => "Fill window",
        L10nKey::SettingsNotifications => "Notifications",
        L10nKey::SettingsNotifyOnCommandFinish => "Notify on command finish",
        L10nKey::SettingsNotifyOnCommandFinishDesc => {
            "Alert when a long foreground command finishes."
        }
        L10nKey::SettingsNotifyThreshold => "Minimum command duration",
        L10nKey::SettingsNotifyThresholdDesc => {
            "Notify only when a command runs for at least this long."
        }
        L10nKey::NotifyModeNever => "Never",
        L10nKey::NotifyModeUnfocused => "When unfocused",
        L10nKey::NotifyModeAlways => "Always",
        L10nKey::ProgramNotesDropped => "More notifications from this pane weren't shown",
        L10nKey::SettingsStartupNormal => "Normal",
        L10nKey::SettingsStartupMaximized => "Maximized",
        L10nKey::SettingsStartupFullscreen => "Fullscreen",
        L10nKey::SettingsAfterCurrent => "After current",
        L10nKey::SettingsAtEnd => "At end",
        L10nKey::SettingsTop => "Top",
        L10nKey::SettingsLeft => "Left",
        L10nKey::SettingsPrefix => "Prefix",
        L10nKey::SettingsKeybindingsIntroDesc => {
            "Click a shortcut and press new keys; chain keys for sequences like Ctrl-B X. Esc cancels, Backspace clears."
        }
        L10nKey::KeybindGoToTab => "Go to Tab {n}",
        L10nKey::KeybindInsertNewline => "Insert Newline",
        L10nKey::KeybindForkSessionRight => "Fork Session Right",
        L10nKey::KeybindForkSessionLeft => "Fork Session Left",
        L10nKey::KeybindForkSessionDown => "Fork Session Down",
        L10nKey::KeybindForkSessionUp => "Fork Session Up",
        L10nKey::SettingsAboutDesc1 => {
            "A terminal workbench for long-running work. Sessions outlive the window, remote hosts feel local, and coding agents report their status right in the sidebar."
        }
        L10nKey::SettingsDefaultTerminal => "Default terminal",
        L10nKey::SettingsDefaultTerminalDesc => {
            "Open Unix executables, SSH links and man pages in anterm."
        }
        L10nKey::SettingsDefaultTerminalSet => "Set as Default Terminal",
        L10nKey::SettingsDefaultTerminalSetSuccess => {
            "anterm is now the default handler for supported terminal files and links."
        }
        L10nKey::SettingsDefaultTerminalSetFailed => {
            "Could not set anterm as the default terminal: {error}"
        }
        L10nKey::SettingsVersion => "Version",
        L10nKey::SettingsUpdates => "Updates",
        L10nKey::SettingsUpdateAndRelaunch => "Update and relaunch",
        L10nKey::SettingsUpdateViewRelease => "View release",
        L10nKey::SettingsUpdateChecking => "Checking for updates…",
        L10nKey::SettingsUpdateUpToDate => "You're running the latest version.",
        L10nKey::SettingsUpdateDownloadingPercent => "Downloading the update… {percent}% of {size}",
        L10nKey::SettingsUpdateDownloadingBytes => "Downloading the update… {received}",
        L10nKey::SettingsUpdateVerifying => "Verifying the downloaded update…",
        L10nKey::SettingsUpdateInstalling => "Relaunching with the update…",
        L10nKey::SettingsUpdateCheckNow => "Check now",
        L10nKey::SettingsUpdateCancel => "Cancel download",
        L10nKey::SettingsUpdateRetry => "Try again",
        L10nKey::SettingsUpdateDismiss => "Dismiss",
        L10nKey::SettingsUpdateDownloadManually => "Download manually",
        L10nKey::SettingsUpdateFailedTitle => "Updating to {version} failed.",
        L10nKey::SettingsUpdateReady => "Version {version} is downloaded and ready to install.",
        L10nKey::SettingsUpdateReadyNextLaunch => {
            "It will be installed the next time you start anterm."
        }
        L10nKey::SettingsUpdateInstallNow => "Install and relaunch",
        L10nKey::SettingsUpdateDiscard => "Discard",
        L10nKey::SettingsAutoDownload => "Download updates in the background",
        L10nKey::SettingsAutoDownloadDesc => {
            "Download updates in the background so installing is just a restart. Nothing installs without asking."
        }
        L10nKey::SettingsUpdateChannel => "Update channel",
        L10nKey::SettingsUpdateChannelDesc => {
            "Nightly builds the latest code every night, untested."
        }
        L10nKey::SettingsUpdateChannelStable => "Stable",
        L10nKey::SettingsUpdateChannelNightly => "Nightly",
        L10nKey::SettingsDaemonStale => "anterm server is still running {build}.",
        L10nKey::SettingsDaemonStaleDesc => {
            "anterm was updated, but your panes still run on the old server. Restarting it loads the new build and ends everything running in them."
        }
        L10nKey::UpdateDialogTitle => "Update available",
        L10nKey::UpdateDialogDetail => {
            "anterm {version} is available — you're on {current}. Installing restarts the app; anterm server keeps running, so your panes survive."
        }
        L10nKey::UpdateDialogDetailWindows => {
            "anterm {version} is available — you're on {current}. Installing restarts the app and anterm server: processes in your panes are ended, and your tabs and layout come back with fresh shells."
        }
        L10nKey::UpdateDialogDetailManual => {
            "anterm {version} is available — you're on {current}. {hint}"
        }
        L10nKey::UpdateDialogCannotSelfUpdate => "This installation cannot update itself.",
        L10nKey::UpdateDialogLater => "Later",
        L10nKey::UpdateDialogNextLaunch => "Install on Next Launch",
        L10nKey::UpdateDialogNeedsElevation => {
            "anterm is installed for all users, so Windows asks for administrator approval once before installing. anterm itself never runs elevated."
        }
        L10nKey::SettingsUpdateCheckFailed => "Could not check for updates: {error}",
        L10nKey::SettingsUpdatePrepareFailed => "Update failed: {error}",
        L10nKey::SettingsUpdateLaunchFailed => "Could not start the installer: {error}",
        L10nKey::SettingsUpdateUnsupportedMacos => {
            "This copy is not in a writable anterm.app bundle, so it cannot replace itself. Move anterm to Applications, or open the release page to update."
        }
        L10nKey::SettingsUpdateUnsupportedLinux => {
            "The release has no Linux package for this architecture. Build from source, or use your package manager."
        }
        L10nKey::SettingsUpdateLinuxPackage => {
            "Linux installations are updated by hand. Download {name} from the release page, or use your package manager."
        }
        L10nKey::SettingsUpdateUnsupportedWindows => {
            "This copy is not a recognized Inno Setup or portable ZIP installation, so it cannot update itself. Open the release page to update it by hand."
        }
        L10nKey::SettingsUpdateWindowsAllUsers => {
            "anterm is installed for all users, so replacing it needs administrator rights that anterm will not ask for itself. Open the release page and run the installer to update."
        }
        L10nKey::SettingsUpdateUnsupportedPlatform => {
            "Automatic installation is not available on this platform. Open the release page."
        }
        L10nKey::SettingsUpdateMissingPackage => {
            "The release has no {name} package for this installation. Open the release page to choose another package."
        }
        L10nKey::SettingsUpdateMissingChecksums => {
            "The release has no checksums.txt, so anterm refuses to install it automatically."
        }
        L10nKey::SettingsVersionAvailable => "Version {version} is available.",
        L10nKey::SettingsCheckUpdatesDesc => {
            "Opens the release page if anterm can't update in place."
        }
        L10nKey::SettingsCheckUpdatesOnLaunch => "Check for updates on launch",
        L10nKey::SettingsServer => "anterm server",
        L10nKey::SettingsServerDesc => "Keeps terminal sessions running in the background.",
        L10nKey::SettingsRestartServer => "Restart anterm server…",
        L10nKey::SettingsAppHttpProxy => "Proxy for updates",
        L10nKey::SettingsAppHttpProxyDesc => {
            "For anterm's update checks. Empty uses the system proxy."
        }
        L10nKey::SettingsAppHttpProxyInvalid => {
            "Not a valid proxy address — this value was not saved."
        }
        L10nKey::SettingsAgentClaudeCode => "Claude Code",
        L10nKey::SettingsAgentCodex => "Codex",
        L10nKey::SettingsAgentTraeCode => "TraeCode",
        L10nKey::SettingsAgentCopilotCli => "Copilot CLI",
        L10nKey::SettingsAgentOpencode => "OpenCode",
        L10nKey::SettingsAgentPi => "Pi",
        L10nKey::SettingsAgentGrokBuild => "Grok Build",
        L10nKey::SettingsAgentOhMyPi => "Oh My Pi",
        L10nKey::SettingsAgentGemini => "Gemini",
        L10nKey::SettingsAgentDroid => "Droid",
        L10nKey::SettingsAgentQwenCode => "Qwen Code",
        L10nKey::SettingsAgentGoose => "Goose",
        L10nKey::SettingsAgentKimiCode => "Kimi Code",
        L10nKey::SettingsAgentQoderCLI => "Qoder CLI",
        L10nKey::SettingsAgentCrush => "Crush",
        L10nKey::SettingsAgentCodeBuddy => "CodeBuddy",
        L10nKey::SettingsAgentCursorCli => "Cursor CLI",
        L10nKey::SettingsAgentPrimeAgent => "Prime Agent",
        L10nKey::SettingsAgentAntigravity => "Antigravity",
        L10nKey::SettingsAgentEmpryo => "Empryo",
        L10nKey::SettingsAgentJcode => "jcode",
        L10nKey::SettingsAgentMuse => "Muse Code",
        L10nKey::SettingsMuseManualInstall => "Manual install required",
        L10nKey::AppAgentHooksMuseManualInstall => "Run on the remote host: {command}",
        L10nKey::SettingsAgentQoderCn => "Qoder CN CLI",
        L10nKey::SettingsSearchAboutKeywords => "version license credits build update check github",
        L10nKey::SettingsSearchAppHttpProxyKeywords => {
            "proxy http https socks socks5 clash v2ray network download update"
        }
        L10nKey::SettingsSearchAnsiColorsKeywords => "palette 16 terminal colours theme",
        L10nKey::SettingsSearchBackgroundImageKeywords => {
            "background image wallpaper picture photo backdrop theme"
        }
        L10nKey::SettingsSearchImageOpacityKeywords => {
            "background image opacity strength fade wallpaper"
        }
        L10nKey::SettingsSearchArgumentsKeywords => "shell flags login args",
        L10nKey::SettingsSearchBlurKeywords => {
            "transparency translucent frosted vibrancy window background"
        }
        L10nKey::SettingsSearchBoldFontKeywords => "typeface weight",
        L10nKey::SettingsSearchClaudeCodeKeywords => {
            "agent integration hooks install uninstall status rich session working waiting tab bar sidebar badge claude"
        }
        L10nKey::SettingsSearchCodexKeywords => "agent integration hooks install openai codex",
        L10nKey::SettingsSearchTraeCodeKeywords => {
            "agent integration hooks install trae code traecli traex"
        }
        L10nKey::SettingsSearchCopilotCliKeywords => {
            "agent integration hooks install github copilot"
        }
        L10nKey::SettingsSearchCopyOnSelectKeywords => "clipboard selection yank mouse",
        L10nKey::SettingsSearchCursorBlinkKeywords => "caret blinking flash",
        L10nKey::SettingsSearchCursorShapeKeywords => "caret block bar underline beam",
        L10nKey::SettingsSearchPromptCursorShapeKeywords => {
            "prompt cursor shape caret block bar underline beam shell integration"
        }
        L10nKey::SettingsSearchCustomThemesKeywords => {
            "theme duplicate edit colors folder yaml import background image wallpaper"
        }
        L10nKey::SettingsSearchDetectUrlsKeywords => "links hyperlink clickable open",
        L10nKey::SettingsSearchDimInactivePanesKeywords => {
            "fade unfocused inactive split pane focus opacity highlight active dimming"
        }
        L10nKey::SettingsSearchAutoHideTitlebarButtonsKeywords => {
            "auto hide autohide title bar titlebar buttons chrome hover pointer minimal clean new tab sidebar toggle"
        }
        L10nKey::SettingsSearchFocusFollowsMouseKeywords => "pane hover activate",
        L10nKey::SettingsSearchFontFamilyKeywords => "typeface monospace typography",
        L10nKey::SettingsSearchFontLigaturesKeywords => "typography glyph fira",
        L10nKey::SettingsSearchFontThickenKeywords => {
            "font smoothing thicken bold weight thin dilation antialiasing AppleFontSmoothing"
        }
        L10nKey::SettingsSearchFontSizeKeywords => "typography text bigger smaller zoom",
        L10nKey::SettingsSearchForwardSshLoopbackLinksKeywords => {
            "ssh remote port tunnel localhost forward links ports autoforward detect"
        }
        L10nKey::SettingsSearchGrokBuildKeywords => {
            "agent integration hooks install xai grok build"
        }
        L10nKey::SettingsSearchHideMouseWhileTypingKeywords => "cursor pointer autohide",
        L10nKey::SettingsSearchHistorySearchKeywords => {
            "ctrl-r reverse search fuzzy history recall fzf prompt"
        }
        L10nKey::SettingsSearchHostsKeywords => {
            "ssh host connection saved profile import ssh_config manage add edit quick connect"
        }
        L10nKey::SettingsSearchItalicFontKeywords => "typeface oblique",
        L10nKey::SettingsSearchKeybindingsKeywords => {
            "shortcut hotkey keyboard binding chord tmux preset rebind prefix"
        }
        L10nKey::SettingsSearchKeybindingsTitle => "Keyboard shortcuts",
        L10nKey::SettingsSearchLineHeightKeywords => "typography leading spacing",
        L10nKey::SettingsSearchUiFontFamilyKeywords => {
            "interface font family ui typeface typography chrome sidebar tab"
        }
        L10nKey::SettingsSearchNewTabPositionKeywords => "tabs order end after current",
        L10nKey::SettingsSearchConfirmCloseKeywords => {
            "confirm close closing tab pane ask prompt warn busy running idle always never"
        }
        L10nKey::SettingsSearchNotifyOnCommandFinishKeywords => {
            "notification alert done osc desktop banner long command"
        }
        L10nKey::SettingsSearchNotifyThresholdKeywords => {
            "notification alert seconds duration long command delay"
        }
        L10nKey::SettingsSearchOpacityKeywords => {
            "transparency translucent see through window alpha"
        }
        L10nKey::SettingsSearchOpenFilesWithKeywords => {
            "links file editor command external app path line column"
        }
        L10nKey::SettingsSearchOpencodeKeywords => "agent integration plugin install opencode",
        L10nKey::SettingsSearchOptionAsMetaKeywords => {
            "alt keyboard modifier escape macos option meta option acts as meta"
        }
        L10nKey::SettingsSearchOhMyPiKeywords => "agent integration extension install omp oh my pi",
        L10nKey::SettingsSearchGeminiKeywords => "agent integration hooks install gemini google",
        L10nKey::SettingsSearchDroidKeywords => "agent integration hooks install droid factory",
        L10nKey::SettingsSearchQwenCodeKeywords => {
            "agent integration hooks install qwen code qwen-code"
        }
        L10nKey::SettingsSearchGooseKeywords => "agent integration hooks plugin install goose",
        L10nKey::SettingsSearchKimiCodeKeywords => {
            "agent integration hooks install kimi code kimi-code moonshot"
        }
        L10nKey::SettingsSearchQoderCLIKeywords => "agent integration hooks install qoder qodercli",
        L10nKey::SettingsSearchCrushKeywords => "agent integration hooks install crush",
        L10nKey::SettingsSearchCodeBuddyKeywords => {
            "agent integration hooks install codebuddy codebuddy-code cbc tencent"
        }
        L10nKey::SettingsSearchCursorCliKeywords => {
            "agent integration hooks install cursor cursor-agent"
        }
        L10nKey::SettingsSearchPrimeAgentKeywords => {
            "agent integration extension install prime prime-agent primeintellect"
        }
        L10nKey::SettingsSearchEmpryoKeywords => "agent integration hooks install empryo",
        L10nKey::SettingsSearchJcodeKeywords => "agent integration hooks install jcode",
        L10nKey::SettingsSearchMuseKeywords => "agent integration hooks plugins install muse meta",
        L10nKey::SettingsSearchAntigravityKeywords => {
            "agent integration hooks install antigravity agy google"
        }
        L10nKey::SettingsSearchQoderCnKeywords => {
            "agent integration hooks install qodercn qoderclicn qoder-cn qoder china"
        }
        L10nKey::SettingsSearchPiKeywords => "agent integration extension install pi",
        L10nKey::SettingsSearchProgramKeywords => {
            "shell binary zsh bash fish nu nushell pwsh powershell executable launch"
        }
        L10nKey::SettingsSearchRememberWindowSizeKeywords => {
            "window size position bounds geometry launch startup remember"
        }
        L10nKey::SettingsSearchReportMouseToAppsKeywords => {
            "mouse reporting vim tmux click scroll shift passthrough"
        }
        L10nKey::SettingsSearchRestoreLastLayoutKeywords => {
            "restore session previous tabs splits reopen launch startup layout"
        }
        L10nKey::SettingsSearchScrollSpeedKeywords => "mouse wheel multiplier scrolling",
        L10nKey::SettingsSearchSmoothScrollKeywords => {
            "smooth animation ease wheel notch trackpad scrolling"
        }
        L10nKey::SettingsSearchUpdateChannelKeywords => {
            "update channel stable nightly release prerelease build"
        }
        L10nKey::SettingsSearchCheckUpdatesOnLaunchKeywords => {
            "update check launch startup automatic release"
        }
        L10nKey::SettingsSearchAutoDownloadKeywords => {
            "update download background install metered connection"
        }
        L10nKey::SettingsSearchScrollbackKeywords => "history buffer lines scroll",
        L10nKey::SettingsSearchShowTrayIconKeywords => {
            "tray menu bar status item agent attention system icon"
        }
        L10nKey::SettingsSearchSmartSelectionKeywords => {
            "double click word url path select semantic bracket email"
        }
        L10nKey::SettingsSearchStartInKeywords => {
            "cwd working directory start folder path home inherit custom"
        }
        L10nKey::SettingsSearchSyncWithSystemKeywords => {
            "theme dark light auto follow os appearance mode"
        }
        L10nKey::SettingsSearchLegiblePaletteKeywords => {
            "legible contrast bright palette psreadline parameter readable"
        }
        L10nKey::SettingsSearchPromptEditorKeywords => {
            "prompt editor native shell input line editor zle readline fish keybindings ime paste"
        }
        L10nKey::SettingsSearchTabBarPositionKeywords => {
            "tabs vertical sidebar left top layout rail"
        }
        L10nKey::SettingsSearchTabCompletionKeywords => {
            "complete completion menu suggestions tab prompt"
        }
        L10nKey::SettingsSearchTerminalBellKeywords => {
            "bell audible visual flash sound silence beep both ^g"
        }
        L10nKey::SettingsSearchThemeKeywords => {
            "appearance color colours scheme dark light palette background foreground accent sync system os auto follow"
        }
        L10nKey::SettingsSearchTrimTrailingSpacesKeywords => "clipboard whitespace copy",
        L10nKey::SettingsSearchVerifyHostKeysKeywords => {
            "ssh security known_hosts fingerprint mitm host key verification"
        }
        L10nKey::SettingsSearchWarnBeforeClosingKeywords => {
            "ssh confirm close tab pane live session security"
        }
        L10nKey::SettingsSearchStartupWindowKeywords => "launch open maximized fullscreen normal",
        L10nKey::RestartServer => "Restart anterm server",
        L10nKey::Ok => "OK",
        L10nKey::SftpNoTransfers => "No transfers yet.",
        L10nKey::SftpPanelTitleFiles => "Files",
        L10nKey::SftpTooltipRefresh => "Refresh",
        L10nKey::SftpTooltipMore => "More",
        L10nKey::SftpMenuNewFolder => "New Folder",
        L10nKey::SftpMenuNewFile => "New File",
        L10nKey::SftpMenuUpload => "Upload…",
        L10nKey::SftpMenuGotoShellCwd => "Go to Shell Directory",
        L10nKey::SftpMenuHideTransferHistory => "Hide Transfer History",
        L10nKey::SftpMenuTransferHistory => "Transfer History",
        L10nKey::SftpEditNewFolder => "New folder",
        L10nKey::SftpEditNewFile => "New file",
        L10nKey::SftpEditRename => "Rename",
        L10nKey::SftpEditPermissions => "Permissions · {mode}",
        L10nKey::SftpLoading => "Loading…",
        L10nKey::SftpEmptyDirectory => "Empty directory.",
        L10nKey::SftpContextOpen => "Open",
        L10nKey::SftpContextEdit => "Edit",
        L10nKey::SftpContextFollowSymlink => "Follow Symlink",
        L10nKey::SftpContextRename => "Rename",
        L10nKey::SftpContextChmod => "chmod…",
        L10nKey::SftpTransferSummaryRunning => "{count} transferring · {pct}%",
        L10nKey::SftpTransferSummaryFailed => "{count} failed",
        L10nKey::SftpTransferSummaryIdle => "Transfers",
        L10nKey::SftpTransferProgress => "{done} / {total} ({pct}%)",
        L10nKey::SftpTransferDone => "done",
        L10nKey::SftpTransferCancelled => "cancelled",
        L10nKey::SftpTransferError => "error",
        L10nKey::SftpTransferListFailed => "Could not check transfers: {error}",
        L10nKey::SftpPasteUploadFailed => "Could not upload {name} to {host}: {error}",
        L10nKey::LinkFileOpenFailed => "Could not open {path}: {error}",
        L10nKey::ForwardDisconnected => "Disconnected",
        L10nKey::ForwardDisconnectedFrom => "Disconnected from {host}",
        L10nKey::SshEditProfile => "Edit connection…",
        L10nKey::ForwardTooltipAdd => "Add forward",
        L10nKey::ForwardTooltipRemove => "Remove",
        L10nKey::ForwardTooltipTurnOn => "Turn on",
        L10nKey::ForwardTooltipTurnOff => "Turn off — the rule is kept",
        L10nKey::ForwardSwitchFailed => "Couldn't switch the forward — {error}",
        L10nKey::ForwardLocal => "Local",
        L10nKey::ForwardRemote => "Remote",
        L10nKey::ForwardDynamic => "Dynamic",
        L10nKey::ForwardBindLabel => "bind",
        L10nKey::ForwardToLabel => "to",
        L10nKey::ForwardSocksLabel => "SOCKS",
        L10nKey::ForwardAdd => "Add",
        L10nKey::ForwardPortLabel => "Remote port",
        L10nKey::ForwardPortHere => "opens at localhost:{port}",
        L10nKey::ForwardNeedsPort => "A port is a number from 1 to 65535.",
        L10nKey::ForwardAdvancedToggle => "Advanced",
        L10nKey::ForwardSimpleToggle => "Simple",
        L10nKey::ForwardRequestFailed => "Could not reach the session — nothing changed.",
        L10nKey::FileTreePlaceholderFileName => "file name",
        L10nKey::FileTreePlaceholderFolderName => "folder name",
        L10nKey::FileTreePlaceholderNewName => "new name",
        L10nKey::FileTreeDeleteTitle => "Delete \"{name}\"?",
        L10nKey::FileTreeDeleteFolderBody => {
            "The folder and everything inside it will be deleted. This can't be undone."
        }
        L10nKey::FileTreeDeleteFileBody => "This can't be undone.",
        L10nKey::SftpDeleteFolderBody => {
            "The folder and everything inside it will be deleted on {host}. There is no trash on the far side."
        }
        L10nKey::SftpDeleteFileBody => {
            "The file will be deleted on {host}. There is no trash on the far side."
        }
        L10nKey::FileTreeDeleteFailed => "Could not delete {name}",
        L10nKey::FileTreeCreateFailed => "Could not create {name}",
        L10nKey::FileTreeRenameFailed => "Could not rename {name}",
        L10nKey::FileTreeDownloadFailed => "Could not download {name}",
        L10nKey::FileTreeDownloaded => "Downloaded to {path}",
        L10nKey::FileTreeDownloadTooLarge => {
            "Larger than {limit} MB — fetch it with scp or rsync instead."
        }
        L10nKey::FileTreeContextOpen => "Open",
        L10nKey::FileTreeContextCdHere => "cd Here",
        L10nKey::FileTreeContextInsertPath => "Insert Path in Terminal",
        L10nKey::FileTreeContextAttachAgent => "Attach to Agent",
        L10nKey::FileTreeContextNewFile => "New File",
        L10nKey::FileTreeContextNewFolder => "New Folder",
        L10nKey::FileTreeContextRename => "Rename",
        L10nKey::FileTreeContextCopyPath => "Copy Path",
        L10nKey::FileTreeContextHideDotfiles => "Hide Dotfiles",
        L10nKey::FileTreeContextShowDotfiles => "Show Dotfiles",
        L10nKey::FileDropIntoItself => "A folder cannot be copied into itself.",
        L10nKey::FileDropNotHere => "Not on this machine.",
        L10nKey::FileDropNameTaken => "Another item in the same drop already has that name.",
        L10nKey::FileDropTooDeep => "Nested more than {n} folders deep.",
        L10nKey::FileDropTooLarge => "Larger than {limit} MB — send it over SFTP instead.",
        L10nKey::FileDropNoWorkingName => "No free name beside it to copy onto first.",
        L10nKey::FileDropLeftAside => {
            "The copy could not be put in place; what was there is now named \"{name}\" in the same folder."
        }
        L10nKey::FileDropReplaceTitle => "Replace \"{name}\"?",
        L10nKey::FileDropReplaceManyTitle => "Replace {n} items?",
        L10nKey::FileDropReplaceBody => {
            "This folder already has something by that name. Replacing it cannot be undone."
        }
        L10nKey::FileDropReplace => "Replace",
        L10nKey::FileDropFailed => "Could not copy {name}",
        L10nKey::FileDropFailedMany => "Could not copy {name}, and {n} more failed",
        L10nKey::SshPromptNewKey => "new {fingerprint}",
        L10nKey::SshPromptOldKey => "old {old_fingerprint}",
        L10nKey::SshPromptHostKeyNewAlgorithm => {
            "You already know this host by a {previous_algorithm} key. This is a new \
             {algorithm} key, not a replacement for that one."
        }
        L10nKey::SshPromptTypeYesToOverride => "Type \"yes\" to enable Override.",
        L10nKey::EditorCantOpen => "Could not open {path}: {e}",
        L10nKey::EditorCantRead => "Could not read {path}: {e}",
        L10nKey::EditorSaveFailed => "Could not save {name}",
        L10nKey::EditorUnsavedChanges => "\"{name}\" has unsaved changes",
        L10nKey::EditorDiscard => "Discard",
        L10nKey::EditorNoFileOpen => "No file open",
        L10nKey::EditorStripSearch => "Search {n} open files",
        L10nKey::EditorStripAllFiles => "All open files",
        L10nKey::EditorStripHidden => "Hidden · {n}",
        L10nKey::EditorStripInBar => "In tab bar",
        L10nKey::EditorStripNoMatch => "No open files match",
        L10nKey::EditorStripCloseSaved => "Close saved",
        L10nKey::EditorStripCloseOthers => "Close others",
        L10nKey::EditorBackToTerminal => "Back to Terminal (Esc)",
        L10nKey::EditorLnCol => "Ln {line}, Col {column}",
        L10nKey::EditorSelections => "({n} selections)",
        L10nKey::EditorEdit => "Edit",
        L10nKey::EditorPreview => "Preview",
        L10nKey::EditorWrapOn => "Wrap: on",
        L10nKey::EditorWrapOff => "Wrap: off",
        L10nKey::EditorFileTooLarge => "\"{path}\" is too large for the editor ({size} MB)",
        L10nKey::EditorBinaryFile => "\"{path}\" looks like a binary file",
        L10nKey::EditorUntitled => "Untitled-{n}",
        L10nKey::EditorUnsavedChangesMany => "{count} files have unsaved changes",
        L10nKey::EditorSaveAll => "Save All",
        L10nKey::EditorSaveConflictTitle => "\"{name}\" changed on disk",
        L10nKey::EditorSaveConflictBody => {
            "Another program changed it after it was opened here. Overwriting replaces those changes with yours."
        }
        L10nKey::EditorOverwrite => "Overwrite",
        L10nKey::EditorEncodeFailedTitle => "Can't save \"{name}\" as {encoding}",
        L10nKey::EditorEncodeFailedBody => {
            "It contains \"{ch}\", which {encoding} can't represent. Save it as UTF-8 instead?"
        }
        L10nKey::EditorSaveAsUtf8 => "Save as UTF-8",
        L10nKey::EditorAlreadyOpen => "\"{path}\" is already open in the editor",
        L10nKey::EditorGoToLine => "Go to line",
        L10nKey::EditorGoToLineAction => "Go to Line…",
        L10nKey::EditorGoToMatchingBracket => "Go to Matching Bracket",
        L10nKey::EditorToggleComment => "Toggle Comment",
        L10nKey::EditorMoveLineUp => "Move Line Up",
        L10nKey::EditorMoveLineDown => "Move Line Down",
        L10nKey::EditorDuplicateLine => "Duplicate Line",
        L10nKey::EditorDeleteLine => "Delete Line",
        L10nKey::EditorCopyRelativePath => "Copy Relative Path",
        L10nKey::EditorGitNextChange => "Go to Next Change",
        L10nKey::EditorGitPrevChange => "Go to Previous Change",
        L10nKey::EditorGitRevertChange => "Revert Change",
        L10nKey::EditorGitToggleGutter => "Toggle Git Change Markers",
        L10nKey::EditorGitPeekRevert => "Revert",
        L10nKey::EditorGitPeekSummary => "−{removed} +{added} lines against the staged version",
        L10nKey::EditorGitPeekAddedOnly => {
            "These lines are new: the staged version has nothing here."
        }
        L10nKey::EditorGitPeekChange => "Peek Change",
        L10nKey::EditorGitPeekKeys => "Enter reverts · Esc closes",
        L10nKey::EditorProblemsTitle => "Problems",
        L10nKey::EditorProblemsToggle => "Toggle Problems",
        L10nKey::EditorProblemsNone => "No problems in the open files.",
        L10nKey::EditorProblemsMore => "…and {n} more",
        L10nKey::SettingsEditor => "Editor",
        L10nKey::SettingsEditorGitGutter => "Git change markers",
        L10nKey::SettingsEditorGitGutterDesc => {
            "Mark lines that differ from the staged version beside the line numbers and on the scrollbar."
        }
        L10nKey::SettingsEditorLsp => "Language servers",
        L10nKey::SettingsEditorLspDesc => {
            "Start a language server for files it knows, for errors, completion and go to definition. Only files on this machine."
        }
        L10nKey::SettingsEditorSoftWrap => "Wrap long lines",
        L10nKey::SettingsEditorSoftWrapDesc => {
            "Open files with soft wrap on. The status bar's Wrap button changes it for one file."
        }
        L10nKey::SettingsEditorMarkdownPreview => "Open Markdown rendered",
        L10nKey::SettingsEditorMarkdownPreviewDesc => {
            "Show Markdown files as a rendered preview rather than as source when they open."
        }
        L10nKey::SettingsSearchEditorGitGutterKeywords => {
            "git gutter diff changes markers staged index scm vcs modified added deleted"
        }
        L10nKey::SettingsSearchEditorLspKeywords => {
            "lsp language server diagnostics errors warnings completion rust-analyzer definition"
        }
        L10nKey::SettingsSearchEditorSoftWrapKeywords => "wrap soft wrap long lines editor",
        L10nKey::SettingsSearchEditorMarkdownPreviewKeywords => {
            "markdown preview rendered md readme"
        }
        L10nKey::EditorGoToLinePlaceholder => "Line, or line:column (1–{total})",
        L10nKey::EditorGoToSymbolAction => "Go to Symbol in Editor…",
        L10nKey::EditorNavigateBack => "Go Back",
        L10nKey::EditorNavigateForward => "Go Forward",
        L10nKey::EditorSplitRight => "Split Editor Right",
        L10nKey::EditorFocusLeftGroup => "Focus Left Editor Group",
        L10nKey::EditorFocusRightGroup => "Focus Right Editor Group",
        L10nKey::EditorSplitSameFile => "Open in the other group — click to edit it here",
        L10nKey::CmdEditorGoToSymbol => "Editor: Go to Symbol…",
        L10nKey::SearchHeadingReferences => "References",
        L10nKey::SearchSectionThisFile => "In This File",
        L10nKey::SearchSectionProject => "Project",
        L10nKey::SearchHeadingDefinitions => "Definitions",
        L10nKey::CmdEditorGoBack => "Editor: Go Back",
        L10nKey::CmdEditorGoForward => "Editor: Go Forward",
        L10nKey::CmdEditorSplitRight => "Editor: Split Right",
        L10nKey::EditorSaveAs => "Save as",
        L10nKey::EditorSaveAsAction => "Save As…",
        L10nKey::EditorSaveAsPlaceholder => "Full path to save to",
        L10nKey::EditorReplaceExisting => "\"{path}\" already exists. Replace it?",
        L10nKey::EditorReplace => "Replace",
        L10nKey::EditorNewFile => "New File",
        L10nKey::EditorOrphanAdopted => "Unsaved \"{name}\" was moved here from a tab that closed",
        L10nKey::EditorFileDeletedOnDisk => "This file was deleted on disk",
        L10nKey::EditorIndentSpaces => "Spaces: {n}",
        L10nKey::EditorIndentTabs => "Tab Size: {n}",
        L10nKey::LspGoToDefinition => "Go to Definition",
        L10nKey::LspQuickFix => "Quick Fix…",
        L10nKey::LspFormatDocument => "Format Document",
        L10nKey::LspRenameSymbol => "Rename symbol",
        L10nKey::LspRenameSymbolAction => "Rename Symbol…",
        L10nKey::LspRenamePlaceholder => "New name for {name}",
        L10nKey::LspRenameFailed => "Could not rename {name}",
        L10nKey::LspServerMissing => "{name} not installed",
        L10nKey::LspServerStarting => "{name} starting…",
        L10nKey::LspServerDown => "{name} stopped",
        L10nKey::LspProblemsTooltip => "{errors} errors, {warnings} warnings",
        L10nKey::SearchTabLocations => "Locations",
        L10nKey::SearchPlaceholderLocations => "Filter the places found…",
        L10nKey::SearchLocationsNone => "Nothing found",
        L10nKey::LspFindReferences => "Find All References",
        L10nKey::PanelInfoTitle => "Info",
        L10nKey::PanelChangesTitle => "Changes",
        L10nKey::PanelScmTitle => "Changes",
        L10nKey::PanelFilesTitle => "Files",
        L10nKey::PanelNoSession => "No active session.",
        L10nKey::PanelNoSessionHint => {
            "Open a tab to see its shell, directory, and processes here."
        }
        L10nKey::PanelNoWorkingDirectory => "No working directory.",
        L10nKey::PanelNoWorkingDirectoryHint => "This pane has not reported one yet.",
        L10nKey::PanelLoading => "Loading…",
        L10nKey::PanelNotAGitRepo => "Not a git repository.",
        L10nKey::PanelNotAGitRepoHint => "cd into one and this tab lists its uncommitted changes.",
        L10nKey::PanelNoChanges => "No uncommitted changes.",
        L10nKey::PanelNoChangesHint => "The working tree is clean.",
        L10nKey::PanelSessionSubtitle => "Session",
        L10nKey::PanelProcessesSubtitle => "Processes",
        L10nKey::PanelProcessesTotal => "Total",
        L10nKey::PanelPortsSubtitle => "Ports",
        L10nKey::PanelPortsUnsupported => "That machine's anterm-server is too old to list ports.",
        L10nKey::PanelPortsProbeFailed => "Couldn't check what this pane is listening on.",
        L10nKey::PanelPortsRestricted => {
            "Something here runs as another user, whose ports aren't visible."
        }
        L10nKey::PanelPortsEmpty => "No forwarded ports",
        L10nKey::PanelLatency => "latency",
        L10nKey::PortAutoForwarded => "Remote :{port} is now http://localhost:{local}",
        L10nKey::PanelCwd => "cwd",
        L10nKey::PanelShell => "shell",
        L10nKey::PanelSsh => "ssh",
        L10nKey::PanelBranch => "branch",
        L10nKey::PanelChangesRow => "changes",
        L10nKey::PanelAgentWorking => "working",
        L10nKey::PanelAgentWaiting => "waiting",
        L10nKey::PanelAgentDone => "done",
        L10nKey::PanelRevealInFinder => "Reveal in Finder",
        L10nKey::PanelOpenFolder => "Open Folder",
        L10nKey::PanelOpenInBrowser => "Open in Browser",
        L10nKey::ScmGroupMerge => "Merge Changes",
        L10nKey::ScmGroupStaged => "Staged Changes",
        L10nKey::ScmGroupChanges => "Changes",
        L10nKey::ScmGroupUntracked => "Untracked",
        L10nKey::ScmCommitPlaceholder => "Say what changed…",
        L10nKey::ScmCommitButton => "Commit",
        L10nKey::ScmCommitAllButton => "Commit All",
        L10nKey::ScmCommitAmendButton => "Commit (Amend)",
        L10nKey::ScmCommitAndPush => "Commit & Push",
        L10nKey::ScmCommitAndSync => "Commit & Sync",
        L10nKey::ScmAmendLastCommit => "Amend Last Commit",
        L10nKey::ScmStashAll => "Stash All",
        L10nKey::ScmNothingToCommit => "Nothing to commit",
        L10nKey::ScmNetworkBusy => "Another network operation is still running for this repository",
        L10nKey::ScmCommitNeedsMessage => "Write a commit message first",
        L10nKey::ScmDetailFilesFailed => "The file list could not be read",
        L10nKey::ScmTimeNow => "now",
        L10nKey::ScmTimeMinutes => "{n}m",
        L10nKey::ScmTimeHours => "{n}h",
        L10nKey::ScmTimeDays => "{n}d",
        L10nKey::ScmTimeMonths => "{n}mo",
        L10nKey::ScmTimeYears => "{n}y",
        L10nKey::ScmResetHardConfirm => {
            "Reset the branch to this commit? Commits after it fall off the branch, \
             and uncommitted changes are discarded."
        }
        L10nKey::ScmReset => "Reset",
        L10nKey::ScmChipStaged => "STAGED",
        L10nKey::ScmStage => "Stage Changes",
        L10nKey::ScmStageAll => "Stage All Changes",
        L10nKey::ScmUnstage => "Unstage Changes",
        L10nKey::ScmUnstageAll => "Unstage All Changes",
        L10nKey::ScmDiscard => "Discard Changes",
        L10nKey::ScmDiscardAll => "Discard All Changes",
        L10nKey::ScmDiscardConfirm => "Discard changes to {path}? This cannot be undone.",
        L10nKey::ScmOpenConflict => "Resolve Conflict",
        L10nKey::ScmMarkResolved => "Mark as Resolved",
        L10nKey::ScmUnrepresentablePath => {
            "This path is not valid UTF-8, so git cannot be asked about it — read only."
        }
        L10nKey::ScmPublishBranch => "Publish Branch",
        L10nKey::ScmDetached => "detached",
        L10nKey::ScmPushDetached => "Detached HEAD — check out a branch to push",
        L10nKey::ScmPushNoCommits => "No commits to push yet",
        L10nKey::ScmAmendBadge => "amend",
        L10nKey::ScmSync => "Sync Changes",
        L10nKey::ScmPush => "Push",
        L10nKey::ScmPull => "Pull",
        L10nKey::ScmFetch => "Fetch",
        L10nKey::ScmCheckoutBranch => "Checkout to…",
        L10nKey::ScmCreateBranch => "Create Branch…",
        L10nKey::ScmGraphTitle => "History",
        L10nKey::ScmGraphLoadMore => "Load more",
        L10nKey::ScmGraphFilterPlaceholder => "Filter commits…",
        L10nKey::ScmGraphAllBranches => "All Branches",
        L10nKey::ScmGraphEmpty => "No commits yet",
        L10nKey::ScmGraphCurrentBranch => "Current Branch",
        L10nKey::ScmCheckoutCommit => "Checkout Commit",
        L10nKey::ScmCreateBranchHere => "Create Branch Here…",
        L10nKey::ScmResetSoft => "Reset (Soft)",
        L10nKey::ScmResetMixed => "Reset (Mixed)",
        L10nKey::ScmResetHard => "Reset (Hard)",
        L10nKey::ScmCopyCommitSha => "Copy Commit SHA",
        L10nKey::ScmCherryPick => "Cherry Pick",
        L10nKey::ScmRevertCommit => "Revert Commit",
        L10nKey::ScmRefresh => "Refresh",
        L10nKey::ScmBackToChanges => "Back",
        L10nKey::ScmCommitParents => "Parents",
        L10nKey::ScmShowMore => "Show more",
        L10nKey::ScmShowLess => "Show less",
        L10nKey::ScmCommitNotFound => "This commit is not in this repository.",
        L10nKey::ScmFilterChanges => "Filter changed files…",
        L10nKey::ScmViewAsTree => "View as Tree",
        L10nKey::ScmViewAsList => "View as List",
        L10nKey::ScmNoMatchingChanges => "No changed files match the filter.",
        L10nKey::ScmTooManyChanges => "Showing the first {shown} of {total} changes.",
        L10nKey::ScmOpenChanges => "Open Changes",
        L10nKey::ScmDiscardAllConfirm => {
            "Discard all unstaged and untracked changes? Staged changes are kept. \
             This cannot be undone."
        }
        L10nKey::ScmAmendConfirm => {
            "Amend the last commit? It will be replaced by a new one, so anyone who already has it has to reconcile."
        }
        L10nKey::ScmOpMerge => "merging",
        L10nKey::ScmOpRebase => "rebasing",
        L10nKey::ScmOpCherryPick => "cherry-picking",
        L10nKey::ScmOpRevert => "reverting",
        L10nKey::ScmOpBisect => "bisecting",
        L10nKey::ScmOpAm => "applying",
        L10nKey::ScmSwitchRepository => "Switch Repository",
        L10nKey::DiffReading => "Reading diff…",
        L10nKey::DiffNotARepo => "Not a git repository",
        L10nKey::DiffReadFailed => {
            "Could not read the working-tree diff — retrying on the next refresh."
        }
        L10nKey::DiffWorkingTreeClean => "Working tree clean",
        L10nKey::DiffCloseTooltip => "Close Diff (Esc)",
        L10nKey::DiffChangedFiles => "{count} changed files",
        L10nKey::DiffUntrackedCount => " · {count} untracked",
        L10nKey::DiffMoreFiles => {
            "… and {count} more changed files — run git diff in the terminal to see them."
        }
        L10nKey::DiffOversizedNotice => {
            "This working tree is too large to render ({summary}). Every file is collapsed — expand them one at a time, or run git diff in the terminal."
        }
        L10nKey::DiffTruncatedPerFile => {
            "Diff truncated at {limit} lines — run git diff in the terminal for the rest."
        }
        L10nKey::DiffTruncatedBudget => {
            "Body not loaded — past anterm's diff budget. Run git diff in the terminal for this file."
        }
        L10nKey::DiffUntrackedHeader => "Untracked files ({count})",
        L10nKey::DiffMoreUntracked => {
            "… and {count} more — run git status in the terminal to see them."
        }
        L10nKey::DiffLines => "{count} diff lines",
        L10nKey::DiffChangedLines => {
            "{total} changed lines, {loaded} diff rows loaded before {cap} cut the rest"
        }
        L10nKey::DiffBudgetAndCap => "anterm's budget and the per-file cap",
        L10nKey::DiffBudget => "anterm's budget",
        L10nKey::DiffPerFileCap => "the per-file cap",
        L10nKey::DiffUntrackedSummary => "{count} untracked",
        L10nKey::DiffViewSplit => "Side by Side",
        L10nKey::DiffViewUnified => "Unified",
        L10nKey::DiffCopySelection => "Copy Selected Lines",
        L10nKey::PendingConnecting => "Connecting to {machine}…",
        L10nKey::PendingUnreachable => "Could not reach {machine}",
        L10nKey::HomeTimeJustNow => "just now",
        L10nKey::HomeTimeMinutesAgo => "{count} min ago",
        L10nKey::HomeTimeHourAgo => "1 hour ago",
        L10nKey::HomeTimeHoursAgo => "{count} hours ago",
        L10nKey::HomeTimeYesterday => "yesterday",
        L10nKey::HomeTimeDaysAgo => "{count} days ago",
        L10nKey::HomeTimeWeeksAgo => "{count} weeks ago",
        L10nKey::HomeTimeMonthsAgo => "{count} months ago",
        L10nKey::HomeTimeOverYearAgo => "over a year ago",
        L10nKey::HomeReopenNamed => "Reopen \"{name}\"",
        L10nKey::RemoteActionTakeBack => "Take Back",
        L10nKey::PaneLeasedBy => "In use on {by}, at its screen size",
        // Same button, opposite direction: the machine is ahead of this build,
        // so putting our server there takes it back a version. Calling that an
        // update would be a lie, and it is the kind that ends other people's
        // sessions on the way through.
        L10nKey::AppNoRunningCodingAgent => {
            "No running coding agent found — start one (claude, codex, …) in a pane first."
        }
        L10nKey::SwitcherEditHost => "Edit Host…",
        L10nKey::SwitcherSaveAsHost => "Save as SSH Host…",
        L10nKey::SshSaveDroppedJumpHost => {
            "The jump host was left out — a saved host reaches its jump through another saved host."
        }
        L10nKey::PanelFilesNameMatches => "File names",
        L10nKey::PanelSearchInContents => "In file contents",
        L10nKey::SshPromptPasswordFor => "Password for {user}@{host}",
        L10nKey::SshPromptPassphraseFor => "Passphrase for {key_path}",
        L10nKey::SshPromptTwoFactor => "Two-factor authentication",
        L10nKey::SshPromptUnknownHost => "Unknown host {host}",
        L10nKey::SshPromptHostKeyChanged => "Host key CHANGED — possible man-in-the-middle",
        L10nKey::SshPromptHostKeyChangedBody => {
            "The host key differs from the one previously trusted. This may be an attack."
        }
        L10nKey::SshPromptConnect => "Connect",
        L10nKey::SshPromptUnlock => "Unlock",
        L10nKey::SshPromptSubmit => "Submit",
        L10nKey::GitOpFailed => "git {op} failed",
        L10nKey::IoDenied => "You do not have permission.",
        L10nKey::IoGone => "It is not there any more.",
        L10nKey::IoNoSpace => "There is no space left on the disk.",
        L10nKey::IoReadOnly => "That location is read-only.",
        L10nKey::IoBusy => "Something else has it open.",
        L10nKey::IoTimedOut => "The machine did not answer in time.",
        L10nKey::TreeWindowOpenedEmpty => {
            "The server never handed over this window's tabs, so it opened empty. Nothing was lost — they come back when it answers. If it doesn't, run \"Restart anterm server\" from the command palette."
        }
        L10nKey::CmdGroupTabsPanes => "Tabs & Panes",
        L10nKey::CmdGroupView => "View",
        L10nKey::CmdGroupGit => "Git",
        L10nKey::CmdGroupTerminal => "Terminal",
        L10nKey::CmdGroupSsh => "SSH",
        L10nKey::CmdGroupAgents => "Agents",
        L10nKey::CmdGroupApplication => "Application",
        L10nKey::CmdNewTab => "New Tab",
        L10nKey::CmdSwitchTab => "Switch Tab…",
        L10nKey::CmdSwitchTabSubtitle => "Jump to an open tab",
        L10nKey::CmdSwitchTag => "Switch Tag…",
        L10nKey::CmdSwitchTagSubtitle => "Filter by tag and jump to its tab",
        L10nKey::CmdNewWindow => "New Window",
        L10nKey::CmdEditTabTags => "Edit Tab Tags…",
        L10nKey::CmdEditTabTagsSubtitle => "add, rename, recolor, or delete tags",
        L10nKey::CmdRenameTab => "Rename Tab…",
        L10nKey::CmdSplitRight => "Split Right",
        L10nKey::CmdSplitLeft => "Split Left",
        L10nKey::CmdSplitDown => "Split Down",
        L10nKey::CmdSplitUp => "Split Up",
        L10nKey::CmdZoomPane => "Zoom Pane",
        L10nKey::CmdNextPane => "Next Pane",
        L10nKey::CmdPreviousPane => "Previous Pane",
        L10nKey::CmdFocusPaneLeft => "Focus Pane Left",
        L10nKey::CmdFocusPaneRight => "Focus Pane Right",
        L10nKey::CmdFocusPaneUp => "Focus Pane Up",
        L10nKey::CmdFocusPaneDown => "Focus Pane Down",
        L10nKey::CmdResizePaneLeft => "Resize Pane Left",
        L10nKey::CmdResizePaneRight => "Resize Pane Right",
        L10nKey::CmdResizePaneUp => "Resize Pane Up",
        L10nKey::CmdResizePaneDown => "Resize Pane Down",
        L10nKey::CmdSwapPaneNext => "Swap Pane Next",
        L10nKey::CmdSwapPanePrevious => "Swap Pane Previous",
        L10nKey::CmdNextTab => "Next Tab",
        L10nKey::CmdPreviousTab => "Previous Tab",
        L10nKey::CmdMoveTabLeft => "Move Tab Left",
        L10nKey::CmdMoveTabRight => "Move Tab Right",
        L10nKey::CmdRecentTabSwitcher => "Recent Tab Switcher",
        L10nKey::CmdRecentTabSwitcherReverse => "Recent Tab Switcher (Reverse)",
        L10nKey::CmdCopyWorkingDirectory => "Copy Working Directory",
        L10nKey::CmdCopySessionId => "Copy Session ID",
        L10nKey::CmdCopySessionIdSubtitle => "the coding agent's own session id",
        L10nKey::CmdNewAgentTab => "New Agent Tab",
        L10nKey::CmdNextAgentNeedingAttention => "Agent: Go to Next Needing Attention",
        L10nKey::CmdNextAgentNeedingAttentionSubtitle => {
            "waiting agents first, oldest first, then finished turns you have not read"
        }
        L10nKey::AppNoAgentNeedsAttention => "No agent in your open windows needs you.",
        L10nKey::AgentWaitingFor => "waiting {age}",
        L10nKey::AgeUnderMinute => "<1m",
        L10nKey::AgeMinutes => "{count}m",
        L10nKey::AgeHours => "{count}h",
        L10nKey::AgeDays => "{count}d",
        L10nKey::CmdForkSession => "Fork Session",
        L10nKey::CmdForkSessionSubtitle => "branch this agent session into a new tab",
        L10nKey::CmdMarkTabAsUnread => "Mark Tab as Unread",
        L10nKey::CmdHibernateTab => "Hibernate Tab",
        L10nKey::CmdHibernateTabSubtitle => {
            "stop its processes to free memory; selecting it wakes it"
        }
        L10nKey::CmdClosePaneTab => "Close Pane / Tab",
        L10nKey::CmdCloseWindow => "Close Window",
        L10nKey::CmdCloseWindowSubtitle => "shells keep running",
        L10nKey::CmdCloseOtherTabs => "Close Other Tabs",
        L10nKey::CmdCloseTabsToTheRight => "Close Tabs to the Right",
        L10nKey::CmdReopenClosedTab => "Reopen Closed Tab",
        L10nKey::CmdShowLeftSidebar => "Show Left Sidebar",
        L10nKey::CmdHideLeftSidebar => "Hide Left Sidebar",
        L10nKey::CmdHideRightPanel => "Hide Right Panel",
        L10nKey::CmdShowRightPanel => "Show Right Panel",
        L10nKey::CmdShowCodePanel => "Show Code Panel",
        L10nKey::CmdTabBarMoveToTop => "Tab Bar: Move to Top",
        L10nKey::CmdTabBarMoveToLeftSidebar => "Tab Bar: Move to Left Sidebar",
        L10nKey::CmdRightPanelInfo => "Right Panel: Info",
        L10nKey::CmdRightPanelChanges => "Right Panel: Changes",
        L10nKey::CmdRightPanelFiles => "Right Panel: Files",
        L10nKey::CmdRightPanelSearch => "Right Panel: Search",
        L10nKey::CmdChangeTheme => "Change Theme…",
        L10nKey::CmdResetFontSize => "Reset Font Size",
        L10nKey::CmdEnterFullScreen => "Enter Full Screen",
        L10nKey::CmdToggleDiffViewMode => "Toggle Unified / Side-by-Side Diff",
        L10nKey::CmdDocumentDock => "Document: Dock Beside Terminal",
        L10nKey::CmdDocumentFill => "Document: Fill Window",
        L10nKey::CmdToggleDocumentFill => "Toggle Document Fill / Dock",
        L10nKey::CmdDocumentWidthThird => "Document: Third Width",
        L10nKey::CmdDocumentWidthHalf => "Document: Half Width",
        L10nKey::CmdDocumentWidthTwoThirds => "Document: Two-Thirds Width",
        L10nKey::CmdToggleDocumentPreview => "Document: Toggle Markdown Preview",
        L10nKey::CmdToggleDocumentWrap => "Document: Toggle Word Wrap",
        L10nKey::CmdEditorTransformUppercase => "Document: Transform to Uppercase",
        L10nKey::CmdEditorTransformLowercase => "Document: Transform to Lowercase",
        L10nKey::CmdEditorTransformTitleCase => "Document: Transform to Title Case",
        L10nKey::CmdEditorTrimTrailingWhitespace => "Document: Trim Trailing Whitespace",
        L10nKey::CmdEditorJoinLines => "Document: Join Lines",
        L10nKey::CmdEditorRemoveSurroundingBrackets => "Document: Remove Surrounding Brackets",
        L10nKey::CmdGitCommit => "Git: Commit",
        L10nKey::CmdGitStageAll => "Git: Stage All Changes",
        L10nKey::CmdGitUnstageAll => "Git: Unstage All Changes",
        L10nKey::CmdGitDiscardAll => "Git: Discard All Changes",
        L10nKey::CmdGitDiscardAllSubtitle => {
            "Throws away every uncommitted change in the working tree."
        }
        L10nKey::CmdGitCheckoutTo => "Git: Checkout to…",
        L10nKey::CmdGitCreateBranch => "Git: Create Branch…",
        L10nKey::CmdGitSync => "Git: Sync",
        L10nKey::CmdGitSyncSubtitle => "Pull, then push.",
        L10nKey::CmdGitPush => "Git: Push",
        L10nKey::CmdGitPull => "Git: Pull",
        L10nKey::CmdGitFetch => "Git: Fetch",
        L10nKey::CmdGitToggleGraph => "Git: Toggle Commit History",
        L10nKey::CmdClearScrollback => "Clear Scrollback",
        L10nKey::CmdFindInTerminal => "Find in Terminal…",
        L10nKey::CmdToggleComposer => "Toggle Message Composer",
        L10nKey::ComposerModeDefault => "Ask before edits",
        L10nKey::ComposerModeAcceptEdits => "Accept edits",
        L10nKey::ComposerModePlan => "Plan mode",
        L10nKey::ComposerModeBypass => "Bypass permissions",
        L10nKey::ComposerModeAuto => "Auto mode",
        L10nKey::ComposerModeTip => "Permission mode  ·  Shift+Tab to cycle",
        L10nKey::ComposerModel => "Model",
        L10nKey::ComposerModelTip => "Switch model",
        L10nKey::ComposerModelDefault => "Default (recommended)",
        L10nKey::ComposerEffort => "Effort",
        L10nKey::ComposerEffortTip => "Reasoning effort  ·  click to change",
        L10nKey::ComposerPlaceholder => "Message {agent}   ·   @ files   / commands",
        L10nKey::ComposerPlaceholderFiles => "Message {agent}   ·   @ files",
        L10nKey::ComposerSendTip => "Send (Enter)  ·  Shift+Enter for a new line",
        L10nKey::ComposerAttach => "Attach files or images",
        L10nKey::ComposerMenuCommands => "Commands",
        L10nKey::ComposerMenuFiles => "Files",
        L10nKey::ComposerCmdProject => "Project command",
        L10nKey::ComposerCmdUser => "User command",
        L10nKey::ComposerAgentAsking => {
            "{agent} is asking something. Answer it in the terminal, then send."
        }
        L10nKey::CmdFindNext => "Find Next",
        L10nKey::CmdFindPrevious => "Find Previous",
        L10nKey::CmdCopy => "Copy",
        L10nKey::CmdCut => "Cut",
        L10nKey::CmdPaste => "Paste",
        L10nKey::CmdAlternatePaste => "Paste (outside full-screen apps)",
        L10nKey::CmdSelectAll => "Select All",
        L10nKey::CmdSshAddConnection => "SSH: Add Connection…",
        L10nKey::CmdSshManageProfiles => "SSH: Manage Profiles…",
        L10nKey::CmdSshReconnect => "SSH: Reconnect",
        L10nKey::CmdSshRemoteFiles => "SSH: Remote Files",
        L10nKey::CmdSshPortForwarding => "SSH: Port Forwarding",
        L10nKey::CmdSshSaveConnection => "SSH: Save Connection as Host…",
        L10nKey::CmdSshSaveConnectionSubtitle => "Keep this connection as a saved host.",
        L10nKey::CmdSshConnectWithInput => "SSH: Connect {input}",
        L10nKey::CmdAgentSendSelection => "Agent: Send Selection",
        L10nKey::CmdAgentSendSelectionSubtitle => "selection → running coding agent",
        L10nKey::CmdAgentSendGitDiffForReview => "Agent: Send Git Diff for Review",
        L10nKey::CmdAgentSendGitDiffSubtitle => "git diff → running coding agent",
        L10nKey::CmdSettings => "Settings…",
        L10nKey::CmdKeyboardShortcuts => "Keyboard Shortcuts",
        L10nKey::CmdAboutAnterm => "About anterm",
        L10nKey::CmdCheckForUpdates => "Check for Updates…",
        L10nKey::CmdDocumentation => "Documentation",
        L10nKey::CmdJoinDiscord => "Join the Discord",
        L10nKey::CmdReportIssue => "Report an Issue…",
        L10nKey::CmdRestartServer => "Restart anterm server…",
        L10nKey::CmdRestartServerSubtitle => "ends every running shell; layout is kept",
        L10nKey::CmdQuitAnterm => "Quit anterm",
        L10nKey::CmdQuitAntermSubtitle => "stops the server; every running shell ends",
        L10nKey::CmdQuickConnect => "Connect to \"{target}\"",
        L10nKey::CmdQuickConnectSaveProfile => "Save \"{target}\" as profile…",
        L10nKey::AppRestartServerTitle => "Restart anterm server?",
        L10nKey::AppRestartServerAfterUpdateDetail => {
            "anterm was updated to {ours}; the server still runs {build}. It can swap itself for this build in place and keep your shells, but panes on anterm's built-in SSH client would close. Swap now, or later from Settings."
        }
        L10nKey::AppRestartServerFailed => "Could not restart anterm server: {error}",
        L10nKey::AppRestartServerMismatchDetail => {
            "anterm server holding your shells speaks protocol {protocol} (build v{build}); this app speaks {ours}, so your tabs are out of reach.\n\nQuit: nothing changes — anterm server and your shells keep running.\nRestart: tabs come back with fresh shells; anything running now is killed."
        }
        L10nKey::AppRestartServerDialectDetail => {
            "anterm server holding your shells speaks control dialect v{dialect} (build v{build}); this app speaks v{ours}, so every window opens empty.\n\nQuit: nothing changes — anterm server and your shells keep running.\nRestart: tabs come back with fresh shells; anything running now is killed."
        }
        L10nKey::AppRestartServerDialectNewerDetail => {
            "anterm server holding your shells speaks control dialect v{dialect} (build v{build}); this app speaks v{ours}, so every window opens empty.\n\nQuit and install the newer build: the real fix — your shells survive it.\nRestart: tabs come back with fresh shells; anything running now is killed."
        }
        L10nKey::AppRestartServerOldDetail => {
            "anterm server holding your shells predates the version handshake, so this app can't tell what it speaks.\n\nQuit: nothing changes — anterm server and your shells keep running.\nRestart: tabs come back with fresh shells; anything running now is killed."
        }
        L10nKey::AppRestart => "Restart",
        L10nKey::AppRestartServerBody => {
            "This ends every shell on this computer. Your tabs and layout are kept and reopen with fresh shells."
        }
        L10nKey::ConfigQuarantinedStartup => {
            "config.json could not be parsed. anterm is on default settings and kept the file's contents beside it as config.json.corrupt. Fix it and anterm reloads; until then, Settings changes are not saved."
        }
        L10nKey::ConfigQuarantinedReload => {
            "The edited config.json could not be parsed. anterm kept the settings it is running on and set the file's contents aside as config.json.corrupt. Fix it and anterm reloads; saving a setting first overwrites it."
        }
        L10nKey::ConfigUnreadableStartup => {
            "config.json could not be read. anterm is on default settings and left the file exactly as it is. Fix its permissions or contents and anterm reloads; until then, Settings changes are not saved."
        }
        L10nKey::ConfigUnreadableReload => {
            "config.json could not be read. anterm kept the settings it is running on and left the file exactly as it is. Fix its permissions or contents and anterm reloads; saving a setting first overwrites it."
        }
        L10nKey::AppWorktreeRemoveDetailDirty => {
            "The closed tab's worktree at {path} has uncommitted changes. They are saved to refs/anterm/trash/{name} before anything is deleted."
        }
        L10nKey::AppWorktreeRemoveDetailClean => "The closed tab's worktree at {path} is clean.",
        L10nKey::AppWorktreeRemoveTitle => "Remove worktree \"{branch}\"?",
        L10nKey::AppWorktreeDiscardAndRemove => "Discard Changes & Remove",
        L10nKey::AppWorktreeRemove => "Remove Worktree",
        L10nKey::AppWorktreeKeep => "Keep",
        L10nKey::AppReopenTabFailed => "Could not reopen the tab: no terminal started",
        L10nKey::AppOpenTerminalFailed => "Could not open a terminal: {error}",
        L10nKey::AppTabsNotRestored => "{count} tabs from last time could not be reopened",
        L10nKey::AppFullscreenEntered => "Fullscreen — press {key} to leave",
        L10nKey::AppFullscreenEnteredNoKey => {
            "Fullscreen — the window buttons are hidden until you leave"
        }
        L10nKey::AppSshConnectionFailed => "SSH connection failed: {error}",
        L10nKey::AppSshReconnectFailed => "SSH reconnect failed: {error}",
        L10nKey::AppSplitPaneFailed => "Could not split the pane: {error}",
        L10nKey::PaneDragHandleTooltip => "Drag to move this pane",
        L10nKey::AppWorktreeRemoved => "Removed worktree \"{branch}\"",
        L10nKey::AppWorktreeRemoveFailed => "Worktree removal failed: {error}",
        L10nKey::AppWorktreeRemovedBranchKept => {
            "Removed worktree; kept branch \"{branch}\", which has unmerged commits"
        }
        L10nKey::AppPaneNoCodingAgent => "This pane isn't running a coding agent",
        L10nKey::AppForkNoCommand => "anterm has no fork command for {name}",
        L10nKey::AppForkLocalOnly => {
            "{name} sessions can't be forked from inside an SSH or WSL session"
        }
        L10nKey::AppForkNoSessionId => {
            "anterm hasn't seen a {name} session id in this pane — install its hooks in Settings → Integrations"
        }
        L10nKey::AppForkSessionIdNotToken => "{name}'s session id isn't a plain token",
        L10nKey::AppForkMidTurn => "{name} is mid-turn — the fork won't include the turn in flight",
        L10nKey::AppNothingSelected => "Nothing selected — select some terminal output first.",
        L10nKey::AppPaneNoKnownDirectory => "This pane has no known directory.",
        L10nKey::AppNoUncommittedChanges => {
            "No uncommitted changes in {cwd} (or not a git repository)."
        }
        L10nKey::AppCmdAgentLaunchTitle => "Agent: {name}",
        L10nKey::AppNoAgentOnPath => "No coding agent was found on this machine's PATH",
        L10nKey::AppAgentLaunchSaved => "{name} will now launch as: {command}",
        L10nKey::AppAgentLaunchArgsUnknown => {
            "{name} did not report the arguments it was started with"
        }
        L10nKey::AppPlaceholderDescription => "description",
        L10nKey::AppPlaceholderLoginShell => "login shell",
        L10nKey::AppPlaceholderNone => "none",
        L10nKey::AppPlaceholderOpenInDefaultApp => "open in default app",
        L10nKey::AppThemeColorBackground => "Background",
        L10nKey::AppThemeColorForeground => "Foreground",
        L10nKey::AppThemeColorAccent => "Accent",
        L10nKey::AppThemeColorCursor => "Cursor",
        L10nKey::AppThemeColorSelection => "Selection",
        L10nKey::AppThemeAnsiBlack => "Black",
        L10nKey::AppThemeAnsiRed => "Red",
        L10nKey::AppThemeAnsiGreen => "Green",
        L10nKey::AppThemeAnsiYellow => "Yellow",
        L10nKey::AppThemeAnsiBlue => "Blue",
        L10nKey::AppThemeAnsiMagenta => "Magenta",
        L10nKey::AppThemeAnsiCyan => "Cyan",
        L10nKey::AppThemeAnsiWhite => "White",
        L10nKey::AppThemeAnsiBrightBlack => "Bright black",
        L10nKey::AppThemeAnsiBrightRed => "Bright red",
        L10nKey::AppThemeAnsiBrightGreen => "Bright green",
        L10nKey::AppThemeAnsiBrightYellow => "Bright yellow",
        L10nKey::AppThemeAnsiBrightBlue => "Bright blue",
        L10nKey::AppThemeAnsiBrightMagenta => "Bright magenta",
        L10nKey::AppThemeAnsiBrightCyan => "Bright cyan",
        L10nKey::AppThemeAnsiBrightWhite => "Bright white",
        L10nKey::AppAgentHooksThisComputer => "This Computer",
        L10nKey::AppAgentHooksNoHomeDir => {
            "anterm could not work out this computer's home directory, so there is nowhere to install to."
        }
        L10nKey::AppAgentHooksOffline => {
            "Not connected to this machine, so its agent config can't be read or written. Open a workspace on it and come back."
        }
        L10nKey::AppAgentHooksHomeDirUnresolved => "cannot resolve home directory",
        L10nKey::AppAgentHooksInstalled => "Installed",
        L10nKey::AppAgentHooksInstalledEnableCodexThere => {
            "Installed — run `codex features enable hooks` once on that machine"
        }
        L10nKey::AppAgentHooksInstalledCodexEnableFailed => {
            "Installed, but could not run `codex features enable hooks` ({error}) — run it once manually"
        }
        L10nKey::AppAgentHooksRemoved => "Removed",
        L10nKey::AppAgentHooksNothingInstalled => "Nothing installed; nothing to remove",
        L10nKey::AppAgentHooksNoAntermHooks => "No anterm hooks found; nothing to remove",
        L10nKey::AppAgentHooksInstallFailed => "Could not install hooks: {error}",
        L10nKey::AppAgentHooksRemoveFailed => "Could not remove hooks: {error}",
        L10nKey::AppKeybindingDisplacedNote => {
            "{action} took the shortcut from {previous}, which is now unset."
        }
        L10nKey::AppLocalServerName => "the local server",
        L10nKey::AppSshParseUnbalancedQuotes => "Unbalanced quotes in the SSH command",
        L10nKey::AppSshParseNoRemoteCommands => "Remote commands aren't supported here",
        L10nKey::AppSshParseFlagNeedsValue => "-{flag} needs a value",
        L10nKey::AppSshParseInvalidPort => "Invalid port \"{value}\"",
        L10nKey::AppSshParseUnsupportedOption => "Unsupported option \"{option}\"",
        L10nKey::AppSshParseEnterHost => "Enter a host to connect to",
        L10nKey::AppSshParseBadHost => "Invalid host \"{host}\"",
        L10nKey::AppMenuMinimize => "Minimize",
        L10nKey::AppMenuZoom => "Zoom",
        L10nKey::SettingsFontDefault => "Default (match primary)",
        L10nKey::SettingsUiFontDefault => "Default (system UI font)",
        L10nKey::ForwardDescriptionPlaceholder => "what it's for",
        L10nKey::SettingsShellDefaultLoginShell => "your login shell",
        L10nKey::SftpErrorUnsafeRemoteName => "refusing unsafe remote name {name}",
        L10nKey::SftpErrorNoFreeLocalName => {
            "no free name left in Downloads for {name} — move or delete the older copies"
        }
        L10nKey::SftpReplaceTitle => "Replace what is already there?",
        L10nKey::SftpReplaceBody => {
            "{names} already exist in this folder. Uploading overwrites them."
        }
        L10nKey::Replace => "Replace",
        L10nKey::SftpErrorInvalidOctalMode => "invalid octal mode",
        L10nKey::SettingsDaemonStaleDescInPlace => {
            "anterm was updated. The server can swap itself in place and keep your shells; only built-in SSH panes close."
        }
        L10nKey::AppRestartServerBodyInPlace => {
            "anterm server swaps itself for this build in place: your shells keep running, and the window reconnects a moment later. Panes on anterm's built-in SSH client are the exception — those close and need reopening."
        }
        L10nKey::PaneRestoredScreenBanner => {
            "restored screen — this shell is new, nothing above it is still running"
        }
        L10nKey::SettingsPerPaneHistory => "Separate command history per pane",
        L10nKey::SettingsPerPaneHistoryDescription => {
            "↑ walks this pane's own history. bash and zsh only."
        }
        L10nKey::IntegrationNoticeBlocked => {
            "\u{201c}{wrapper}\u{201d} is intercepting shell reports in this pane, so inline completion and the Ctrl+R menu are unavailable. The shell's own history search still works."
        }
        L10nKey::IntegrationNoticeNotEngaged => {
            "anterm shell integration hasn't engaged in this pane, so inline completion and the Ctrl+R menu are unavailable. Usual causes: a shell you started with your own arguments, a PTY wrapper, or an unsupported shell."
        }
        L10nKey::PaneTitleProcessExited => "{title} — process exited",
        L10nKey::LoopbackForwardFailed => "Couldn't forward :{port} — {error}",
        L10nKey::TrayTooltipAgents => "anterm — {parts}",
        L10nKey::TrayAgentSep => ", ",
        L10nKey::CursorShapeBlock => "Block",
        L10nKey::CursorShapeBar => "Bar",
        L10nKey::CursorShapeUnderline => "Underline",
        L10nKey::PromptCursorShapeFollow => "Follow",
        L10nKey::PaletteTryDifferentSearch => "Try a different search.",
        L10nKey::CompletionListingRemote => "listing remote…",
        L10nKey::CompletionRemoteListingFailed => "remote listing failed — {error}",
        L10nKey::CmdUpdateLocalServer => "Update anterm server on this computer…",
        L10nKey::CmdUpdateLocalServerSubtitle => "restarts it onto this app's build",
        L10nKey::AppLocalServerAlreadyCurrent => {
            "anterm server on this computer is already running this build ({build})."
        }
        L10nKey::PanelMoreChangedFiles => {
            "… and {count} more changed files — run git diff to see them."
        }
        L10nKey::ScmFilesChanged => "{count} files changed",
        L10nKey::ScmStagedFileCount => "{count} files staged",
        L10nKey::AppMenuAbout => "About anterm",
        L10nKey::AppMenuCheckForUpdates => "Check for Updates…",
        L10nKey::AppMenuSettings => "Settings…",
        L10nKey::AppMenuServices => "Services",
        L10nKey::AppMenuHideApp => "Hide anterm",
        L10nKey::AppMenuHideOthers => "Hide Others",
        L10nKey::AppMenuShowAll => "Show All",
        L10nKey::AppMenuQuit => "Quit anterm",
        L10nKey::AppMenuFile => "File",
        L10nKey::AppMenuEdit => "Edit",
        L10nKey::AppMenuView => "View",
        L10nKey::AppMenuWindow => "Window",
        L10nKey::AppMenuHelp => "Help",
        L10nKey::AppMenuNewTab => "New Tab",
        L10nKey::AppMenuSplitRight => "Split Right",
        L10nKey::AppMenuSplitLeft => "Split Left",
        L10nKey::AppMenuSplitDown => "Split Down",
        L10nKey::AppMenuSplitUp => "Split Up",
        L10nKey::AppMenuRenameTab => "Rename Tab…",
        L10nKey::AppMenuCopyWorkingDirectory => "Copy Working Directory",
        L10nKey::AppMenuCopySessionId => "Copy Session ID",
        L10nKey::AppMenuForkSession => "Fork Session",
        L10nKey::AppMenuSaveAgentLaunchArgs => "Set Current Launch Args as Default",
        L10nKey::AppMenuClosePaneTab => "Close",
        L10nKey::AppMenuCloseOtherTabs => "Close Other Tabs",
        L10nKey::AppMenuCloseTabsRight => "Close Tabs to the Right",
        L10nKey::AppMenuReopenClosedTab => "Reopen Closed Tab",
        L10nKey::AppMenuUndo => "Undo",
        L10nKey::AppMenuRedo => "Redo",
        L10nKey::AppMenuCut => "Cut",
        L10nKey::AppMenuCopy => "Copy",
        L10nKey::AppMenuPaste => "Paste",
        L10nKey::AppMenuSelectAll => "Select All",
        L10nKey::AppMenuFind => "Find…",
        L10nKey::AppMenuFindNext => "Find Next",
        L10nKey::AppMenuFindPrevious => "Find Previous",
        L10nKey::AppMenuSearchEverywhere => "Search…",
        L10nKey::AppMenuIncreaseFontSize => "Increase Font Size",
        L10nKey::AppMenuDecreaseFontSize => "Decrease Font Size",
        L10nKey::AppMenuResetFontSize => "Reset Font Size",
        L10nKey::AppMenuLeftSidebar => "Left Sidebar",
        L10nKey::AppMenuRightPanel => "Right Panel",
        L10nKey::AppMenuCodePanel => "Code Panel",
        L10nKey::AppMenuTabBarPosition => "Tab Bar Position",
        L10nKey::AppMenuFocusNextPane => "Focus Next Pane",
        L10nKey::AppMenuFocusPreviousPane => "Focus Previous Pane",
        L10nKey::AppMenuZoomPane => "Zoom Pane",
        L10nKey::AppMenuClearScrollback => "Clear Scrollback",
        L10nKey::AppMenuOpenLink => "Open",
        L10nKey::AppMenuOpenLinkWithDefaultApp => "Open with Default App",
        L10nKey::AppMenuRevealInFinder => "Show in Finder",
        L10nKey::AppMenuRevealInFolder => "Show Containing Folder",
        L10nKey::AppMenuCopyLinkPath => "Copy Path",
        L10nKey::AppMenuDocumentation => "anterm Documentation",
        L10nKey::AppMenuKeyboardShortcuts => "Keyboard Shortcuts",
        L10nKey::AppMenuJoinDiscord => "Join the Discord",
        L10nKey::AppMenuReportIssue => "Report an Issue…",
        L10nKey::AppMenuRestartServer => "Restart anterm server…",
        L10nKey::TrayShowAnterm => "Show anterm",
        L10nKey::TrayNotifications => "Notifications",
        L10nKey::TrayAgentNeedsInput => "needs input",
        L10nKey::AgentStatusWorking => "Working",
        L10nKey::AgentStatusWaiting => "Needs input",
        L10nKey::AgentStatusDone => "Done",
        L10nKey::NotifyCommandFinished => "Command finished after {secs}s",
        L10nKey::NotifyCommandFinishedWithCommand => "{command} — finished after {secs}s",
        L10nKey::NotifyAgentFinished => "Finished after {secs}s",
        L10nKey::NotifyAgentWaiting => "Waiting for your input",
        L10nKey::NotifyTurnFinished => "Turn finished",
        L10nKey::TabTooltipShowSidebar => "Show Sidebar",
        L10nKey::TabTooltipHideSidebar => "Hide Sidebar",
        L10nKey::TabTooltipHideDetailPanel => "Hide Detail Panel",
        L10nKey::TabTooltipShowDetailPanel => "Show Detail Panel",
        L10nKey::TabTooltipZoomed => "Pane zoomed — other panes hidden",
        L10nKey::TabMenuLocalShells => "Local",
        L10nKey::TabMenuAddHost => "Add SSH Host…",
        L10nKey::TabMenuAllHosts => "All SSH Hosts…",
        L10nKey::TabMenuOtherShells => "Other Shells…",
        L10nKey::TabMenuSplitHint => "Hold {key} to split",
        L10nKey::TabUnnamedShell => "Shell {n}",
        L10nKey::ShellDefault => "default",
        L10nKey::SidebarTagsAll => "All",
        L10nKey::SidebarTabTags => "Tags",
        L10nKey::SidebarNewTag => "New Tag…",
        L10nKey::SidebarRenameTag => "Rename Tag…",
        L10nKey::SidebarTagColor => "Color",
        L10nKey::SidebarDeleteTag => "Delete Tag",
        L10nKey::SidebarCreateAndAddTag => "Create and add \"{name}\"",
        L10nKey::SidebarTagFilterEmpty => "No tabs tagged \"{name}\"",
        L10nKey::SidebarTagShowAll => "Show all tabs",
        L10nKey::SidebarTagSearchPlaceholder => "Search or create tag…",
        L10nKey::TagColorNone => "None",
        L10nKey::TagColorRed => "Red",
        L10nKey::TagColorOrange => "Orange",
        L10nKey::TagColorAmber => "Amber",
        L10nKey::TagColorGreen => "Green",
        L10nKey::TagColorTeal => "Teal",
        L10nKey::TagColorBlue => "Blue",
        L10nKey::TagColorPurple => "Purple",
        L10nKey::TagColorPink => "Pink",
        L10nKey::TabContextCloseTab => "Close Tab",
        L10nKey::TerminalContextClear => "Clear",
        L10nKey::TabContextCloseTabsBelow => "Close Tabs Below",
        L10nKey::TabContextMarkUnread => "Mark as Unread",
        L10nKey::TabContextHibernate => "Hibernate",
        L10nKey::TabContextWake => "Wake",
        L10nKey::TabTooltipAsleep => "Hibernated — select to wake",
        L10nKey::TabWakeFailed => "Could not wake the tab: none of its panes could be started",
        L10nKey::CmdCopyRaw => "Copy Raw",
        L10nKey::SettingsSearchCopyJoinWrappedKeywords => {
            "clipboard copy wrap unwrap join lines claude raw"
        }
        L10nKey::SettingsCopyJoinWrapped => "Join wrapped lines when copying",
        L10nKey::SettingsCopyJoinWrappedDesc => {
            "In a Claude Code pane, rejoin the lines Claude wrapped at the pane's edge, so a paragraph pastes as one line. ⌘⌥C or Copy Raw copies them as shown"
        }
        L10nKey::SettingsEditorHelixMode => "Helix mode",
        L10nKey::SettingsEditorHelixModeDesc => {
            "Selection-first modal editing (Normal, Insert, Select) in the code editor, inline prompt editor, agent composer, and read-only views"
        }
        L10nKey::SettingsSearchEditorHelixModeKeywords => {
            "helix modal normal insert select mode editor prompt composer keybindings"
        }
        L10nKey::HistoryScopeSession => "Pane",
        L10nKey::HistoryScopeDirectory => "Directory",
        L10nKey::HistoryScopeHost => "Host",
        L10nKey::HistoryScopeAll => "All",
        L10nKey::HistorySearchHint => "Tab scope · ⌘↵ run · ⌃X delete",
        L10nKey::HistorySearchEmpty => "No matching commands",
    }
}

pub fn translate_variant_en(key: L10nKey, branch: &'static str) -> Option<&'static str> {
    let res = match (key, branch) {
        (L10nKey::SettingsMatchCount, "zero") => "No matches",
        (L10nKey::SettingsMatchCount, "one") => "1 match",
        (L10nKey::SettingsMatchCount, "other") => "{count} matches",
        (L10nKey::SearchTagTabsCount, "zero") => "0 tabs",
        (L10nKey::SearchTagTabsCount, "one") => "1 tab",
        (L10nKey::SearchTagTabsCount, "other") => "{count} tabs",
        (L10nKey::SettingsRestoreChanged, "zero") => "Restore changes",
        (L10nKey::SettingsRestoreChanged, "one") => "Restore 1 changed",
        (L10nKey::SettingsRestoreChanged, "other") => "Restore {count} changed",
        (L10nKey::SettingsMoreHosts, "zero") => "No more hosts",
        (L10nKey::SettingsMoreHosts, "one") => "1 more host",
        (L10nKey::SettingsMoreHosts, "other") => "{count} more hosts",
        (L10nKey::SettingsMoreAgents, "zero") => "No more agents",
        (L10nKey::SettingsMoreAgents, "one") => "1 more agent available",
        (L10nKey::SettingsMoreAgents, "other") => "{count} more agents available",
        (L10nKey::SettingsImportSummary, "zero") => {
            "Nothing new — {updated} updated, {unchanged} already current"
        }
        (L10nKey::SettingsImportSummary, "one") => {
            "1 host added — {updated} updated, {unchanged} already current"
        }
        (L10nKey::SettingsImportSummary, "other") => {
            "{count} hosts added — {updated} updated, {unchanged} already current"
        }
        (L10nKey::SettingsImportIgnored, "zero") => "Every option in the file has a anterm setting.",
        (L10nKey::SettingsImportIgnored, "one") => {
            "1 option has no setting in anterm and was left in the file: {options}"
        }
        (L10nKey::SettingsImportIgnored, "other") => {
            "{count} options have no setting in anterm and were left in the file: {options}"
        }
        (L10nKey::SettingsOfflineMachines, "zero") => {
            "0 more saved machines are not connected — open a workspace on one to install its hooks there."
        }
        (L10nKey::SettingsOfflineMachines, "one") => {
            "1 more saved machine is not connected — open a workspace on it to install its hooks there."
        }
        (L10nKey::SettingsOfflineMachines, "other") => {
            "{count} more saved machines are not connected — open a workspace on one to install its hooks there."
        }
        (L10nKey::SftpReplaceBody, "one") => {
            "{names} already exists in this folder. Uploading overwrites it."
        }
        (L10nKey::SftpReplaceBody, "other") => {
            "{names} already exist in this folder. Uploading overwrites them."
        }
        (L10nKey::CloseTabsTitle, "one") => "Close 1 tab?",
        (L10nKey::CloseTabsTitle, "other") => "Close {count} tabs?",
        (L10nKey::AppTabsNotRestored, "one") => "1 tab from last time could not be reopened",
        (L10nKey::AppTabsNotRestored, "other") => {
            "{count} tabs from last time could not be reopened"
        }
        (L10nKey::ScmFilesChanged, "zero") => "No files changed",
        (L10nKey::ScmFilesChanged, "one") => "1 file changed",
        (L10nKey::ScmFilesChanged, "other") => "{count} files changed",
        (L10nKey::ScmStagedFileCount, "zero") => "No staged changes",
        (L10nKey::ScmStagedFileCount, "one") => "1 file staged",
        (L10nKey::ScmStagedFileCount, "other") => "{count} files staged",
        (L10nKey::PanelMoreChangedFiles, "zero") => {
            "… and 0 more changed files — run git diff to see them."
        }
        (L10nKey::PanelMoreChangedFiles, "one") => {
            "… and 1 more changed file — run git diff to see it."
        }
        (L10nKey::PanelMoreChangedFiles, "other") => {
            "… and {count} more changed files — run git diff to see them."
        }
        (L10nKey::DiffChangedFiles, "zero") => "0 changed files",
        (L10nKey::DiffChangedFiles, "one") => "1 changed file",
        (L10nKey::DiffChangedFiles, "other") => "{count} changed files",
        (L10nKey::DiffUntrackedCount, "zero") => " · 0 untracked",
        (L10nKey::DiffUntrackedCount, "one") => " · 1 untracked",
        (L10nKey::DiffUntrackedCount, "other") => " · {count} untracked",
        (L10nKey::DiffMoreFiles, "zero") => {
            "… and 0 more changed files — run git diff in the terminal to see them."
        }
        (L10nKey::DiffMoreFiles, "one") => {
            "… and 1 more changed file — run git diff in the terminal to see it."
        }
        (L10nKey::DiffMoreFiles, "other") => {
            "… and {count} more changed files — run git diff in the terminal to see them."
        }
        (L10nKey::DiffUntrackedHeader, "zero") => "Untracked files (0)",
        (L10nKey::DiffUntrackedHeader, "one") => "Untracked files (1)",
        (L10nKey::DiffUntrackedHeader, "other") => "Untracked files ({count})",
        (L10nKey::DiffMoreUntracked, "zero") => {
            "… and 0 more — run git status in the terminal to see them."
        }
        (L10nKey::DiffMoreUntracked, "one") => {
            "… and 1 more — run git status in the terminal to see it."
        }
        (L10nKey::DiffMoreUntracked, "other") => {
            "… and {count} more — run git status in the terminal to see them."
        }
        (L10nKey::DiffUntrackedSummary, "zero") => "0 untracked",
        (L10nKey::DiffUntrackedSummary, "one") => "1 untracked",
        (L10nKey::DiffUntrackedSummary, "other") => "{count} untracked",
        (L10nKey::HomeTimeMinutesAgo, "one") => "1 min ago",
        (L10nKey::HomeTimeMinutesAgo, "other") => "{count} min ago",
        (L10nKey::HomeTimeHoursAgo, "one") => "1 hour ago",
        (L10nKey::HomeTimeHoursAgo, "other") => "{count} hours ago",
        (L10nKey::HomeTimeDaysAgo, "one") => "1 day ago",
        (L10nKey::HomeTimeDaysAgo, "other") => "{count} days ago",
        (L10nKey::HomeTimeWeeksAgo, "one") => "1 week ago",
        (L10nKey::HomeTimeWeeksAgo, "other") => "{count} weeks ago",
        (L10nKey::HomeTimeMonthsAgo, "one") => "1 month ago",
        (L10nKey::HomeTimeMonthsAgo, "other") => "{count} months ago",
        _ => return None,
    };
    Some(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_is_the_complete_table() {
        assert_eq!(translate_en(L10nKey::SearchFiles), "Search files…");
        assert_eq!(
            translate_variant_en(L10nKey::ScmFilesChanged, "one"),
            Some("1 file changed")
        );
        assert_eq!(translate_variant_en(L10nKey::SearchFiles, "one"), None);
    }
}
