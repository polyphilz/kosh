use std::{collections::HashSet, str::FromStr};

use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};

use super::{DatabaseError, Result};

pub const DEFAULT_QUICK_ADD_ACCELERATOR: &str = "control+alt+super+KeyK";
pub const DEFAULT_MAIN_WINDOW_ACCELERATOR: &str = "control+alt+super+KeyO";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum KoshCommand {
    QuickAdd,
    MainWindow,
}

impl KoshCommand {
    pub const ALL: [Self; 2] = [Self::QuickAdd, Self::MainWindow];

    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::QuickAdd => "QUICK_ADD",
            Self::MainWindow => "MAIN_WINDOW",
        }
    }

    fn from_db(value: &str) -> Result<Self> {
        match value {
            "QUICK_ADD" => Ok(Self::QuickAdd),
            "MAIN_WINDOW" => Ok(Self::MainWindow),
            _ => Err(DatabaseError::Validation {
                kind: "main",
                reason: format!("unknown Kosh command {value}"),
            }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyboardBinding {
    pub command: KoshCommand,
    pub accelerator: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutSettings {
    pub revision: i64,
    pub automatic_update_checks_enabled: bool,
    pub keyboard_bindings: Vec<KeyboardBinding>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetShortcutSettingsInput {
    pub expected_revision: i64,
    pub keyboard_bindings: Vec<KeyboardBinding>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetAutomaticUpdateChecksInput {
    pub expected_revision: i64,
    pub enabled: bool,
}

pub(super) fn load_shortcut_settings(connection: &Connection) -> Result<ShortcutSettings> {
    let (revision, automatic_update_checks_enabled) = connection.query_row(
        "SELECT revision, automatic_update_checks_enabled
         FROM shortcut_settings WHERE singleton_id = 1",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?)),
    )?;
    let mut statement = connection.prepare(
        "SELECT command, accelerator
         FROM keyboard_binding
         ORDER BY CASE command
             WHEN 'QUICK_ADD' THEN 0
             WHEN 'MAIN_WINDOW' THEN 1
         END",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut keyboard_bindings = Vec::new();
    for row in rows {
        let (command, accelerator) = row?;
        validate_accelerator(&accelerator)?;
        keyboard_bindings.push(KeyboardBinding {
            command: KoshCommand::from_db(&command)?,
            accelerator,
        });
    }
    validate_complete_bindings(&keyboard_bindings)?;
    Ok(ShortcutSettings {
        revision,
        automatic_update_checks_enabled,
        keyboard_bindings,
    })
}

pub(super) fn set_automatic_update_checks(
    connection: &mut Connection,
    input: SetAutomaticUpdateChecksInput,
) -> Result<ShortcutSettings> {
    if input.expected_revision <= 0 {
        return Err(DatabaseError::InvalidInput(
            "expectedRevision must be positive".into(),
        ));
    }
    let changed = connection.execute(
        "UPDATE shortcut_settings
         SET automatic_update_checks_enabled = ?1, revision = revision + 1
         WHERE singleton_id = 1 AND revision = ?2",
        params![input.enabled, input.expected_revision],
    )?;
    if changed != 1 {
        return Err(DatabaseError::InvalidInput(
            "settings changed before this update".into(),
        ));
    }
    load_shortcut_settings(connection)
}

pub(super) fn set_shortcut_settings(
    connection: &mut Connection,
    input: SetShortcutSettingsInput,
) -> Result<ShortcutSettings> {
    validate_complete_bindings(&input.keyboard_bindings)?;
    if input.expected_revision <= 0 {
        return Err(DatabaseError::InvalidInput(
            "expectedRevision must be positive".into(),
        ));
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let actual_revision = transaction.query_row(
        "SELECT revision FROM shortcut_settings WHERE singleton_id = 1",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if actual_revision != input.expected_revision {
        return Err(DatabaseError::InvalidInput(format!(
            "shortcut settings changed before this update: revision is {actual_revision}, expected {}",
            input.expected_revision
        )));
    }

    for binding in &input.keyboard_bindings {
        let changed = transaction.execute(
            "UPDATE keyboard_binding SET accelerator = ?1 WHERE command = ?2",
            params![binding.accelerator, binding.command.as_db_str()],
        )?;
        if changed != 1 {
            return Err(DatabaseError::Validation {
                kind: "main",
                reason: format!("missing {} keyboard binding", binding.command.as_db_str()),
            });
        }
    }
    let changed = transaction.execute(
        "UPDATE shortcut_settings
         SET revision = revision + 1
         WHERE singleton_id = 1 AND revision = ?1",
        params![input.expected_revision],
    )?;
    if changed != 1 {
        return Err(DatabaseError::InvalidInput(
            "shortcut settings changed before commit".into(),
        ));
    }
    transaction.commit()?;
    load_shortcut_settings(connection)
}

pub fn validate_complete_bindings(bindings: &[KeyboardBinding]) -> Result<()> {
    if bindings.len() != KoshCommand::ALL.len() {
        return Err(DatabaseError::InvalidInput(
            "keyboardBindings must contain every Kosh command exactly once".into(),
        ));
    }
    let commands = bindings
        .iter()
        .map(|binding| binding.command)
        .collect::<HashSet<_>>();
    if commands.len() != KoshCommand::ALL.len()
        || KoshCommand::ALL
            .iter()
            .any(|command| !commands.contains(command))
    {
        return Err(DatabaseError::InvalidInput(
            "keyboardBindings contains duplicate or missing commands".into(),
        ));
    }

    let mut shortcut_ids = HashSet::new();
    for binding in bindings {
        let shortcut = validate_accelerator(&binding.accelerator)?;
        if !shortcut_ids.insert(shortcut.id()) {
            return Err(DatabaseError::InvalidInput(
                "two Kosh commands cannot use the same shortcut".into(),
            ));
        }
    }
    Ok(())
}

pub fn validate_accelerator(accelerator: &str) -> Result<Shortcut> {
    let shortcut = Shortcut::from_str(accelerator).map_err(|error| {
        DatabaseError::InvalidInput(format!("invalid global shortcut: {error}"))
    })?;
    if shortcut
        .mods
        .intersection(Modifiers::SHIFT | Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER)
        == Modifiers::empty()
    {
        return Err(DatabaseError::InvalidInput(
            "global shortcuts must include at least one modifier".into(),
        ));
    }
    if !supported_shortcut_key(shortcut.key) {
        return Err(DatabaseError::InvalidInput(
            "that key is not supported for Kosh global shortcuts".into(),
        ));
    }
    Ok(shortcut)
}

fn supported_shortcut_key(key: Code) -> bool {
    !matches!(
        key,
        Code::CapsLock
            | Code::NumLock
            | Code::Pause
            | Code::PrintScreen
            | Code::ScrollLock
            | Code::AudioVolumeDown
            | Code::AudioVolumeMute
            | Code::AudioVolumeUp
            | Code::MediaPause
            | Code::MediaPlay
            | Code::MediaPlayPause
            | Code::MediaStop
            | Code::MediaTrackNext
            | Code::MediaTrackPrevious
    )
}
