#![cfg_attr(feature = "test-support", allow(dead_code))]

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{mpsc, Mutex},
    time::Duration,
};

use objc2::MainThreadMarker as ObjcMainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSRunningApplication, NSWindow,
    NSWindowCollectionBehavior, NSWorkspace,
};
use serde::Serialize;
use tauri::{
    image::Image,
    menu::{Menu, MenuBuilder, MenuItemBuilder, MenuItemKind, PredefinedMenuItem},
    tray::TrayIconBuilder,
    utils::config::BackgroundThrottlingPolicy,
    App, AppHandle, Emitter, Manager, PhysicalPosition, State, WebviewUrl, WebviewWindowBuilder,
    WindowEvent,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::{
    database::{
        validate_complete_bindings, KeyboardBinding, KoshCommand, SetAutomaticUpdateChecksInput,
        SetShortcutSettingsInput, ShortcutSettings, DEFAULT_MAIN_WINDOW_ACCELERATOR,
        DEFAULT_QUICK_ADD_ACCELERATOR,
    },
    runtime::RuntimeState,
};

const MAIN_LABEL: &str = "main";
const QUICK_ADD_LABEL: &str = "quick-add";
const TRAY_ID: &str = "kosh-tray";
#[cfg(test)]
const APP_ICON_BYTES: &[u8] = include_bytes!("../icons/icon.png");
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray-icon.png");
const QUICK_ADD_SHOWN_EVENT: &str = "kosh://quick-add-shown";
const OPEN_SETTINGS_EVENT: &str = "kosh://open-settings";
const CHECK_FOR_UPDATES_EVENT: &str = "kosh://check-for-updates";
const SHORTCUT_SETTINGS_CHANGED_EVENT: &str = "kosh://shortcut-settings-changed";
const PREPARE_QUIT_EVENT: &str = "kosh://prepare-quit";
const QUIT_CANCELED_EVENT: &str = "kosh://quit-canceled";
const QUIT_ACK_TIMEOUT: Duration = Duration::from_secs(5);
const SETTINGS_MENU_ID: &str = "open-settings";
const CHECK_FOR_UPDATES_MENU_ID: &str = "check-for-updates";
const CHECK_FOR_UPDATES_TRAY_MENU_ID: &str = "check-for-updates-tray";

#[derive(Clone, Copy)]
enum TrayAction {
    ShowMain,
    ShowSettings,
    CheckForUpdates,
    ShowQuickAdd,
    Quit,
}

impl TrayAction {
    const fn id(self) -> &'static str {
        match self {
            Self::ShowMain => "show-main",
            Self::ShowSettings => "show-settings",
            Self::CheckForUpdates => CHECK_FOR_UPDATES_TRAY_MENU_ID,
            Self::ShowQuickAdd => "show-quick-add",
            Self::Quit => "quit",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        [
            Self::ShowMain,
            Self::ShowSettings,
            Self::CheckForUpdates,
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
    quit: Mutex<QuitContext>,
    relaunch_preparations: Mutex<BTreeMap<u64, mpsc::SyncSender<Result<(), String>>>>,
    shortcut_errors: Mutex<Vec<String>>,
}

#[derive(Default)]
struct QuitContext {
    next_request_id: u64,
    pending: Option<QuitAttempt>,
}

struct QuitAttempt {
    awaiting: BTreeSet<String>,
    request_id: u64,
}

#[derive(Debug, PartialEq)]
enum QuitAcknowledgement {
    Canceled { error: String, window_label: String },
    Exit,
    Ignored,
    Waiting,
}

impl QuitContext {
    fn begin(
        &mut self,
        window_labels: impl IntoIterator<Item = String>,
    ) -> Option<(u64, Vec<String>)> {
        if self.pending.is_some() {
            return None;
        }
        let awaiting = window_labels.into_iter().collect::<BTreeSet<_>>();
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        let request_id = self.next_request_id;
        let labels = awaiting.iter().cloned().collect();
        self.pending = Some(QuitAttempt {
            awaiting,
            request_id,
        });
        Some((request_id, labels))
    }

    fn acknowledge(
        &mut self,
        request_id: u64,
        window_label: &str,
        error: Option<String>,
    ) -> QuitAcknowledgement {
        let Some(attempt) = self
            .pending
            .as_mut()
            .filter(|attempt| attempt.request_id == request_id)
        else {
            return QuitAcknowledgement::Ignored;
        };
        if !attempt.awaiting.remove(window_label) {
            return QuitAcknowledgement::Ignored;
        }
        if let Some(error) = error {
            self.pending = None;
            return QuitAcknowledgement::Canceled {
                error,
                window_label: window_label.to_owned(),
            };
        }
        if attempt.awaiting.is_empty() {
            self.pending = None;
            QuitAcknowledgement::Exit
        } else {
            QuitAcknowledgement::Waiting
        }
    }

    fn cancel(&mut self, request_id: u64) -> bool {
        if self
            .pending
            .as_ref()
            .is_some_and(|attempt| attempt.request_id == request_id)
        {
            self.pending = None;
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct QuitNotice {
    request_id: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShortcutSettingsSnapshot {
    #[serde(flatten)]
    settings: ShortcutSettings,
    shortcut_errors: Vec<String>,
}

pub(crate) fn setup(
    app: &mut App,
    settings: ShortcutSettings,
    show_main_on_launch: bool,
) -> tauri::Result<()> {
    // Kosh stays a Regular application for its whole lifetime. macOS binds menu-bar ownership
    // during activation from the policy the process already holds, so switching from Accessory
    // after a tray action can leave the previously active application named in the menu bar.
    // Remaining Regular costs a Dock icon and keeps activation and menu-bar ownership stable.
    app.set_activation_policy(tauri::ActivationPolicy::Regular);
    app.manage(WindowState::default());
    install_application_menu(app)?;
    create_quick_add_window(app.handle())?;
    install_tray(app, &settings.keyboard_bindings)?;
    let errors = register_shortcuts(app.handle(), &settings.keyboard_bindings);
    *app.state::<WindowState>()
        .shortcut_errors
        .lock()
        .expect("shortcut errors poisoned") = errors;
    if show_main_on_launch {
        if let Err(error) = activate_main_window(app.handle()) {
            log::error!("failed to show the main window on launch: {error}");
        }
    }
    Ok(())
}

fn install_application_menu(app: &mut App) -> tauri::Result<()> {
    let menu = Menu::default(app.handle())?;
    if let Some(application_menu) = menu.items()?.into_iter().find_map(|item| match item {
        MenuItemKind::Submenu(submenu) => Some(submenu),
        _ => None,
    }) {
        let check_for_updates =
            MenuItemBuilder::with_id(CHECK_FOR_UPDATES_MENU_ID, "Check for Updates…").build(app)?;
        let settings = MenuItemBuilder::with_id(SETTINGS_MENU_ID, "Settings…")
            .accelerator("CmdOrCtrl+,")
            .build(app)?;
        let separator = PredefinedMenuItem::separator(app)?;
        application_menu.append_items(&[&check_for_updates, &separator, &settings])?;
    }
    app.on_menu_event(|app, event| match event.id().as_ref() {
        SETTINGS_MENU_ID => dispatch_logged(app, "show settings", show_settings_inner),
        CHECK_FOR_UPDATES_MENU_ID => {
            dispatch_logged(app, "check for updates", check_for_updates_inner)
        }
        _ => {}
    });
    app.set_menu(menu)?;
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
        .text(TrayAction::CheckForUpdates.id(), "Check for Updates…")
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
    TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("Kosh")
        .icon(load_tray_icon()?)
        .icon_as_template(true)
        .on_menu_event(
            |app, event| match TrayAction::from_id(event.id().as_ref()) {
                Some(TrayAction::ShowMain) => {
                    dispatch_logged(app, "show main window", show_main_inner)
                }
                Some(TrayAction::ShowSettings) => {
                    dispatch_logged(app, "show settings", show_settings_inner)
                }
                Some(TrayAction::CheckForUpdates) => {
                    dispatch_logged(app, "check for updates", check_for_updates_inner)
                }
                Some(TrayAction::ShowQuickAdd) => {
                    dispatch_logged(app, "show quick add", show_quick_add_inner)
                }
                Some(TrayAction::Quit) => request_quit(app),
                None => {}
            },
        )
        .build(app)?;
    Ok(())
}

fn load_tray_icon() -> tauri::Result<Image<'static>> {
    let icon = image::load_from_memory(TRAY_ICON_BYTES)
        .map_err(|error| tauri::Error::InvalidIcon(std::io::Error::other(error)))?
        .into_rgba8();
    let width = icon.width();
    let height = icon.height();
    Ok(Image::new_owned(icon.into_raw(), width, height))
}

pub(crate) fn request_quit(app: &AppHandle) {
    let labels = [MAIN_LABEL, QUICK_ADD_LABEL]
        .into_iter()
        .filter(|label| app.get_webview_window(label).is_some())
        .map(str::to_owned);
    let Some((request_id, labels)) = app
        .state::<WindowState>()
        .quit
        .lock()
        .expect("quit context poisoned")
        .begin(labels)
    else {
        return;
    };
    if labels.is_empty() {
        app.exit(0);
        return;
    }

    let notice = QuitNotice { request_id };
    for label in &labels {
        if let Err(error) = app.emit_to(label, PREPARE_QUIT_EVENT, notice) {
            app.state::<WindowState>()
                .quit
                .lock()
                .expect("quit context poisoned")
                .cancel(request_id);
            cancel_quit_ui(
                app,
                request_id,
                label,
                &format!("Could not ask {label} to preserve its draft: {error}"),
            );
            return;
        }
    }

    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(QUIT_ACK_TIMEOUT);
        let timed_out = app
            .state::<WindowState>()
            .quit
            .lock()
            .expect("quit context poisoned")
            .cancel(request_id);
        if timed_out {
            cancel_quit_ui(
                &app,
                request_id,
                MAIN_LABEL,
                "Kosh did not finish preserving open drafts before the quit timeout",
            );
        }
    });
}

pub(crate) fn handle_exit_requested(
    app: &AppHandle,
    code: Option<i32>,
    api: &tauri::ExitRequestApi,
) {
    if !should_prepare_for_exit(code) {
        return;
    }
    api.prevent_exit();
    request_quit(app);
}

const fn should_prepare_for_exit(code: Option<i32>) -> bool {
    code.is_none()
}

#[tauri::command]
pub(crate) fn acknowledge_quit(
    window: tauri::Window,
    state: State<'_, WindowState>,
    request_id: u64,
    error: Option<String>,
) {
    let error = error.map(|error| error.chars().take(512).collect());
    let acknowledgement = state
        .quit
        .lock()
        .expect("quit context poisoned")
        .acknowledge(request_id, window.label(), error);
    match acknowledgement {
        QuitAcknowledgement::Exit => {
            let preparation = state
                .relaunch_preparations
                .lock()
                .expect("relaunch preparations poisoned")
                .remove(&request_id);
            if let Some(completion) = preparation {
                let _ = completion.send(Ok(()));
            } else {
                dispatch_logged(window.app_handle(), "quit Kosh", |app| {
                    app.exit(0);
                    Ok(())
                });
            }
        }
        QuitAcknowledgement::Canceled {
            error,
            window_label,
        } => {
            if let Some(completion) = state
                .relaunch_preparations
                .lock()
                .expect("relaunch preparations poisoned")
                .remove(&request_id)
            {
                let _ = completion.send(Err(error.clone()));
            }
            cancel_quit_ui(window.app_handle(), request_id, &window_label, &error);
        }
        QuitAcknowledgement::Ignored | QuitAcknowledgement::Waiting => {}
    }
}

#[tauri::command]
pub(crate) async fn prepare_update_relaunch(app: AppHandle) -> Result<(), String> {
    let labels = [MAIN_LABEL, QUICK_ADD_LABEL]
        .into_iter()
        .filter(|label| app.get_webview_window(label).is_some())
        .map(str::to_owned);
    let Some((request_id, labels)) = app
        .state::<WindowState>()
        .quit
        .lock()
        .expect("quit context poisoned")
        .begin(labels)
    else {
        return Err("Kosh is already preparing to close".into());
    };
    if labels.is_empty() {
        app.state::<WindowState>()
            .quit
            .lock()
            .expect("quit context poisoned")
            .cancel(request_id);
        return Ok(());
    }

    let (completion, receiver) = mpsc::sync_channel(1);
    app.state::<WindowState>()
        .relaunch_preparations
        .lock()
        .expect("relaunch preparations poisoned")
        .insert(request_id, completion);
    let notice = QuitNotice { request_id };
    for label in &labels {
        if let Err(error) = app.emit_to(label, PREPARE_QUIT_EVENT, notice) {
            app.state::<WindowState>()
                .quit
                .lock()
                .expect("quit context poisoned")
                .cancel(request_id);
            app.state::<WindowState>()
                .relaunch_preparations
                .lock()
                .expect("relaunch preparations poisoned")
                .remove(&request_id);
            let message = format!("Could not ask {label} to preserve its draft: {error}");
            cancel_quit_ui(&app, request_id, label, &message);
            return Err(message);
        }
    }

    let wait =
        tauri::async_runtime::spawn_blocking(move || receiver.recv_timeout(QUIT_ACK_TIMEOUT))
            .await
            .map_err(|error| error.to_string())?;
    match wait {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            app.state::<WindowState>()
                .quit
                .lock()
                .expect("quit context poisoned")
                .cancel(request_id);
            app.state::<WindowState>()
                .relaunch_preparations
                .lock()
                .expect("relaunch preparations poisoned")
                .remove(&request_id);
            let message = "Kosh did not finish preserving open drafts before the update timeout";
            cancel_quit_ui(&app, request_id, MAIN_LABEL, message);
            Err(message.into())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("Kosh could not confirm that open drafts were preserved".into())
        }
    }
}

fn cancel_quit_ui(app: &AppHandle, request_id: u64, window_label: &str, error: &str) {
    log::error!("quit canceled while preserving {window_label}: {error}");
    let notice = QuitNotice { request_id };
    for label in [MAIN_LABEL, QUICK_ADD_LABEL] {
        if app.get_webview_window(label).is_some() {
            if let Err(emit_error) = app.emit_to(label, QUIT_CANCELED_EVENT, notice) {
                log::error!("could not release {label} after canceled quit: {emit_error}");
            }
        }
    }
    if window_label == QUICK_ADD_LABEL {
        dispatch_logged(
            app,
            "show quick-add draft after canceled quit",
            show_quick_add_inner,
        );
    } else {
        dispatch_logged(app, "show main draft after canceled quit", show_main_inner);
    }
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
pub(crate) async fn set_automatic_update_checks(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    input: SetAutomaticUpdateChecksInput,
) -> Result<ShortcutSettingsSnapshot, String> {
    let client = state.database_client();
    let persisted =
        tauri::async_runtime::spawn_blocking(move || client.set_automatic_update_checks(input))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
    let snapshot = snapshot(&app, persisted);
    if let Err(error) = app.emit_to(MAIN_LABEL, SHORTCUT_SETTINGS_CHANGED_EVENT, &snapshot) {
        log::error!("could not publish update settings: {error}");
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
        enter_resident_mode();
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

fn check_for_updates_inner(app: &AppHandle) -> Result<(), String> {
    show_main_inner(app)?;
    app.emit_to(MAIN_LABEL, CHECK_FOR_UPDATES_EVENT, ())
        .map_err(|error| format!("could not request an update check: {error}"))
}

fn activate_main_window(app: &AppHandle) -> Result<(), String> {
    let marker = ObjcMainThreadMarker::new()
        .ok_or_else(|| "main-window activation was not on the main thread".to_string())?;
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

/// Steps out of the foreground while leaving Kosh running behind its menu-bar icon. The
/// activation policy stays Regular so the next activation still owns the menu bar.
fn enter_resident_mode() {
    if let Some(marker) = ObjcMainThreadMarker::new() {
        NSApplication::sharedApplication(marker).deactivate();
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
                enter_resident_mode();
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

    fn pixel_bounds(
        icon: &image::RgbaImage,
        predicate: impl Fn(&image::Rgba<u8>) -> bool,
    ) -> Option<(u32, u32, u32, u32)> {
        icon.enumerate_pixels()
            .filter(|(_, _, pixel)| predicate(pixel))
            .fold(None, |bounds, (x, y, _)| {
                Some(match bounds {
                    None => (x, y, x, y),
                    Some((left, top, right, bottom)) => {
                        (left.min(x), top.min(y), right.max(x), bottom.max(y))
                    }
                })
            })
    }

    #[test]
    fn app_icon_uses_the_centered_macos_safe_geometry() {
        let icon = image::load_from_memory(APP_ICON_BYTES)
            .expect("app icon should decode")
            .into_rgba8();
        assert_eq!((icon.width(), icon.height()), (512, 512));

        let occupied = pixel_bounds(&icon, |pixel| pixel[3] > 0)
            .expect("app icon should contain visible pixels");
        assert_eq!(occupied, (50, 50, 461, 461));

        let artwork = pixel_bounds(&icon, |pixel| {
            pixel[3] > 0 && pixel.0[..3].iter().any(|channel| *channel > 80)
        })
        .expect("app icon should contain foreground artwork");
        assert_eq!(artwork, (91, 88, 421, 422));

        assert_eq!(icon.get_pixel(0, 0)[3], 0);
        assert_eq!(icon.get_pixel(511, 511)[3], 0);
        assert_eq!(icon.get_pixel(100, 50)[3], 0);
        assert!(icon.get_pixel(140, 50)[3] > 0);
        assert_eq!(icon.get_pixel(256, 256)[3], 255);
    }

    #[test]
    fn tray_icon_is_a_transparent_monochrome_template() {
        let icon = load_tray_icon().expect("tray icon should decode");
        assert_eq!((icon.width(), icon.height()), (32, 32));
        assert!(icon.rgba().chunks_exact(4).any(|pixel| pixel[3] == 0));
        assert!(icon.rgba().chunks_exact(4).any(|pixel| pixel[3] == 255));
        assert!(icon
            .rgba()
            .chunks_exact(4)
            .filter(|pixel| pixel[3] > 0)
            .all(|pixel| pixel[..3] == [255, 255, 255]));
    }

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
    fn quit_waits_for_every_window_and_ignores_duplicate_acknowledgements() {
        let mut context = QuitContext::default();
        let (request_id, labels) = context
            .begin([MAIN_LABEL.to_owned(), QUICK_ADD_LABEL.to_owned()])
            .expect("first request starts");
        assert_eq!(
            labels,
            vec![MAIN_LABEL.to_owned(), QUICK_ADD_LABEL.to_owned()]
        );
        assert!(context
            .begin([MAIN_LABEL.to_owned(), QUICK_ADD_LABEL.to_owned()])
            .is_none());
        assert_eq!(
            context.acknowledge(request_id, MAIN_LABEL, None),
            QuitAcknowledgement::Waiting
        );
        assert_eq!(
            context.acknowledge(request_id, MAIN_LABEL, None),
            QuitAcknowledgement::Ignored
        );
        assert_eq!(
            context.acknowledge(request_id, QUICK_ADD_LABEL, None),
            QuitAcknowledgement::Exit
        );
    }

    #[test]
    fn failed_or_timed_out_draft_flush_cancels_quit() {
        let mut context = QuitContext::default();
        let (request_id, _) = context
            .begin([MAIN_LABEL.to_owned(), QUICK_ADD_LABEL.to_owned()])
            .expect("request starts");
        assert_eq!(
            context.acknowledge(
                request_id,
                QUICK_ADD_LABEL,
                Some("attachment is still being ingested".to_owned())
            ),
            QuitAcknowledgement::Canceled {
                error: "attachment is still being ingested".to_owned(),
                window_label: QUICK_ADD_LABEL.to_owned(),
            }
        );
        assert_eq!(
            context.acknowledge(request_id, MAIN_LABEL, None),
            QuitAcknowledgement::Ignored
        );

        let (next_request_id, _) = context
            .begin([MAIN_LABEL.to_owned(), QUICK_ADD_LABEL.to_owned()])
            .expect("a canceled request can be retried");
        assert!(context.cancel(next_request_id));
        assert!(!context.cancel(next_request_id));
    }

    #[test]
    fn native_quit_requires_draft_preparation_but_approved_exit_does_not() {
        assert!(should_prepare_for_exit(None));
        assert!(!should_prepare_for_exit(Some(0)));
        assert!(!should_prepare_for_exit(Some(tauri::RESTART_EXIT_CODE)));
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
