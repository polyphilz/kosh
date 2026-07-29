#![cfg_attr(feature = "test-support", allow(dead_code))]

use std::sync::Mutex;

use objc2::MainThreadMarker as ObjcMainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSRunningApplication, NSWindow,
    NSWindowCollectionBehavior, NSWorkspace,
};
use serde::Serialize;
use tauri::{
    menu::{Menu, MenuBuilder},
    tray::TrayIconBuilder,
    utils::config::BackgroundThrottlingPolicy,
    App, AppHandle, Emitter, Manager, PhysicalPosition, State, WebviewUrl, WebviewWindowBuilder,
    WindowEvent,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::{
    database::{
        validate_complete_bindings, KeyboardBinding, KoshCommand, SetShortcutSettingsInput,
        ShortcutSettings, DEFAULT_MAIN_WINDOW_ACCELERATOR, DEFAULT_QUICK_ADD_ACCELERATOR,
    },
    runtime::RuntimeState,
};

const MAIN_LABEL: &str = "main";
const QUICK_ADD_LABEL: &str = "quick-add";
const TRAY_ID: &str = "kosh-tray";
const QUICK_ADD_SHOWN_EVENT: &str = "kosh://quick-add-shown";
const OPEN_SETTINGS_EVENT: &str = "kosh://open-settings";
const SHORTCUT_SETTINGS_CHANGED_EVENT: &str = "kosh://shortcut-settings-changed";

#[derive(Clone, Copy)]
enum TrayAction {
    ShowMain,
    ShowSettings,
    ShowQuickAdd,
    Quit,
}

impl TrayAction {
    const fn id(self) -> &'static str {
        match self {
            Self::ShowMain => "show-main",
            Self::ShowSettings => "show-settings",
            Self::ShowQuickAdd => "show-quick-add",
            Self::Quit => "quit",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        [
            Self::ShowMain,
            Self::ShowSettings,
            Self::ShowQuickAdd,
            Self::Quit,
        ]
        .into_iter()
        .find(|action| action.id() == id)
    }
}

#[derive(Default)]
struct FocusContext {
    dismissing: bool,
    file_dialog_open: bool,
    previous_external_pid: Option<i32>,
    quick_add_visible: bool,
    restore_main: bool,
}

impl FocusContext {
    fn begin_show(&mut self, current_pid: i32, frontmost_pid: Option<i32>, main_focused: bool) {
        if self.quick_add_visible {
            return;
        }
        self.previous_external_pid = frontmost_pid.filter(|pid| *pid != current_pid);
        self.restore_main = frontmost_pid == Some(current_pid) && main_focused;
        self.quick_add_visible = true;
    }

    fn begin_dismiss(&mut self) -> Option<RestoreTarget> {
        if self.dismissing || !self.quick_add_visible {
            return None;
        }
        self.dismissing = true;
        self.file_dialog_open = false;
        self.quick_add_visible = false;
        Some(RestoreTarget {
            external_pid: self.previous_external_pid.take(),
            main: std::mem::take(&mut self.restore_main),
        })
    }

    fn finish_dismiss(&mut self) {
        self.dismissing = false;
    }

    const fn should_dismiss_on_focus_loss(&self) -> bool {
        self.quick_add_visible && !self.dismissing && !self.file_dialog_open
    }
}

#[derive(Default)]
struct RestoreTarget {
    external_pid: Option<i32>,
    main: bool,
}

#[derive(Clone, Copy)]
enum DismissFocus {
    RestorePrevious,
    PreserveCurrent,
}

#[derive(Default)]
pub(crate) struct WindowState {
    focus: Mutex<FocusContext>,
    shortcut_errors: Mutex<Vec<String>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShortcutSettingsSnapshot {
    #[serde(flatten)]
    settings: ShortcutSettings,
    shortcut_errors: Vec<String>,
}

pub(crate) fn setup(app: &mut App, settings: ShortcutSettings) -> tauri::Result<()> {
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    app.manage(WindowState::default());
    create_quick_add_window(app.handle())?;
    install_tray(app, &settings.keyboard_bindings)?;
    let errors = register_shortcuts(app.handle(), &settings.keyboard_bindings);
    *app.state::<WindowState>()
        .shortcut_errors
        .lock()
        .expect("shortcut errors poisoned") = errors;
    Ok(())
}

fn create_quick_add_window(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(QUICK_ADD_LABEL).is_some() {
        return Ok(());
    }
    let window = WebviewWindowBuilder::new(
        app,
        QUICK_ADD_LABEL,
        WebviewUrl::App("quick-add.html".into()),
    )
    .title("Kosh Quick Add")
    .inner_size(780.0, 680.0)
    .visible(false)
    .focused(false)
    .focusable(true)
    .decorations(false)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .shadow(true)
    .transparent(true)
    .background_throttling(BackgroundThrottlingPolicy::Disabled)
    .build()?;

    window.with_webview(|webview| unsafe {
        // SAFETY: Tauri supplies the main-thread NSWindow owned by this webview
        // for the duration of the closure.
        let ns_window: &NSWindow = &*webview.ns_window().cast();
        ns_window.setCollectionBehavior(
            NSWindowCollectionBehavior::MoveToActiveSpace
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Transient
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
    })?;
    Ok(())
}

fn tray_menu(app: &AppHandle, bindings: &[KeyboardBinding]) -> tauri::Result<Menu<tauri::Wry>> {
    let quick_add = binding_for(bindings, KoshCommand::QuickAdd)
        .map(|binding| shortcut_label(&binding.accelerator))
        .unwrap_or_else(|| shortcut_label(DEFAULT_QUICK_ADD_ACCELERATOR));
    let main_window = binding_for(bindings, KoshCommand::MainWindow)
        .map(|binding| shortcut_label(&binding.accelerator))
        .unwrap_or_else(|| shortcut_label(DEFAULT_MAIN_WINDOW_ACCELERATOR));
    MenuBuilder::new(app)
        .text(
            TrayAction::ShowMain.id(),
            format!("Open Kosh  {main_window}"),
        )
        .text(TrayAction::ShowSettings.id(), "Settings…")
        .text(
            TrayAction::ShowQuickAdd.id(),
            format!("Quick Add  {quick_add}"),
        )
        .separator()
        .text(TrayAction::Quit.id(), "Quit Kosh")
        .build()
}

fn install_tray(app: &App, bindings: &[KeyboardBinding]) -> tauri::Result<()> {
    let menu = tray_menu(app.handle(), bindings)?;
    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .title("k")
        .tooltip("Kosh")
        .icon_as_template(true)
        .on_menu_event(
            |app, event| match TrayAction::from_id(event.id().as_ref()) {
                Some(TrayAction::ShowMain) => {
                    dispatch_logged(app, "show main window", show_main_inner)
                }
                Some(TrayAction::ShowSettings) => {
                    dispatch_logged(app, "show settings", show_settings_inner)
                }
                Some(TrayAction::ShowQuickAdd) => {
                    dispatch_logged(app, "show quick add", show_quick_add_inner)
                }
                Some(TrayAction::Quit) => app.exit(0),
                None => {}
            },
        );
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn update_tray(app: &AppHandle, bindings: &[KeyboardBinding]) -> Result<(), String> {
    let menu = tray_menu(app, bindings).map_err(|error| error.to_string())?;
    let tray = app
        .tray_by_id(TRAY_ID)
        .ok_or_else(|| "Kosh menu-bar icon is unavailable".to_string())?;
    tray.set_menu(Some(menu)).map_err(|error| error.to_string())
}

fn register_shortcuts(app: &AppHandle, bindings: &[KeyboardBinding]) -> Vec<String> {
    bindings
        .iter()
        .filter_map(|binding| {
            register_binding(app, binding).err().map(|error| {
                let message = format!(
                    "Could not register {}: {error}",
                    shortcut_label(&binding.accelerator)
                );
                log::error!("{message}");
                message
            })
        })
        .collect()
}

fn register_binding(app: &AppHandle, binding: &KeyboardBinding) -> Result<(), String> {
    let shortcut = binding
        .accelerator
        .parse::<Shortcut>()
        .map_err(|error| format!("invalid shortcut: {error}"))?;
    match binding.command {
        KoshCommand::QuickAdd => app
            .global_shortcut()
            .on_shortcut(shortcut, |app, _shortcut, event| {
                if event.state() == ShortcutState::Pressed {
                    dispatch_logged(app, "show quick add", show_quick_add_inner);
                }
            })
            .map_err(|error| error.to_string()),
        KoshCommand::MainWindow => app
            .global_shortcut()
            .on_shortcut(shortcut, |app, _shortcut, event| {
                if event.state() == ShortcutState::Pressed {
                    dispatch_logged(app, "show main window", show_main_inner);
                }
            })
            .map_err(|error| error.to_string()),
    }
}

fn unregister_binding(app: &AppHandle, binding: &KeyboardBinding) -> Result<(), String> {
    if app
        .global_shortcut()
        .is_registered(binding.accelerator.as_str())
    {
        app.global_shortcut()
            .unregister(binding.accelerator.as_str())
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn replace_bindings<Unregister, Register>(
    current: &[KeyboardBinding],
    candidate: &[KeyboardBinding],
    mut unregister: Unregister,
    mut register: Register,
) -> Result<(), String>
where
    Unregister: FnMut(&KeyboardBinding) -> Result<(), String>,
    Register: FnMut(&KeyboardBinding) -> Result<(), String>,
{
    let mut removed = Vec::new();
    for binding in current {
        if let Err(error) = unregister(binding) {
            for removed_binding in removed {
                if let Err(restore_error) = register(removed_binding) {
                    log::error!(
                        "failed to restore {}: {restore_error}",
                        removed_binding.accelerator
                    );
                }
            }
            return Err(format!(
                "could not replace {}: {error}",
                shortcut_label(&binding.accelerator)
            ));
        }
        removed.push(binding);
    }
    let mut installed: Vec<&KeyboardBinding> = Vec::new();
    for binding in candidate {
        if let Err(error) = register(binding) {
            for installed_binding in installed.iter().rev() {
                let _ = unregister(installed_binding);
            }
            for old_binding in current {
                if let Err(restore_error) = register(old_binding) {
                    log::error!(
                        "failed to restore {}: {restore_error}",
                        old_binding.accelerator
                    );
                }
            }
            return Err(format!(
                "{} is unavailable: {error}",
                shortcut_label(&binding.accelerator)
            ));
        }
        installed.push(binding);
    }
    Ok(())
}

fn replace_runtime_shortcuts(
    app: &AppHandle,
    current: &[KeyboardBinding],
    candidate: &[KeyboardBinding],
) -> Result<(), String> {
    replace_bindings(
        current,
        candidate,
        |binding| unregister_binding(app, binding),
        |binding| register_binding(app, binding),
    )
}

fn restore_runtime_shortcuts(
    app: &AppHandle,
    candidate: &[KeyboardBinding],
    current: &[KeyboardBinding],
) {
    for binding in candidate {
        if let Err(error) = unregister_binding(app, binding) {
            log::error!("failed to unregister candidate shortcut: {error}");
        }
    }
    for binding in current {
        if let Err(error) = register_binding(app, binding) {
            log::error!("failed to restore {}: {error}", binding.accelerator);
        }
    }
}

fn binding_for(bindings: &[KeyboardBinding], command: KoshCommand) -> Option<&KeyboardBinding> {
    bindings.iter().find(|binding| binding.command == command)
}

fn shortcut_label(accelerator: &str) -> String {
    let Ok(shortcut) = accelerator.parse::<Shortcut>() else {
        return accelerator.to_owned();
    };
    let mut label = String::new();
    if shortcut.mods.contains(Modifiers::CONTROL) {
        label.push('⌃');
    }
    if shortcut.mods.contains(Modifiers::ALT) {
        label.push('⌥');
    }
    if shortcut.mods.contains(Modifiers::SHIFT) {
        label.push('⇧');
    }
    if shortcut.mods.contains(Modifiers::SUPER) {
        label.push('⌘');
    }
    let key = shortcut.key.to_string();
    label.push_str(
        key.strip_prefix("Key")
            .or_else(|| key.strip_prefix("Digit"))
            .unwrap_or(&key),
    );
    label
}

fn snapshot(app: &AppHandle, settings: ShortcutSettings) -> ShortcutSettingsSnapshot {
    ShortcutSettingsSnapshot {
        settings,
        shortcut_errors: app
            .state::<WindowState>()
            .shortcut_errors
            .lock()
            .expect("shortcut errors poisoned")
            .clone(),
    }
}

#[tauri::command]
pub(crate) async fn load_shortcut_settings(
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<ShortcutSettingsSnapshot, String> {
    let client = state.database_client();
    let settings = tauri::async_runtime::spawn_blocking(move || client.load_shortcut_settings())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    Ok(snapshot(&app, settings))
}

#[tauri::command]
pub(crate) async fn set_shortcut_settings(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    input: SetShortcutSettingsInput,
) -> Result<ShortcutSettingsSnapshot, String> {
    validate_complete_bindings(&input.keyboard_bindings).map_err(|error| error.to_string())?;
    let client = state.database_client();
    let current_client = client.clone();
    let current =
        tauri::async_runtime::spawn_blocking(move || current_client.load_shortcut_settings())
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
    if current.revision != input.expected_revision {
        return Err(format!(
            "shortcut settings changed before this update: revision is {}, expected {}",
            current.revision, input.expected_revision
        ));
    }
    replace_runtime_shortcuts(&app, &current.keyboard_bindings, &input.keyboard_bindings)?;
    if let Err(error) = update_tray(&app, &input.keyboard_bindings) {
        restore_runtime_shortcuts(&app, &input.keyboard_bindings, &current.keyboard_bindings);
        return Err(format!("Could not update shortcut labels: {error}"));
    }

    let candidate = input.keyboard_bindings.clone();
    let persisted =
        match tauri::async_runtime::spawn_blocking(move || client.set_shortcut_settings(input))
            .await
            .map_err(|error| error.to_string())?
        {
            Ok(settings) => settings,
            Err(error) => {
                restore_runtime_shortcuts(&app, &candidate, &current.keyboard_bindings);
                if let Err(menu_error) = update_tray(&app, &current.keyboard_bindings) {
                    log::error!("failed to restore tray shortcut labels: {menu_error}");
                }
                return Err(error.to_string());
            }
        };
    app.state::<WindowState>()
        .shortcut_errors
        .lock()
        .expect("shortcut errors poisoned")
        .clear();
    let snapshot = snapshot(&app, persisted);
    if let Err(error) = app.emit_to(MAIN_LABEL, SHORTCUT_SETTINGS_CHANGED_EVENT, &snapshot) {
        log::error!("could not publish shortcut settings: {error}");
    }
    Ok(snapshot)
}

#[tauri::command]
pub(crate) fn show_quick_add(app: AppHandle) -> Result<(), String> {
    dispatch_to_main_thread(&app, "show quick add", show_quick_add_inner)
}

fn show_quick_add_inner(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window(QUICK_ADD_LABEL)
        .ok_or_else(|| "quick-add window is unavailable".to_string())?;
    let current_pid = std::process::id() as i32;
    let frontmost_pid = frontmost_application_pid();
    let main_focused = app
        .get_webview_window(MAIN_LABEL)
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false);
    app.state::<WindowState>()
        .focus
        .lock()
        .expect("focus context poisoned")
        .begin_show(current_pid, frontmost_pid, main_focused);
    position_quick_add_on_cursor_monitor(app)?;

    let result = (|| {
        window
            .show()
            .map_err(|error| format!("could not show quick add: {error}"))?;
        activate_quick_add_window(&window)?;
        app.emit_to(QUICK_ADD_LABEL, QUICK_ADD_SHOWN_EVENT, ())
            .map_err(|error| format!("could not focus the quick-add editor: {error}"))
    })();
    if let Err(error) = result {
        if let Err(cleanup_error) = dismiss_quick_add_inner(app, DismissFocus::RestorePrevious) {
            log::error!("failed to clean up Quick Add after show error: {cleanup_error}");
        }
        return Err(error);
    }
    Ok(())
}

fn activate_quick_add_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    let marker = ObjcMainThreadMarker::new()
        .ok_or_else(|| "quick-add activation was not on the main thread".to_string())?;
    let application = NSApplication::sharedApplication(marker);
    application.activate();
    #[allow(deprecated)]
    application.activateIgnoringOtherApps(true);
    window
        .set_focus()
        .map_err(|error| format!("could not focus quick add: {error}"))
}

fn position_quick_add_on_cursor_monitor(app: &AppHandle) -> Result<(), String> {
    let cursor = app
        .cursor_position()
        .map_err(|error| format!("could not read the cursor position: {error}"))?;
    let monitor = app
        .monitor_from_point(cursor.x, cursor.y)
        .map_err(|error| format!("could not find the cursor monitor: {error}"))?
        .or_else(|| app.primary_monitor().ok().flatten())
        .ok_or_else(|| "no monitor is available for quick add".to_string())?;
    let window = app
        .get_webview_window(QUICK_ADD_LABEL)
        .ok_or_else(|| "quick-add window is unavailable".to_string())?;
    let window_size = window
        .outer_size()
        .map_err(|error| format!("could not read the quick-add size: {error}"))?;
    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let x =
        monitor_position.x + ((monitor_size.width as i32 - window_size.width as i32) / 2).max(0);
    let top_offset = ((monitor_size.height as f64 * 0.12).round() as i32).clamp(64, 140);
    window
        .set_position(PhysicalPosition::new(x, monitor_position.y + top_offset))
        .map_err(|error| format!("could not position quick add: {error}"))
}

#[tauri::command]
pub(crate) fn dismiss_quick_add(app: AppHandle) -> Result<(), String> {
    dispatch_to_main_thread(&app, "dismiss quick add", |app| {
        dismiss_quick_add_inner(app, DismissFocus::RestorePrevious)
    })
}

#[tauri::command]
pub(crate) fn set_quick_add_file_dialog_open(state: State<'_, WindowState>, open: bool) {
    let mut context = state.focus.lock().expect("focus context poisoned");
    context.file_dialog_open = open && context.quick_add_visible;
}

fn dismiss_quick_add_inner(app: &AppHandle, focus: DismissFocus) -> Result<(), String> {
    let target = {
        let state = app.state::<WindowState>();
        let mut context = state.focus.lock().expect("focus context poisoned");
        let Some(target) = context.begin_dismiss() else {
            return Ok(());
        };
        target
    };
    let result = (|| {
        app.get_webview_window(QUICK_ADD_LABEL)
            .ok_or_else(|| "quick-add window is unavailable".to_string())?
            .hide()
            .map_err(|error| format!("could not hide quick add: {error}"))?;
        if matches!(focus, DismissFocus::RestorePrevious) {
            restore_previous_focus(app, target)?;
        }
        Ok(())
    })();
    app.state::<WindowState>()
        .focus
        .lock()
        .expect("focus context poisoned")
        .finish_dismiss();
    result
}

fn restore_previous_focus(app: &AppHandle, target: RestoreTarget) -> Result<(), String> {
    if target.main {
        return activate_main_window(app);
    }
    let current_pid = std::process::id() as i32;
    if frontmost_application_pid() == Some(current_pid) {
        if let Some(pid) = target.external_pid {
            if let Some(application) =
                NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            {
                if application.activateWithOptions(NSApplicationActivationOptions::empty()) {
                    return Ok(());
                }
            }
        }
        enter_resident_mode(app);
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn show_main(app: AppHandle) -> Result<(), String> {
    dispatch_to_main_thread(&app, "show main window", show_main_inner)
}

fn show_main_inner(app: &AppHandle) -> Result<(), String> {
    dismiss_quick_add_inner(app, DismissFocus::PreserveCurrent)?;
    activate_main_window(app)
}

fn show_settings_inner(app: &AppHandle) -> Result<(), String> {
    show_main_inner(app)?;
    app.emit_to(MAIN_LABEL, OPEN_SETTINGS_EVENT, ())
        .map_err(|error| format!("could not open Settings: {error}"))
}

fn activate_main_window(app: &AppHandle) -> Result<(), String> {
    let marker = ObjcMainThreadMarker::new()
        .ok_or_else(|| "main-window activation was not on the main thread".to_string())?;
    app.set_activation_policy(tauri::ActivationPolicy::Regular)
        .map_err(|error| format!("could not enter regular-app mode: {error}"))?;
    let window = app
        .get_webview_window(MAIN_LABEL)
        .ok_or_else(|| "main window is unavailable".to_string())?;
    window
        .unminimize()
        .map_err(|error| format!("could not unminimize the main window: {error}"))?;
    window
        .show()
        .map_err(|error| format!("could not show the main window: {error}"))?;
    let application = NSApplication::sharedApplication(marker);
    application.activate();
    #[allow(deprecated)]
    application.activateIgnoringOtherApps(true);
    window
        .set_focus()
        .map_err(|error| format!("could not focus the main window: {error}"))
}

fn frontmost_application_pid() -> Option<i32> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|application| application.processIdentifier())
}

fn enter_resident_mode(app: &AppHandle) {
    if let Some(marker) = ObjcMainThreadMarker::new() {
        NSApplication::sharedApplication(marker).deactivate();
    }
    if let Err(error) = app.set_activation_policy(tauri::ActivationPolicy::Accessory) {
        log::error!("failed to restore Accessory mode: {error}");
    }
}

fn dispatch_logged(
    app: &AppHandle,
    operation_name: &'static str,
    operation: fn(&AppHandle) -> Result<(), String>,
) {
    if let Err(error) = dispatch_to_main_thread(app, operation_name, operation) {
        log::error!("{operation_name} dispatch failed: {error}");
    }
}

fn dispatch_to_main_thread<F>(
    app: &AppHandle,
    operation_name: &'static str,
    operation: F,
) -> Result<(), String>
where
    F: FnOnce(&AppHandle) -> Result<(), String> + Send + 'static,
{
    let app_handle = app.clone();
    app.run_on_main_thread(move || {
        if let Err(error) = operation(&app_handle) {
            log::error!("{operation_name} failed on the main thread: {error}");
        }
    })
    .map_err(|error| format!("could not dispatch {operation_name}: {error}"))
}

pub(crate) fn handle_window_event(window: &tauri::Window, event: &WindowEvent) {
    match event {
        WindowEvent::CloseRequested { api, .. } => match window.label() {
            MAIN_LABEL => {
                api.prevent_close();
                if let Err(error) = window.hide() {
                    log::error!("failed to hide the main window: {error}");
                }
                enter_resident_mode(window.app_handle());
            }
            QUICK_ADD_LABEL => {
                api.prevent_close();
                if let Err(error) =
                    dismiss_quick_add_inner(window.app_handle(), DismissFocus::RestorePrevious)
                {
                    log::error!("failed to hide quick add: {error}");
                }
            }
            _ => {}
        },
        WindowEvent::Focused(false) if window.label() == QUICK_ADD_LABEL => {
            let should_dismiss = window
                .app_handle()
                .state::<WindowState>()
                .focus
                .lock()
                .expect("focus context poisoned")
                .should_dismiss_on_focus_loss();
            if should_dismiss {
                if let Err(error) =
                    dismiss_quick_add_inner(window.app_handle(), DismissFocus::PreserveCurrent)
                {
                    log::error!("failed to dismiss Quick Add after focus loss: {error}");
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::*;

    fn bindings(quick_add: &str, main_window: &str) -> Vec<KeyboardBinding> {
        vec![
            KeyboardBinding {
                command: KoshCommand::QuickAdd,
                accelerator: quick_add.into(),
            },
            KeyboardBinding {
                command: KoshCommand::MainWindow,
                accelerator: main_window.into(),
            },
        ]
    }

    #[test]
    fn repeated_show_preserves_the_original_application_restore_target() {
        let mut context = FocusContext::default();
        context.begin_show(100, Some(200), false);
        context.begin_show(100, Some(300), false);
        let target = context.begin_dismiss().expect("visible quick add");
        assert_eq!(target.external_pid, Some(200));
        assert!(!target.main);
        assert!(context.begin_dismiss().is_none());
    }

    #[test]
    fn showing_over_main_restores_main_and_file_dialog_blocks_focus_dismissal() {
        let mut context = FocusContext::default();
        context.begin_show(100, Some(100), true);
        context.file_dialog_open = true;
        assert!(!context.should_dismiss_on_focus_loss());
        context.file_dialog_open = false;
        assert!(context.should_dismiss_on_focus_loss());
        let target = context.begin_dismiss().expect("visible quick add");
        assert!(target.main);
        assert_eq!(target.external_pid, None);
    }

    #[test]
    fn registration_conflict_rolls_back_the_previous_shortcuts() {
        let current = bindings("control+KeyK", "control+KeyO");
        let candidate = bindings("control+KeyT", "control+KeyM");
        let registered = Rc::new(RefCell::new(
            current
                .iter()
                .map(|binding| binding.accelerator.clone())
                .collect::<Vec<_>>(),
        ));
        let unregister_state = Rc::clone(&registered);
        let register_state = Rc::clone(&registered);

        let error = replace_bindings(
            &current,
            &candidate,
            move |binding| {
                unregister_state
                    .borrow_mut()
                    .retain(|accelerator| accelerator != &binding.accelerator);
                Ok(())
            },
            move |binding| {
                if binding.command == KoshCommand::MainWindow
                    && binding.accelerator == "control+KeyM"
                {
                    return Err("already registered".into());
                }
                register_state
                    .borrow_mut()
                    .push(binding.accelerator.clone());
                Ok(())
            },
        )
        .expect_err("simulated conflict");

        assert!(error.contains("unavailable"));
        assert_eq!(
            *registered.borrow(),
            vec!["control+KeyK".to_string(), "control+KeyO".to_string()]
        );
    }

    #[test]
    fn unregistration_failure_restores_shortcuts_removed_before_the_failure() {
        let current = bindings("control+KeyK", "control+KeyO");
        let candidate = bindings("control+KeyT", "control+KeyM");
        let registered = Rc::new(RefCell::new(
            current
                .iter()
                .map(|binding| binding.accelerator.clone())
                .collect::<Vec<_>>(),
        ));
        let unregister_state = Rc::clone(&registered);
        let register_state = Rc::clone(&registered);

        let error = replace_bindings(
            &current,
            &candidate,
            move |binding| {
                if binding.command == KoshCommand::MainWindow {
                    return Err("unregister failed".into());
                }
                unregister_state
                    .borrow_mut()
                    .retain(|accelerator| accelerator != &binding.accelerator);
                Ok(())
            },
            move |binding| {
                register_state
                    .borrow_mut()
                    .push(binding.accelerator.clone());
                Ok(())
            },
        )
        .expect_err("simulated unregister failure");

        assert!(error.contains("could not replace"));
        assert_eq!(
            *registered.borrow(),
            vec!["control+KeyO".to_string(), "control+KeyK".to_string()]
        );
    }
}
