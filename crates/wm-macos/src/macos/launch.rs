//! `Close` and `LaunchNewWindow` (spec 5.6 and 6).

use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_application_services::{AXError, AXUIElement};
use std::path::Path;
use std::time::Instant;

use objc2::rc::Retained;
use objc2_core_foundation::{CFNumber, CFRetained};
use objc2_foundation::{NSBundle, NSString, NSURL};
use wm_core::{App, WindowId};

use super::ax;
use super::windows::WindowTable;
use crate::apps::{bundle_or_name, file_id, open_fresh, pick_running};
use crate::classify::may_close;
use crate::limits::{Budget, MENU_BAR_ITEMS, MENU_ITEMS};
use crate::wm_log;

/// `Close(id)`: press the window's close button. The app may refuse (unsaved changes);
/// core waits for `WindowDestroyed`.
///
/// It acts only on the frontmost app's focused window (HARD-2): the only command checked with
/// macOS first, because it's the only one that can't be undone with the next keypress.
pub(crate) fn close(table: &WindowTable, id: WindowId, active_pid: Option<i32>) {
    let Some(w) = table.get(id) else {
        wm_log!("Close for unknown {id:?}");
        return;
    };
    let is_apps_focused_window = ax::attr(&ax::app_element(w.pid), "AXFocusedWindow")
        .ok()
        .and_then(|v| v.downcast::<AXUIElement>().ok())
        .is_some_and(|focused| objc2_core_foundation::CFEqual(Some(&*focused), Some(&*w.element)));
    if !may_close(w.pid, active_pid, is_apps_focused_window) {
        wm_log!("Close {id:?}: not the window macOS has focused; ignored (HARD-2)");
        return;
    }
    let button = ax::attr(&w.element, "AXCloseButton")
        .ok()
        .and_then(|v| v.downcast::<AXUIElement>().ok());
    match button {
        Some(button) => {
            let err = ax::perform(&button, "AXPress");
            // "Cannot complete" is common here: the app often closes the window before it
            // answers. Whether it really closed is known from `WindowDestroyed`.
            if err != AXError::Success && err != AXError::CannotComplete {
                wm_log!("Close {id:?}: pressing the close button failed: {err:?}");
            }
        }
        None => wm_log!("Close {id:?}: the window has no close button"),
    }
}

/// `LaunchNewWindow(app)`: start the app if it isn't running (it opens its own first
/// window); otherwise bring it forward and press its ⌘N menu item. `Err` says why nothing
/// could be opened (the app isn't installed), for the log and the banner.
///
/// The app is resolved once, to one copy on disk, and that copy is both what gets launched
/// and what a running app must be, by file identity, to receive the ⌘N (SAFE-6).
pub(crate) fn launch_new_window(app: &App) -> Result<(), String> {
    let workspace = NSWorkspace::sharedWorkspace();
    let url = resolve(&workspace, app)?;
    let path = url
        .path()
        .map(|p| p.to_string())
        .ok_or("the app's location isn't a file path")?;
    let bundle = bundle_id_at(&url).ok_or_else(|| format!("{path} has no bundle id"))?;
    let installed = file_id(Path::new(&path));
    let bundle_id = NSString::from_str(&bundle);
    // Any app can claim a bundle id: only the resolved copy counts.
    let running: Vec<_> = NSRunningApplication::runningApplicationsWithBundleIdentifier(&bundle_id)
        .iter()
        .map(|app| {
            let id = app
                .bundleURL()
                .and_then(|u| u.path())
                .and_then(|p| file_id(Path::new(&p.to_string())));
            (app, id)
        })
        .collect();
    let Some(running) = pick_running(&running, installed) else {
        let config = NSWorkspaceOpenConfiguration::configuration();
        // Other copies claiming the id would otherwise make macOS skip the launch (SAFE-6).
        config.setCreatesNewApplicationInstance(open_fresh(!running.is_empty()));
        workspace.openApplicationAtURL_configuration_completionHandler(&url, &config, None);
        return Ok(());
    };
    let app_element = ax::app_element(running.processIdentifier());
    ax::set_bool(&app_element, "AXFrontmost", true);
    match find_new_window_item(&app_element) {
        Some(item) => {
            let err = ax::perform(&item, "AXPress");
            if err != AXError::Success {
                wm_log!("LaunchNewWindow: pressing ⌘N in {bundle} failed: {err:?}");
            }
        }
        None => wm_log!("LaunchNewWindow: no ⌘N menu item in {bundle}"),
    }
    Ok(())
}

/// Where the app `app` refers to is right now (CFG-3): one copy, by location.
fn resolve(workspace: &NSWorkspace, app: &App) -> Result<Retained<NSURL>, String> {
    match app {
        App::Bundle(id) => bundle_or_name(
            id,
            |id| workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(id)),
            app_named,
        ),
        App::Browser => {
            // Whatever opens https links is the default browser.
            let probe = NSURL::URLWithString(&NSString::from_str("https://example.com"))
                .ok_or("could not build a URL to find the default browser")?;
            workspace
                .URLForApplicationToOpenURL(&probe)
                .ok_or_else(|| "no default web browser is set".to_string())
        }
        App::Named(name) => {
            app_named(name).ok_or_else(|| format!("no app named \"{name}\" is installed"))
        }
    }
}

/// `<name>.app` in the usual application folders, the first found.
fn app_named(name: &str) -> Option<Retained<NSURL>> {
    let home = std::env::var("HOME").unwrap_or_default();
    let folders = [
        "/Applications".to_string(),
        "/Applications/Utilities".to_string(),
        "/System/Applications".to_string(),
        "/System/Applications/Utilities".to_string(),
        format!("{home}/Applications"),
    ];
    folders.iter().find_map(|folder| {
        let path = format!("{folder}/{name}.app");
        Path::new(&path)
            .exists()
            .then(|| NSURL::fileURLWithPath(&NSString::from_str(&path)))
    })
}

/// The bundle id of the app bundle at `url`.
fn bundle_id_at(url: &NSURL) -> Option<String> {
    NSBundle::bundleWithURL(url)?
        .bundleIdentifier()
        .map(|id| id.to_string())
}

/// The menu item whose shortcut is exactly ⌘N. Matching the shortcut, not the title, keeps
/// this independent of the menu language.
fn find_new_window_item(app: &AXUIElement) -> Option<CFRetained<AXUIElement>> {
    let menu_bar = ax::attr(app, "AXMenuBar")
        .ok()?
        .downcast::<AXUIElement>()
        .ok()?;
    // Menu bar → menu bar items → their menu → its items (submenus aren't searched). The
    // app decides how big its menus are, so the walk is bounded (SAFE-5).
    let budget = Budget::starting(Instant::now());
    for bar_item in ax::elements_attr(&menu_bar, "AXChildren")
        .into_iter()
        .take(MENU_BAR_ITEMS)
    {
        for menu in ax::elements_attr(&bar_item, "AXChildren") {
            if budget.spent(Instant::now()) {
                wm_log!("LaunchNewWindow: gave up looking for ⌘N after 250 ms (SAFE-5)");
                return None;
            }
            for item in ax::elements_attr(&menu, "AXChildren")
                .into_iter()
                .take(MENU_ITEMS)
            {
                let char_is_n = ax::string_attr(&item, "AXMenuItemCmdChar").as_deref() == Some("N");
                // Modifiers 0 means ⌘ alone.
                let command_only = ax::attr(&item, "AXMenuItemCmdModifiers")
                    .ok()
                    .and_then(|v| v.downcast::<CFNumber>().ok())
                    .and_then(|n| n.as_i64())
                    == Some(0);
                if char_is_n && command_only {
                    return Some(item);
                }
            }
        }
    }
    None
}
