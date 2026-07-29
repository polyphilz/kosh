use super::{
    Database, DatabaseError, DatabasePaths, KeyboardBinding, KoshCommand, SetShortcutSettingsInput,
    DEFAULT_MAIN_WINDOW_ACCELERATOR, DEFAULT_QUICK_ADD_ACCELERATOR,
};

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
fn shortcut_settings_round_trip_and_survive_restart() {
    let root = tempfile::tempdir().expect("temporary library");
    let paths = DatabasePaths::new(root.path());
    let database = Database::initialize(paths.clone()).expect("test database");
    let initial = database
        .client()
        .load_shortcut_settings()
        .expect("initial shortcuts");
    assert_eq!(initial.revision, 1);
    assert_eq!(
        initial.keyboard_bindings,
        bindings(
            DEFAULT_QUICK_ADD_ACCELERATOR,
            DEFAULT_MAIN_WINDOW_ACCELERATOR
        )
    );

    let updated = database
        .client()
        .set_shortcut_settings(SetShortcutSettingsInput {
            expected_revision: initial.revision,
            keyboard_bindings: bindings("control+alt+KeyT", "control+alt+KeyM"),
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
fn shortcut_settings_reject_conflicts_and_stale_updates() {
    let root = tempfile::tempdir().expect("temporary library");
    let database = Database::initialize(DatabasePaths::new(root.path())).expect("test database");
    let client = database.client();

    let duplicate = client
        .set_shortcut_settings(SetShortcutSettingsInput {
            expected_revision: 1,
            keyboard_bindings: bindings("control+alt+KeyK", "control+alt+KeyK"),
        })
        .expect_err("duplicate shortcut");
    assert!(matches!(duplicate, DatabaseError::InvalidInput(_)));

    client
        .set_shortcut_settings(SetShortcutSettingsInput {
            expected_revision: 1,
            keyboard_bindings: bindings("control+alt+KeyT", "control+alt+KeyM"),
        })
        .expect("first update");
    let stale = client
        .set_shortcut_settings(SetShortcutSettingsInput {
            expected_revision: 1,
            keyboard_bindings: bindings("control+alt+KeyY", "control+alt+KeyU"),
        })
        .expect_err("stale update");
    assert!(matches!(stale, DatabaseError::InvalidInput(_)));
}

#[test]
fn shortcut_settings_require_complete_modified_bindings() {
    let root = tempfile::tempdir().expect("temporary library");
    let database = Database::initialize(DatabasePaths::new(root.path())).expect("test database");
    let client = database.client();

    for keyboard_bindings in [
        vec![KeyboardBinding {
            command: KoshCommand::QuickAdd,
            accelerator: "control+KeyK".into(),
        }],
        bindings("KeyK", "control+KeyM"),
        bindings("control+CapsLock", "control+KeyM"),
    ] {
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
