use super::{
    Database, DatabaseError, DatabasePaths, KeyboardBinding, KoshCommand,
    SetAutomaticUpdateChecksInput, SetShortcutSettingsInput, DEFAULT_MAIN_WINDOW_ACCELERATOR,
};

fn bindings(main_window: &str) -> Vec<KeyboardBinding> {
    vec![KeyboardBinding {
        command: KoshCommand::MainWindow,
        accelerator: main_window.into(),
    }]
}

#[test]
fn shortcut_settings_round_trip_and_survive_restart() {
    let root = tempfile::tempdir().expect("temporary library");
    let paths = DatabasePaths::new(root.path());
    let database = Database::initialize(paths.clone()).expect("test database");
    let initial = database
        .client()
        .load_shortcut_settings()
        .expect("initial shortcuts");
    assert_eq!(initial.revision, 1);
    assert!(initial.automatic_update_checks_enabled);
    assert_eq!(
        initial.keyboard_bindings,
        bindings(DEFAULT_MAIN_WINDOW_ACCELERATOR)
    );

    let updated = database
        .client()
        .set_shortcut_settings(SetShortcutSettingsInput {
            expected_revision: initial.revision,
            keyboard_bindings: bindings("control+alt+KeyM"),
        })
        .expect("updated shortcuts");
    assert_eq!(updated.revision, 2);
    database.shutdown().expect("clean shutdown");

    let reopened = Database::initialize(paths).expect("reopened database");
    assert_eq!(
        reopened
            .client()
            .load_shortcut_settings()
            .expect("persisted shortcuts"),
        updated
    );
}

#[test]
fn automatic_update_checks_are_enabled_by_default_and_persist() {
    let root = tempfile::tempdir().expect("temporary library");
    let paths = DatabasePaths::new(root.path());
    let database = Database::initialize(paths.clone()).expect("test database");
    let initial = database
        .client()
        .load_shortcut_settings()
        .expect("initial settings");

    let updated = database
        .client()
        .set_automatic_update_checks(SetAutomaticUpdateChecksInput {
            expected_revision: initial.revision,
            enabled: false,
        })
        .expect("disable automatic update checks");
    assert_eq!(updated.revision, initial.revision + 1);
    assert!(!updated.automatic_update_checks_enabled);
    database.shutdown().expect("clean shutdown");

    let reopened = Database::initialize(paths).expect("reopened database");
    assert_eq!(
        reopened
            .client()
            .load_shortcut_settings()
            .expect("persisted settings"),
        updated
    );
}

#[test]
fn shortcut_settings_reject_stale_updates() {
    let root = tempfile::tempdir().expect("temporary library");
    let database = Database::initialize(DatabasePaths::new(root.path())).expect("test database");
    let client = database.client();

    client
        .set_shortcut_settings(SetShortcutSettingsInput {
            expected_revision: 1,
            keyboard_bindings: bindings("control+alt+KeyM"),
        })
        .expect("first update");
    let stale = client
        .set_shortcut_settings(SetShortcutSettingsInput {
            expected_revision: 1,
            keyboard_bindings: bindings("control+alt+KeyU"),
        })
        .expect_err("stale update");
    assert!(matches!(stale, DatabaseError::InvalidInput(_)));
}

#[test]
fn shortcut_settings_require_complete_modified_bindings() {
    let root = tempfile::tempdir().expect("temporary library");
    let database = Database::initialize(DatabasePaths::new(root.path())).expect("test database");
    let client = database.client();

    for keyboard_bindings in [vec![], bindings("KeyK"), bindings("control+CapsLock")] {
        assert!(matches!(
            client
                .set_shortcut_settings(SetShortcutSettingsInput {
                    expected_revision: 1,
                    keyboard_bindings,
                })
                .expect_err("invalid shortcut settings"),
            DatabaseError::InvalidInput(_)
        ));
    }
}
