//! Desktop notifications (macOS `NSUserNotificationCenter`, Windows WinRT Toast,
//! and Linux `notify-rust`) with click-to-reveal pane activation.
//!
//! Activation events are decoupled from `ui::tray` via
//! [`register_activation_handler`] so platform notifications do not depend on
//! UI modules. Any click that arrives before the UI registers its handler
//! (e.g., Windows COM toast activation at launch) is queued and flushed on
//! registration.

use std::collections::VecDeque;
use std::sync::Mutex;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use std::sync::atomic::Ordering;

use gpui::EntityId;

/// Action requested when the user clicks a delivered desktop notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationActivation {
    RevealPane { leaf_id: u64 },
    #[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
    ShowWindow,
}

type ActivationHandler = fn(NotificationActivation);

struct ActivationBus {
    handler: Option<ActivationHandler>,
    pending: VecDeque<NotificationActivation>,
}

impl ActivationBus {
    const fn new() -> Self {
        Self {
            handler: None,
            pending: VecDeque::new(),
        }
    }
}

static ACTIVATION_BUS: Mutex<ActivationBus> = Mutex::new(ActivationBus::new());

/// Install the UI-side handler for notification clicks and flush any clicks
/// that arrived earlier during startup.
pub fn register_activation_handler(handler: ActivationHandler) {
    let pending: Vec<NotificationActivation> = {
        let Ok(mut bus) = ACTIVATION_BUS.lock() else {
            return;
        };
        bus.handler = Some(handler);
        bus.pending.drain(..).collect()
    };
    for activation in pending {
        handler(activation);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn dispatch_activation(activation: NotificationActivation) {
    let handler = {
        let Ok(mut bus) = ACTIVATION_BUS.lock() else {
            return;
        };
        if let Some(h) = bus.handler {
            Some(h)
        } else {
            bus.pending.push_back(activation);
            None
        }
    };
    if let Some(handler) = handler {
        handler(activation);
    }
}

pub fn notify_desktop(title: Option<&str>, body: &str) {
    notify_desktop_for_pane(title, body, None);
}

/// Show a desktop notification.
///
/// Naming a `pane` makes the notification clickable where the platform supports
/// it, so activating it reveals that pane. It is an `EntityId` on purpose: the
/// tray dispatch identifies leaves by their gpui entity id, and taking a bare
/// `u64` here is what lets a caller hand over a `pane_id` — a different number,
/// assigned by the daemon — and get a notification that reveals nothing.
///
/// macOS always goes through `macos_notify`, clickable or not. Elsewhere, every
/// other case (no pane, unsupported platform, Windows toast queue full) falls
/// back to the plain `notify-rust` path below, which is why
/// `try_clickable_notification` reports whether it took the job.
pub fn notify_desktop_for_pane(title: Option<&str>, body: &str, pane: Option<EntityId>) {
    let summary = sanitize_notification_text(title.unwrap_or("anterm"), NOTIFY_TITLE_MAX);
    let body = sanitize_notification_text(body, NOTIFY_BODY_MAX);

    #[cfg(target_os = "macos")]
    {
        macos_notify::deliver(summary, body, pane.map(|p| p.as_u64()));
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(pane) = pane
            && try_clickable_notification(&summary, &body, pane.as_u64())
        {
            return;
        }

        std::thread::spawn(move || {
            let mut notif = notify_rust::Notification::new();
            notif.summary(&summary).body(&body);
            // Without our own AUMID, the Windows backend falls back to
            // PowerShell's — icon and name included. Only set ours once the
            // shell has indexed a shortcut carrying it: for an AUMID it does
            // not know, `show()` reports success and drops the toast, so the
            // ugly fallback beats the branded one every time we are not sure.
            #[cfg(target_os = "windows")]
            if let Some(app_id) = crate::aumid::toast_app_id() {
                notif.app_id(app_id);
            }
            let _ = notif.show();
        });
    }
}

/// Longest title / body we hand to a notification backend.
///
/// The text comes straight off the terminal (OSC 9/777 payloads, window
/// titles, an agent's own status line), so it is attacker-shaped: arbitrarily
/// long, and free to contain control bytes. Every backend truncates in its own
/// ugly way, and on Windows a stray `ESC` makes the toast XML fail to parse and
/// the whole notification disappear — so clamp both here, once, for all paths.
const NOTIFY_TITLE_MAX: usize = 96;
const NOTIFY_BODY_MAX: usize = 512;

fn sanitize_notification_text(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut taken = 0usize;
    for ch in s.chars() {
        // XML 1.0 allows exactly tab / LF / CR out of the control range.
        if ch.is_control() && !matches!(ch, '\t' | '\n' | '\r') {
            continue;
        }
        if taken == max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
        taken += 1;
    }
    out
}

/// What a click on a notification has to carry back: the pane to reveal.
///
/// On macOS it rides in the notification's `identifier`, which is the one
/// field the center hands back verbatim on activation without a dictionary
/// round trip. On Windows it is the toast's `launch` attribute, which comes
/// back as the `invokedArgs` of a COM activation (`core::toast_activator`).
/// Shape: `anterm-pane-<pid>-<leaf>-<seq>`.
///
/// The pid is what makes a stale identifier fail closed. Notifications outlive
/// the process that sent them, and the center hands a click on one of those to
/// whatever process now owns the bundle id — a relaunched anterm, or a second
/// instance running alongside. A leaf id is a gpui entity id, which a fresh
/// process hands out again in the same order, so without the pid that click
/// would reveal an unrelated pane. On Windows that is the *only* kind of click
/// a process COM launched for the purpose can receive. The sequence number
/// keeps two notifications for the same pane apart — the center treats a
/// repeated identifier as "replace the earlier one".
#[cfg(any(target_os = "macos", target_os = "windows", test))]
const NOTIFICATION_ID_PREFIX: &str = "anterm-pane-";

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn notification_identifier(leaf_id: u64) -> String {
    use std::sync::atomic::AtomicU64;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    format!("{NOTIFICATION_ID_PREFIX}{pid}-{leaf_id}-{seq}")
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn leaf_id_in_identifier(identifier: &str) -> Option<u64> {
    let rest = identifier.strip_prefix(NOTIFICATION_ID_PREFIX)?;
    let (pid, rest) = rest.split_once('-')?;
    if pid.parse::<u32>().ok()? != std::process::id() {
        return None;
    }
    let (leaf_id, _seq) = rest.split_once('-')?;
    leaf_id.parse().ok()
}

/// macOS notifications, straight to `NSUserNotificationCenter`.
///
/// This used to go through `mac-notification-sys` with `wait_for_click`, so a
/// click could reveal the pane. The way that crate notices a click is to park
/// the sending thread and, for every notification outstanding, add a repeating
/// 0.5 s timer to the *main* run loop that calls `deliveredNotifications` — a
/// synchronous XPC round trip — to see whether the banner is still there. A
/// banner nobody clicks stays in Notification Center, so its timer never goes
/// away. Sampled with nine outstanding: a fifth of the UI thread inside that
/// XPC, every window juddering, and each agent turn adding one more timer.
///
/// Here the click arrives through the center's delegate, which is what the
/// API is for: no parked thread, no timer, nothing on the main thread until
/// the user actually clicks. The pane rides in the notification's identifier.
///
/// Nothing else may touch the center on this platform. `mac-notification-sys`
/// installs a delegate of its own the first time it sends, the last
/// `setDelegate:` wins, and `notify-rust` is that crate on macOS — so its
/// `show` is never called here, only its `set_application`, which does not
/// install one (that is what names a bare `cargo run` binary to the center).
#[cfg(target_os = "macos")]
#[allow(
    deprecated,
    reason = "UNUserNotificationCenter needs a signed, entitled bundle; NSUserNotification is what a bare binary can use"
)]
mod macos_notify {
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::ProtocolObject;
    use objc2::{AnyThread, define_class, msg_send};
    use objc2_foundation::{
        NSObject, NSObjectProtocol, NSString, NSUserNotification, NSUserNotificationCenter,
        NSUserNotificationCenterDelegate,
    };

    define_class!(
        // SAFETY: NSObject has no subclassing requirements; `Delegate` has no
        // ivars and no `Drop`.
        #[unsafe(super = NSObject)]
        #[name = "AntermNotificationDelegate"]
        struct Delegate;

        // SAFETY: `NSObjectProtocol` has no safety requirements.
        unsafe impl NSObjectProtocol for Delegate {}

        // SAFETY: `NSUserNotificationCenterDelegate` has no safety requirements.
        unsafe impl NSUserNotificationCenterDelegate for Delegate {
            /// Runs on the main thread, when the user clicks the banner or the
            /// entry in Notification Center. A channel push, then the clicked
            /// entry is dropped from the center — a one-way message, unlike the
            /// `deliveredNotifications` round trip this module exists to avoid.
            #[unsafe(method(userNotificationCenter:didActivateNotification:))]
            fn did_activate(
                &self,
                center: &NSUserNotificationCenter,
                notification: &NSUserNotification,
            ) {
                if let Some(leaf_id) = notification
                    .identifier()
                    .and_then(|id| super::leaf_id_in_identifier(&id.to_string()))
                {
                    super::reveal_pane(leaf_id);
                }
                center.removeDeliveredNotification(notification);
            }
        }
    );

    fn install_delegate() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            // Name the delivering application to the center before it is first
            // touched: the center drops requests from a process with no bundle
            // identity, which is what a bare `cargo run` binary is. This
            // swizzles `-[NSBundle bundleIdentifier]` for the main bundle to
            // `com.anterm.app` when LaunchServices knows that id (a bundled
            // anterm.app, or a machine that has one installed); when it does not,
            // the swizzle's own default, `com.apple.Terminal`, is what the
            // center sees. It can only be called once per process, so there is
            // no second chance to pass a different name.
            let _ = notify_rust::set_application("com.anterm.app");
            // SAFETY: `NSObject`'s `init` takes nothing and returns the object.
            let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(), init] };
            let center = NSUserNotificationCenter::defaultUserNotificationCenter();
            // SAFETY: `delegate` is a `Delegate`, which conforms to the protocol.
            unsafe { center.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
            // The center holds its delegate unretained. Ours lives as long as
            // the process, so it is simply never released.
            std::mem::forget(delegate);
        });
    }

    /// Hands the notification to the center and returns. `deliverNotification:`
    /// is an XPC message, so it goes out on a short-lived thread rather than
    /// from wherever the OSC sequence was parsed — never the UI thread.
    pub(super) fn deliver(title: String, body: String, leaf_id: Option<u64>) {
        std::thread::spawn(move || {
            autoreleasepool(|_| {
                install_delegate();
                let notification = NSUserNotification::new();
                notification.setTitle(Some(&NSString::from_str(&title)));
                notification.setInformativeText(Some(&NSString::from_str(&body)));
                if let Some(leaf_id) = leaf_id {
                    let id = super::notification_identifier(leaf_id);
                    notification.setIdentifier(Some(&NSString::from_str(&id)));
                }
                NSUserNotificationCenter::defaultUserNotificationCenter()
                    .deliverNotification(&notification);
            });
        });
    }
}

#[cfg(all(target_os = "windows", not(test)))]
fn try_clickable_notification(title: &str, body: &str, leaf_id: u64) -> bool {
    // An AUMID the shell has not indexed makes `Show` report success and drops
    // the toast on the floor, so without one the notify-rust path — which
    // deliberately keeps PowerShell's identity rather than lose the toast — is
    // the only one that shows anything at all.
    if crate::aumid::toast_app_id().is_none() {
        return false;
    }
    let Some(tx) = toast_thread() else {
        return false;
    };
    tx.try_send((
        title.to_string(),
        body.to_string(),
        notification_identifier(leaf_id),
    ))
    .is_ok()
}

/// Title, body, and the `launch` identifier a click hands back.
#[cfg(all(target_os = "windows", not(test)))]
type ToastRequest = (String, String, String);

/// Toasts are built, shown and kept on one dedicated thread.
///
/// A click reaches us two ways. The one that always works is COM activation
/// (`core::toast_activator`): the shell calls our registered class with the
/// toast's `launch` string, whether the click lands on the popup or on the
/// toast in notification history, and whether or not this process is still
/// the one that sent it. The other is the toast object's own `Activated`
/// event, which fires only for the popup and only while the object is alive
/// here — it is kept as the fallback for a shortcut that predates the toast
/// activator (an all-users install from an older installer, which we cannot
/// rewrite unelevated). Keeping the objects on a single thread also means the
/// WinRT objects never cross a thread boundary.
#[cfg(all(target_os = "windows", not(test)))]
fn toast_thread() -> Option<&'static std::sync::mpsc::SyncSender<ToastRequest>> {
    use std::sync::OnceLock;
    use std::sync::mpsc::sync_channel;

    static TX: OnceLock<Option<std::sync::mpsc::SyncSender<ToastRequest>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = sync_channel::<ToastRequest>(32);
        std::thread::Builder::new()
            .name("anterm-toasts".into())
            .spawn(move || toast_loop(rx))
            .map_err(|e| log::warn!("failed to start the toast thread: {e}"))
            .ok()
            .map(|_| tx)
    })
    .as_ref()
}

#[cfg(all(target_os = "windows", not(test)))]
fn toast_loop(rx: std::sync::mpsc::Receiver<ToastRequest>) {
    use windows::UI::Notifications::ToastNotification;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

    // MTA lets the Activated callback run on a thread-pool thread without a
    // message loop of our own.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    // How many recent popups keep their `Activated` fallback. Dropping the
    // oldest is what bounds the memory here; the popup is long gone by then,
    // and the COM path does not depend on these objects at all.
    const MAX_LIVE_TOASTS: usize = 32;
    let mut live: VecDeque<ToastNotification> = VecDeque::new();

    while let Ok((title, body, launch)) = rx.recv() {
        if let Some(toast) = show_windows_toast(&title, &body, launch) {
            if live.len() == MAX_LIVE_TOASTS {
                live.pop_front();
            }
            live.push_back(toast);
        }
    }
}

/// The toast's XML. `launch` is what Windows hands back on a click — as the
/// `invokedArgs` of a COM activation, and as the `Arguments` of the in-process
/// `Activated` event.
#[cfg(any(target_os = "windows", test))]
fn toast_xml(title: &str, body: &str, launch: &str) -> String {
    format!(
        r#"<toast launch="{}" activationType="foreground">
  <visual>
    <binding template="ToastText02">
      <text id="1">{}</text>
      <text id="2">{}</text>
    </binding>
  </visual>
</toast>"#,
        xml_escape(launch),
        xml_escape(title),
        xml_escape(body)
    )
}

#[cfg(all(target_os = "windows", not(test)))]
fn show_windows_toast(
    title: &str,
    body: &str,
    launch: String,
) -> Option<windows::UI::Notifications::ToastNotification> {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::core::IInspectable;

    let app_id = crate::aumid::toast_app_id()?;

    let xml = toast_xml(title, body, &launch);

    let doc = match XmlDocument::new() {
        Ok(d) => {
            if let Err(e) = d.LoadXml(&xml.into()) {
                log::warn!("failed to load toast xml: {e}");
                return None;
            }
            d
        }
        Err(e) => {
            log::warn!("failed to create toast xml document: {e}");
            return None;
        }
    };

    let toast = match ToastNotification::CreateToastNotification(&doc) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("failed to create toast notification: {e}");
            return None;
        }
    };

    if let Err(e) = toast.Activated(&TypedEventHandler::new(
        move |_sender: &Option<ToastNotification>, _args: &Option<IInspectable>| {
            toast_activated(&launch);
            Ok(())
        },
    )) {
        log::warn!("failed to register toast activated handler: {e}");
    }

    let notifier = match ToastNotificationManager::CreateToastNotifierWithId(&app_id.into()) {
        Ok(n) => n,
        Err(e) => {
            log::warn!("failed to create toast notifier: {e}");
            return None;
        }
    };

    if let Err(e) = notifier.Show(&toast) {
        log::warn!("failed to show toast: {e}");
        return None;
    }

    Some(toast)
}

/// What a click on a Windows toast asks for. A pane this process notified
/// about is revealed; anything else — a toast from an earlier anterm, one
/// without a pane, a launch string we do not recognize — still brings anterm
/// forward, which is what the user clicked a anterm toast for.
#[cfg(any(target_os = "windows", test))]
fn toast_click_action(launch: &str) -> NotificationActivation {
    match leaf_id_in_identifier(launch) {
        Some(leaf_id) => NotificationActivation::RevealPane { leaf_id },
        None => NotificationActivation::ShowWindow,
    }
}

/// The last few `launch` strings acted on. A click on the popup can arrive
/// twice — through COM and through the toast object's `Activated` event — and
/// must reveal once. Identifiers are unique per toast, so a repeat is that
/// same click; an empty one (a toast without a pane) is never deduplicated.
#[cfg(any(target_os = "windows", test))]
#[derive(Default)]
struct RecentActivations(VecDeque<String>);

#[cfg(any(target_os = "windows", test))]
impl RecentActivations {
    const KEEP: usize = 8;

    fn first_time(&mut self, launch: &str) -> bool {
        if launch.is_empty() {
            return true;
        }
        if self.0.iter().any(|seen| seen == launch) {
            return false;
        }
        if self.0.len() == Self::KEEP {
            self.0.pop_front();
        }
        self.0.push_back(launch.to_string());
        true
    }
}

/// A click on a anterm toast, from whichever path delivered it. Runs on a COM or
/// WinRT callback thread, so it only touches the activation bus — queued there
/// when this process has no dispatch loop yet (a launch COM made to deliver
/// the click).
#[cfg(target_os = "windows")]
#[cfg_attr(test, allow(dead_code))]
pub fn toast_activated(launch: &str) {
    static RECENT: Mutex<RecentActivations> = Mutex::new(RecentActivations(VecDeque::new()));
    let first = RECENT
        .lock()
        .map(|mut recent| recent.first_time(launch))
        .unwrap_or(true);
    if first {
        dispatch_activation(toast_click_action(launch));
    }
}

/// Ask the activation bus to bring `leaf_id` to the front. Runs on whatever
/// thread the platform hands the activation to, so it only touches the bus.
#[cfg(target_os = "macos")]
fn reveal_pane(leaf_id: u64) {
    dispatch_activation(NotificationActivation::RevealPane { leaf_id });
}

#[cfg(not(any(target_os = "macos", all(target_os = "windows", not(test)))))]
fn try_clickable_notification(_title: &str, _body: &str, _leaf_id: u64) -> bool {
    // Linux notifications go through notify-rust; click-to-reveal would need a
    // D-Bus action listener of its own.
    false
}

#[cfg(any(target_os = "windows", test))]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notification_identifier_carries_its_pane_back_out() {
        let id = notification_identifier(42);
        assert_eq!(leaf_id_in_identifier(&id), Some(42));
    }

    #[test]
    fn two_notifications_for_one_pane_do_not_replace_each_other() {
        // The center replaces a delivered notification whose identifier repeats.
        assert_ne!(notification_identifier(7), notification_identifier(7));
    }

    #[test]
    fn a_foreign_identifier_reveals_nothing() {
        let pid = std::process::id();
        for id in [
            String::new(),
            "anterm-pane-".into(),
            "anterm-pane-x-1".into(),
            "anterm-pane-42".into(),
            "other-42-1".into(),
            format!("anterm-pane-{pid}"),
            format!("anterm-pane-{pid}-42"),
            format!("anterm-pane-{pid}-x-1"),
        ] {
            assert_eq!(leaf_id_in_identifier(&id), None, "{id:?}");
        }
    }

    #[test]
    fn a_notification_from_another_process_reveals_nothing() {
        let other_pid = std::process::id().wrapping_add(1);
        let stale = format!("{NOTIFICATION_ID_PREFIX}{other_pid}-42-0");
        assert_eq!(leaf_id_in_identifier(&stale), None);
    }

    #[test]
    fn sanitizing_drops_control_bytes_but_keeps_line_breaks() {
        let raw = "build \x1b[31mfailed\x07\nsee log\ttail";
        assert_eq!(
            sanitize_notification_text(raw, NOTIFY_BODY_MAX),
            "build [31mfailed\nsee log\ttail"
        );
    }

    #[test]
    fn sanitizing_clamps_by_chars_not_bytes() {
        let out = sanitize_notification_text(&"账".repeat(200), 8);
        assert_eq!(out, format!("{}…", "账".repeat(8)));
        assert_eq!(sanitize_notification_text("short", 8), "short");
    }

    /// Pulls the `launch` attribute back out the way the shell would see it.
    fn launch_attribute(xml: &str) -> String {
        let start = xml
            .find(r#"launch=""#)
            .expect("toast has a launch attribute")
            + 8;
        let len = xml[start..].find('"').expect("closed attribute");
        xml[start..start + len].to_string()
    }

    /// #1179: a toast clicked from notification history is only reported over
    /// COM, as the toast's `launch` string. Without one the pane is lost.
    #[test]
    fn a_toast_carries_its_pane_in_the_launch_attribute() {
        let xml = toast_xml("title", "body", &notification_identifier(42));
        assert_eq!(
            toast_click_action(&launch_attribute(&xml)),
            NotificationActivation::RevealPane { leaf_id: 42 }
        );
    }

    #[test]
    fn a_toast_from_another_process_only_shows_the_window() {
        let other_pid = std::process::id().wrapping_add(1);
        let stale = format!("{NOTIFICATION_ID_PREFIX}{other_pid}-42-0");
        let xml = toast_xml("title", "body", &stale);
        assert_eq!(
            toast_click_action(&launch_attribute(&xml)),
            NotificationActivation::ShowWindow
        );
        assert_eq!(toast_click_action(""), NotificationActivation::ShowWindow);
    }

    #[test]
    fn one_click_delivered_twice_reveals_once() {
        let mut recent = RecentActivations::default();
        let a = notification_identifier(1);
        let b = notification_identifier(1);
        assert!(recent.first_time(&a));
        assert!(!recent.first_time(&a));
        assert!(recent.first_time(&b));
        // Toasts without a pane all share the empty launch string.
        assert!(recent.first_time(""));
        assert!(recent.first_time(""));
    }

    /// The real WinRT XML parser must accept the document and hand the launch
    /// string back verbatim. Parses only; shows nothing.
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_parses_the_toast_and_keeps_its_launch_string() {
        use windows::Data::Xml::Dom::XmlDocument;
        let id = notification_identifier(7);
        let doc = XmlDocument::new().expect("XmlDocument");
        doc.LoadXml(&toast_xml("a & <b>", "\"c\" 'd'", &id).into())
            .expect("toast xml parses");
        let launch = doc
            .DocumentElement()
            .and_then(|root| root.GetAttribute(&"launch".into()))
            .expect("launch attribute");
        assert_eq!(launch.to_string(), id);
    }

    #[test]
    fn xml_escaping_covers_every_entity_once() {
        assert_eq!(
            xml_escape(r#"a & b < c > "d" 'e'"#),
            "a &amp; b &lt; c &gt; &quot;d&quot; &apos;e&apos;"
        );
    }
}
