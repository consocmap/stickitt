//! The app's icon in the macOS Dock (nothing to do with our own dock on the screen edge).
//! Without it the app is an "accessory": no Dock icon and no place in Cmd+Tab, while its
//! windows, menu bar icon and shortcuts work as before. Does nothing on other systems.

use winit::event_loop::EventLoopBuilder;

/// Starts the event loop with or without the icon.
pub fn at_launch<T: 'static>(builder: &mut EventLoopBuilder<T>, show: bool) {
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        // Always said outright: the bundle starts without an icon (LSUIElement), as otherwise
        // macOS brings a hidden one back whenever the app is opened again.
        builder.with_activation_policy(if show { ActivationPolicy::Regular } else { ActivationPolicy::Accessory });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (builder, show);
}

/// Shows or hides the icon while running. Call on the main thread. Hiding it drops the app's
/// focus a moment later, so taking that back has to wait too.
pub fn set(show: bool) {
    #[cfg(target_os = "macos")]
    {
        use objc2::runtime::{AnyObject, Bool};
        use objc2::{class, msg_send};
        // NSApplicationActivationPolicy: regular = 0, accessory = 1.
        let policy: isize = if show { 0 } else { 1 };
        // SAFETY: plain NSApplication calls, made from the event loop, which is the main thread.
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let _: Bool = msg_send![app, setActivationPolicy: policy];
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = show;
}
