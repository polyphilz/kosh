use std::collections::HashSet;

use serde_json::Value;

use super::{tidbits, DatabaseError, Result};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AttachmentBlockKind {
    Image,
    Pdf,
    File,
}

impl AttachmentBlockKind {
    pub(super) fn accepts_database_kind(self, database_kind: &str) -> bool {
        match self {
            Self::Image => database_kind == "IMAGE",
            Self::Pdf => database_kind == "PDF",
            Self::File => matches!(database_kind, "TEXT" | "BINARY"),
        }
    }

    pub(super) fn display_role(self) -> &'static str {
        match self {
            Self::Image => "INLINE",
            Self::Pdf | Self::File => "ATTACHMENT",
        }
    }

    pub(super) fn block_type(self) -> &'static str {
        match self {
            Self::Image => "koshImage",
            Self::Pdf => "koshPdf",
            Self::File => "koshFileAttachment",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DocumentAttachment {
    pub(super) attachment_id: String,
    pub(super) block_id: String,
    pub(super) kind: AttachmentBlockKind,
}

pub(super) fn validate(document_json: &str) -> Result<()> {
    extract_attachments(document_json).map(|_| ())
}

pub(super) fn extract_attachments(document_json: &str) -> Result<Vec<DocumentAttachment>> {
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
    let mut attachment_ids = HashSet::new();
    let mut attachments = Vec::new();
    let mut count = 0_usize;
    validate_blocks(
        blocks,
        0,
        &mut count,
        &mut ids,
        &mut attachment_ids,
        &mut attachments,
    )?;
    Ok(attachments)
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

/// Builds a canonical document for native fixtures that still express media through the
/// temporary Markdown projection. Product-authored writes never use this bridge.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn fixture_from_markdown(markdown: &str) -> String {
    let mut blocks = vec![serde_json::json!({
        "id": "native-fixture-block",
        "type": "paragraph",
        "props": {},
        "content": if markdown.is_empty() {
            Value::Array(Vec::new())
        } else {
            serde_json::json!([{"type": "text", "text": markdown, "styles": {}}])
        },
        "children": [],
    })];
    blocks.extend(
        super::media::referenced_attachments(markdown)
            .into_iter()
            .enumerate()
            .map(|(index, reference)| {
                serde_json::json!({
                    "id": format!("native-fixture-media-{index}"),
                    "type": reference.kind.block_type(),
                    "props": {"attachmentId": reference.id},
                    "content": [],
                    "children": [],
                })
            }),
    );
    serde_json::json!({
        "schemaVersion": DOCUMENT_SCHEMA_VERSION,
        "blocks": blocks,
    })
    .to_string()
}

fn validate_blocks(
    blocks: &[Value],
    depth: usize,
    count: &mut usize,
    ids: &mut HashSet<String>,
    attachment_ids: &mut HashSet<String>,
    attachments: &mut Vec<DocumentAttachment>,
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
        if let Some(kind) = attachment_kind(block_type) {
            let attachment_id = object
                .get("props")
                .and_then(Value::as_object)
                .and_then(|props| props.get("attachmentId"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    DatabaseError::InvalidInput(
                        "media blocks must have a props.attachmentId".into(),
                    )
                })?;
            tidbits::validate_uuid_v7(attachment_id, "documentJson attachmentId")?;
            if !attachment_ids.insert(attachment_id.to_owned()) {
                return invalid("an attachment may belong to only one document block");
            }
            attachments.push(DocumentAttachment {
                attachment_id: attachment_id.to_owned(),
                block_id: id.to_owned(),
                kind,
            });
        }
        if let Some(children) = object.get("children") {
            let children = children.as_array().ok_or_else(|| {
                DatabaseError::InvalidInput("documentJson block children must be an array".into())
            })?;
            validate_blocks(children, depth + 1, count, ids, attachment_ids, attachments)?;
        }
    }
    Ok(())
}

fn attachment_kind(block_type: &str) -> Option<AttachmentBlockKind> {
    [
        AttachmentBlockKind::Image,
        AttachmentBlockKind::Pdf,
        AttachmentBlockKind::File,
    ]
    .into_iter()
    .find(|kind| kind.block_type() == block_type)
}

fn invalid<T>(message: &str) -> Result<T> {
    Err(DatabaseError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::{extract_attachments, validate, AttachmentBlockKind};

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

    #[test]
    fn extracts_one_stable_owner_for_each_attachment() {
        let attachments = extract_attachments(
            r#"{"schemaVersion":1,"blocks":[{"id":"image-block","type":"koshImage","props":{"attachmentId":"019f547b-6200-7000-8000-000000002001"},"children":[]},{"id":"file-block","type":"koshFileAttachment","props":{"attachmentId":"019f547b-6200-7000-8000-000000002002"},"children":[]}]}"#,
        )
        .unwrap();
        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments[0].block_id, "image-block");
        assert_eq!(attachments[0].kind, AttachmentBlockKind::Image);
        assert_eq!(attachments[1].block_id, "file-block");
        assert_eq!(attachments[1].kind, AttachmentBlockKind::File);
    }

    #[test]
    fn rejects_one_attachment_reused_by_multiple_blocks() {
        let error = validate(
            r#"{"schemaVersion":1,"blocks":[{"id":"a","type":"koshImage","props":{"attachmentId":"019f547b-6200-7000-8000-000000002001"}},{"id":"b","type":"koshImage","props":{"attachmentId":"019f547b-6200-7000-8000-000000002001"}}]}"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("only one document block"));
    }
}
