use gpui::*;

actions!(herdr, [Quit, Hide, HideOthers, ShowAll, Settings]);

pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Settings, _| crate::mods::request(crate::mods::Request::Run { name: "app.settings".into(), args: serde_json::Value::Null }));
    cx.set_menus([Menu::new("Herdr").items([
        MenuItem::action("Settings", Settings),
        MenuItem::separator(),
        MenuItem::os_submenu("Services", SystemMenuType::Services),
        MenuItem::separator(),
        MenuItem::action("Hide Herdr", Hide),
        MenuItem::action("Hide Others", HideOthers),
        MenuItem::action("Show All", ShowAll),
        MenuItem::separator(),
        MenuItem::action("Quit Herdr", Quit),
    ])]);
    follow_reduce_motion(cx);
}

/// Re-reads Accessibility > Display > Reduce motion into `App::reduce_motion`
/// (`gpui_base` reads it once at start) each time macOS says the display
/// options changed, for the life of the app. The cursor trails go by it.
fn follow_reduce_motion(cx: &mut App) {
    use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
    use objc2_foundation::{NSNotification, NSOperationQueue};
    let (tx, rx) = async_channel::unbounded::<()>();
    let changed = block2::RcBlock::new(move |_: std::ptr::NonNull<NSNotification>| {
        let _ = tx.try_send(());
    });
    // SAFETY: the name is AppKit's constant, there is no object to filter on,
    // and the block runs on the main queue; it only sends on a channel.
    let observer = unsafe {
        NSWorkspace::sharedWorkspace().notificationCenter().addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &changed,
        )
    };
    // Observed until the app quits.
    std::mem::forget(observer);
    cx.spawn(async move |cx| {
        while rx.recv().await.is_ok() {
            cx.update(gpui_base::apply_system_reduce_motion);
        }
    })
    .detach();
}

pub fn action(name: &str) -> Option<Box<dyn Action>> {
    match name {
        "app.quit" => Some(Box::new(Quit)),
        "app.hide" => Some(Box::new(Hide)),
        "app.hide_others" => Some(Box::new(HideOthers)),
        "app.show_all" => Some(Box::new(ShowAll)),
        "app.settings" => Some(Box::new(Settings)),
        _ => None,
    }
}
