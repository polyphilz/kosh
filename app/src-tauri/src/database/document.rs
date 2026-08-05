use std::collections::HashSet;

use serde_json::Value;

use super::{DatabaseError, Result};

const DOCUMENT_SCHEMA_VERSION: i64 = 1;
const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_BLOCKS: usize = 100_000;
const MAX_BLOCK_ID_BYTES: usize = 256;
const MAX_BLOCK_DEPTH: usize = 128;
const SUPPORTED_BLOCK_TYPES: &[&str] = &[
    "paragraph",
    "heading",
    "bulletListItem",
    "numberedListItem",
    "codeBlock",
    "displayMath",
    "koshImage",
    "koshPdf",
    "koshFileAttachment",
];

pub(super) fn validate(document_json: &str) -> Result<()> {
    if document_json.len() > MAX_DOCUMENT_BYTES {
        return invalid("documentJson exceeds the 16 MiB limit");
    }
    let value: Value = serde_json::from_str(document_json)
        .map_err(|_| DatabaseError::InvalidInput("documentJson must be valid JSON".into()))?;
    let object = value.as_object().ok_or_else(|| {
        DatabaseError::InvalidInput("documentJson must contain a JSON object".into())
    })?;
    if object.get("schemaVersion").and_then(Value::as_i64) != Some(DOCUMENT_SCHEMA_VERSION) {
        return invalid("documentJson schemaVersion must be 1");
    }
    let blocks = object
        .get("blocks")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            DatabaseError::InvalidInput("documentJson blocks must be an array".into())
        })?;
    if blocks.is_empty() {
        return invalid("documentJson blocks must not be empty");
    }
    let mut ids = HashSet::new();
    let mut count = 0_usize;
    validate_blocks(blocks, 0, &mut count, &mut ids)
}

/// Creates a valid one-block document for native fixtures that do not exercise the editor.
/// Product-authored writes always provide the exact BlockNote document over IPC.
pub(crate) fn single_paragraph(text: &str) -> String {
    serde_json::json!({
        "schemaVersion": DOCUMENT_SCHEMA_VERSION,
        "blocks": [{
            "id": "native-fixture-block",
            "type": "paragraph",
            "props": {},
            "content": if text.is_empty() {
                Value::Array(Vec::new())
            } else {
                serde_json::json!([{"type": "text", "text": text, "styles": {}}])
            },
            "children": [],
        }],
    })
    .to_string()
}

fn validate_blocks(
    blocks: &[Value],
    depth: usize,
    count: &mut usize,
    ids: &mut HashSet<String>,
) -> Result<()> {
    if depth > MAX_BLOCK_DEPTH {
        return invalid("documentJson block nesting exceeds 128 levels");
    }
    for block in blocks {
        *count = count.checked_add(1).ok_or_else(|| {
            DatabaseError::InvalidInput("documentJson has too many blocks".into())
        })?;
        if *count > MAX_BLOCKS {
            return invalid("documentJson exceeds 100000 blocks");
        }
        let object = block.as_object().ok_or_else(|| {
            DatabaseError::InvalidInput("every documentJson block must be an object".into())
        })?;
        let id = object.get("id").and_then(Value::as_str).ok_or_else(|| {
            DatabaseError::InvalidInput("every documentJson block must have an id".into())
        })?;
        if id.is_empty() || id.len() > MAX_BLOCK_ID_BYTES {
            return invalid("documentJson block ids must contain 1 to 256 bytes");
        }
        if !ids.insert(id.to_owned()) {
            return invalid("documentJson block ids must be unique");
        }
        let block_type = object.get("type").and_then(Value::as_str).ok_or_else(|| {
            DatabaseError::InvalidInput("every documentJson block must have a type".into())
        })?;
        if !SUPPORTED_BLOCK_TYPES.contains(&block_type) {
            return invalid("documentJson contains an unsupported block type");
        }
        if let Some(children) = object.get("children") {
            let children = children.as_array().ok_or_else(|| {
                DatabaseError::InvalidInput("documentJson block children must be an array".into())
            })?;
            validate_blocks(children, depth + 1, count, ids)?;
        }
    }
    Ok(())
}

fn invalid<T>(message: &str) -> Result<T> {
    Err(DatabaseError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::validate;

    #[test]
    fn accepts_versioned_documents_with_nested_unique_ids() {
        validate(
            r#"{"schemaVersion":1,"blocks":[{"id":"a","type":"paragraph","children":[{"id":"b","type":"paragraph"}]}]}"#,
        )
        .unwrap();
    }

    #[test]
    fn rejects_duplicate_ids_across_the_document() {
        let error = validate(
            r#"{"schemaVersion":1,"blocks":[{"id":"same","type":"paragraph","children":[{"id":"same","type":"paragraph"}]}]}"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("must be unique"));
    }

    #[test]
    fn rejects_blocks_outside_the_kosh_editor_schema() {
        let error =
            validate(r#"{"schemaVersion":1,"blocks":[{"id":"a","type":"table","children":[]}]}"#)
                .unwrap_err();
        assert!(error.to_string().contains("unsupported block type"));
    }

    #[test]
    fn rejects_transient_pending_media_blocks() {
        let error = validate(
            r#"{"schemaVersion":1,"blocks":[{"id":"pending","type":"koshPendingMedia","props":{"label":"Adding","requestId":"request"}}]}"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unsupported block type"));
    }
}
