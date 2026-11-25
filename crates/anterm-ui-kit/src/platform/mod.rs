//! OS/desktop integration utilities (notifications, file opener, platform bootstrap).
//!
//! Kept outside `terminal` and `ui` so `core`, `terminal`, and `ui` can all
//! depend on platform helpers without circular dependencies.

pub mod bootstrap;
pub mod notify;
pub mod open;

/// Current host operating system for `gpui_kit::OperatingSystem`-parameterized APIs.
pub const fn current_os() -> gpui::OperatingSystem {
    #[cfg(target_os = "macos")]
    {
        gpui::OperatingSystem::Mac
    }
    #[cfg(target_os = "windows")]
    {
        gpui::OperatingSystem::Windows
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        gpui::OperatingSystem::Linux
    }
}

/// Whether the host platform's secondary modifier (`Cmd` on macOS, `Ctrl` elsewhere) is held.
pub fn is_secondary(modifiers: impl std::borrow::Borrow<gpui::Modifiers>) -> bool {
    let modifiers = modifiers.borrow();
    #[cfg(target_os = "macos")]
    {
        modifiers.platform
    }
    #[cfg(not(target_os = "macos"))]
    {
        modifiers.control
    }
}

/// A [`gpui::Modifiers`] value with only the host platform's secondary modifier set.
#[cfg(any(test, feature = "test-support"))]
pub fn secondary_key() -> gpui::Modifiers {
    #[cfg(target_os = "macos")]
    {
        gpui::Modifiers {
            platform: true,
            ..Default::default()
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        gpui::Modifiers {
            control: true,
            ..Default::default()
        }
    }
}
