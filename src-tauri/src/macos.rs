use crate::{model::Settings, Shared};
use objc2::{define_class, msg_send, rc::Retained, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSColor, NSEvent, NSEventType, NSFloatingWindowLevel,
    NSNormalWindowLevel, NSPanel, NSScreen, NSWindow, NSWindowCollectionBehavior as Behavior,
    NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect};
use std::cell::RefCell;
use tauri::Manager;

define_class!(
    #[unsafe(super = NSPanel)]
    #[thread_kind = MainThreadOnly]
    struct LyricsPanel;
    impl LyricsPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool { false }
        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool { false }
    }
);

thread_local! {
    static PANEL: RefCell<Option<Retained<LyricsPanel>>> = const { RefCell::new(None) };
}

fn panel_behavior(modern: bool) -> Behavior {
    Behavior::CanJoinAllSpaces
        | if modern {
            Behavior::CanJoinAllApplications
        } else {
            Behavior::FullScreenAuxiliary
        }
}

fn fits_screen(origin: NSPoint, screen: NSRect, size: objc2_foundation::NSSize) -> bool {
    origin.x.is_finite()
        && origin.y.is_finite()
        && origin.x >= screen.origin.x
        && origin.y >= screen.origin.y
        && origin.x + size.width <= screen.origin.x + screen.size.width
        && origin.y + size.height <= screen.origin.y + screen.size.height
}

pub fn configure_overlay(
    window: &tauri::WebviewWindow,
    settings: &Settings,
) -> Result<(), Box<dyn std::error::Error>> {
    let mtm = MainThreadMarker::new().ok_or("浮窗必須在主執行緒設定")?;
    let pointer = window.ns_window()?;
    // Tauri keeps ownership of the hidden host and webview for IPC and shutdown.
    let original = unsafe { (pointer as *const NSWindow).as_ref() }.ok_or("找不到原生浮窗")?;
    let content = original.contentView().ok_or("找不到歌詞畫面")?;
    let allocated = LyricsPanel::alloc(mtm).set_ivars(());
    // Create a real panel with nonactivation enabled from birth, rather than changing a window's class.
    let panel: Retained<LyricsPanel> = unsafe {
        msg_send![super(allocated), initWithContentRect: original.frame(),
            styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            backing: NSBackingStoreType::Buffered, defer: false]
    };
    unsafe {
        panel.setReleasedWhenClosed(false);
    }
    panel.setTitle(&original.title());
    panel.setFloatingPanel(true);
    panel.setBecomesKeyOnlyIfNeeded(true);
    panel.setHidesOnDeactivate(false);
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
    panel.setCollectionBehavior(panel_behavior(objc2::available!(macos = 13.0, ..)));
    if let Some((x, y)) = settings.panel_x.zip(settings.panel_y) {
        let origin = NSPoint::new(x, y);
        if NSScreen::screens(mtm)
            .iter()
            .any(|screen| fits_screen(origin, screen.visibleFrame(), panel.frame().size))
        {
            panel.setFrameOrigin(origin);
        } else {
            panel.center();
        }
    }
    original.setContentView(None);
    panel.setContentView(Some(&content));
    original.orderOut(None);
    PANEL.with(|slot| *slot.borrow_mut() = Some(panel));
    Ok(())
}

pub fn apply_overlay(
    app: &tauri::AppHandle,
    settings: Settings,
    editing: bool,
) -> Result<(), String> {
    app.run_on_main_thread(move || {
        PANEL.with(|slot| {
            if let Some(panel) = slot.borrow().as_ref() {
                panel.setLevel(if settings.always_on_top {
                    NSFloatingWindowLevel
                } else {
                    NSNormalWindowLevel
                });
                panel.setIgnoresMouseEvents(settings.click_through && !editing);
                if settings.overlay_enabled {
                    panel.orderFrontRegardless();
                } else {
                    panel.orderOut(None);
                }
            }
        })
    })
    .map_err(|e| e.to_string())
}

fn save_position(app: &tauri::AppHandle, panel: &LyricsPanel) {
    let state = app.state::<Shared>();
    let mut data = state.data.lock().unwrap();
    // Native panel positions use AppKit logical points and a bottom-left origin.
    let origin = panel.frame().origin;
    data.settings.panel_x = Some(origin.x);
    data.settings.panel_y = Some(origin.y);
    if state.db.save_settings(&data.settings).is_err() {
        data.translation_message = Some("無法儲存視窗位置".into());
    }
}

pub fn drag(app: &tauri::AppHandle) -> Result<(), String> {
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let mtm = MainThreadMarker::new().expect("AppKit main thread");
        let panel = PANEL.with(|slot| slot.borrow().clone());
        let Some(panel) = panel else { return; };
        if panel.ignoresMouseEvents() || !panel.isVisible() { return; }
        if let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() {
            // IPC can arrive after mouseDown; synthesize a drag event for this panel, not the hidden host.
            if let Some(drag_event) = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                NSEventType::LeftMouseDown, panel.convertPointFromScreen(NSEvent::mouseLocation()),
                event.modifierFlags(), event.timestamp(), panel.windowNumber(), None, 0, 1, 1.0,
            ) {
                panel.performWindowDragWithEvent(&drag_event);
                save_position(&handle, &panel);
            }
        }
    }).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::NSSize;
    #[test]
    fn panel_uses_reference_projects_fullscreen_behavior() {
        assert_eq!(
            panel_behavior(true),
            Behavior::CanJoinAllSpaces | Behavior::CanJoinAllApplications
        );
        assert_eq!(
            panel_behavior(false),
            Behavior::CanJoinAllSpaces | Behavior::FullScreenAuxiliary
        );
    }
    #[test]
    fn restore_native_position_handles_secondary_screens_and_disconnection() {
        let screen = NSRect::new(NSPoint::new(-1440.0, 0.0), NSSize::new(1440.0, 900.0));
        let size = NSSize::new(540.0, 120.0);
        assert!(fits_screen(NSPoint::new(-1000.0, 400.0), screen, size));
        assert!(!fits_screen(NSPoint::new(0.0, 0.0), screen, size));
        assert!(!fits_screen(NSPoint::new(f64::NAN, 0.0), screen, size));
    }
}
