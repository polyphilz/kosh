use std::collections::HashSet;

use serde_json::Value;

use super::{tidbits, DatabaseError, Result};

const DOCUMENT_SCHEMA_VERSION: i64 = 1;
const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_BLOCKS: usize = 100_000;
const MAX_BLOCK_ID_BYTES: usize = 256;
const MAX_BLOCK_DEPTH: usize = 128;
// Heading context is a derived search hint, not authored content. Bounding each active
// level prevents a large heading from being cloned into every following block document.
const MAX_HEADING_CONTEXT_BYTES_PER_LEVEL: usize = 256;
const SUPPORTED_BLOCK_TYPES: &[&str] = &[
    "paragraph",
    "heading",
    "bulletListItem",
    "numberedListItem",
    "codeBlock",
    "displayMath",
    "koshImage",
    "koshFileAttachment",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AttachmentBlockKind {
    Image,
    File,
}

impl AttachmentBlockKind {
    pub(super) fn accepts_database_kind(self, database_kind: &str) -> bool {
        match self {
            Self::Image => database_kind == "IMAGE",
            Self::File => database_kind == "FILE",
        }
    }

    pub(super) fn display_role(self) -> &'static str {
        match self {
            Self::Image => "INLINE",
            Self::File => "ATTACHMENT",
        }
    }

    pub(super) fn block_type(self) -> &'static str {
        match self {
            Self::Image => "koshImage",
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SearchableBlock {
    pub(super) attachment_id: Option<String>,
    pub(super) authored_text: String,
    pub(super) block_id: String,
    pub(super) block_type: String,
    pub(super) heading_context: Vec<String>,
    pub(super) ordinal: usize,
}

pub(super) fn validate(document_json: &str) -> Result<()> {
    parse_and_validate(document_json).map(|_| ())
}

pub(super) fn extract_attachments(document_json: &str) -> Result<Vec<DocumentAttachment>> {
    parse_and_validate(document_json).map(|(_, attachments)| attachments)
}

fn parse_and_validate(document_json: &str) -> Result<(Value, Vec<DocumentAttachment>)> {
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
    Ok((value, attachments))
}

pub(super) fn extract_searchable_blocks(document_json: &str) -> Result<Vec<SearchableBlock>> {
    let (value, _) = parse_and_validate(document_json)?;
    let blocks = value
        .get("blocks")
        .and_then(Value::as_array)
        .expect("validated document has blocks");
    let mut result = Vec::new();
    let mut headings = [None, None, None];
    collect_searchable_blocks(blocks, &mut headings, &mut result);
    Ok(result)
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
        if block_type == "heading" {
            let level = object
                .get("props")
                .and_then(Value::as_object)
                .and_then(|props| props.get("level"))
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    DatabaseError::InvalidInput(
                        "heading blocks must have an integer props.level from 1 to 3".into(),
                    )
                })?;
            if !(1..=3).contains(&level) {
                return invalid("heading blocks must have an integer props.level from 1 to 3");
            }
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

fn collect_searchable_blocks(
    blocks: &[Value],
    headings: &mut [Option<String>; 3],
    result: &mut Vec<SearchableBlock>,
) {
    for block in blocks {
        let object = block.as_object().expect("validated block is an object");
        let block_id = object
            .get("id")
            .and_then(Value::as_str)
            .expect("validated block has an id");
        let block_type = object
            .get("type")
            .and_then(Value::as_str)
            .expect("validated block has a type");
        let props = object.get("props").and_then(Value::as_object);
        let mut authored_text = match block_type {
            "displayMath" => string_prop(props, "latex")
                .filter(|latex| !latex.is_empty())
                .map_or_else(String::new, |latex| format!("$${latex}$$")),
            "koshImage" => {
                join_nonempty([string_prop(props, "altText"), string_prop(props, "caption")])
            }
            "koshFileAttachment" => string_prop(props, "caption").unwrap_or_default().to_owned(),
            _ => inline_text(object.get("content")),
        };
        authored_text = authored_text.trim().to_owned();
        let heading_level = (block_type == "heading").then(|| {
            props
                .and_then(|value| value.get("level"))
                .and_then(Value::as_u64)
                .and_then(|level| usize::try_from(level).ok())
                .expect("validated heading has an integer level from 1 to 3")
        });
        let heading_context = headings
            .iter()
            .flatten()
            .map(|heading| bounded_utf8_prefix(heading, MAX_HEADING_CONTEXT_BYTES_PER_LEVEL))
            .collect::<Vec<_>>();
        let attachment_id = attachment_kind(block_type)
            .and_then(|_| string_prop(props, "attachmentId").map(ToOwned::to_owned));
        result.push(SearchableBlock {
            attachment_id,
            authored_text: authored_text.clone(),
            block_id: block_id.to_owned(),
            block_type: block_type.to_owned(),
            heading_context,
            ordinal: result.len(),
        });
        if let Some(level) = heading_level {
            headings[level - 1] = (!authored_text.is_empty()).then_some(authored_text);
            headings
                .iter_mut()
                .skip(level)
                .for_each(|heading| *heading = None);
        }
        if let Some(children) = object.get("children").and_then(Value::as_array) {
            collect_searchable_blocks(children, headings, result);
        }
    }
}

fn bounded_utf8_prefix(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn inline_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items.iter().map(|item| inline_text(Some(item))).collect(),
        Some(Value::Object(object)) => match object.get("type").and_then(Value::as_str) {
            Some("text") => object
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            Some("inlineMath") => object
                .get("props")
                .and_then(Value::as_object)
                .and_then(|props| string_prop(Some(props), "latex"))
                .filter(|latex| !latex.is_empty())
                .map_or_else(String::new, |latex| format!("${latex}$")),
            _ => inline_text(object.get("content")),
        },
        _ => String::new(),
    }
}

fn string_prop<'a>(
    props: Option<&'a serde_json::Map<String, Value>>,
    key: &str,
) -> Option<&'a str> {
    props
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
}

fn join_nonempty<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> String {
    values
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn attachment_kind(block_type: &str) -> Option<AttachmentBlockKind> {
    [AttachmentBlockKind::Image, AttachmentBlockKind::File]
        .into_iter()
        .find(|kind| kind.block_type() == block_type)
}

fn invalid<T>(message: &str) -> Result<T> {
    Err(DatabaseError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::{
        extract_attachments, extract_searchable_blocks, validate, AttachmentBlockKind,
        MAX_HEADING_CONTEXT_BYTES_PER_LEVEL,
    };

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
    fn rejects_headings_without_a_supported_level() {
        for props in [
            r#"{}"#,
            r#"{"level":0}"#,
            r#"{"level":4}"#,
            r#"{"level":"2"}"#,
        ] {
            let document = format!(
                r#"{{"schemaVersion":1,"blocks":[{{"id":"heading","type":"heading","props":{props},"content":[]}}]}}"#
            );
            let error = validate(&document).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("integer props.level from 1 to 3"),
                "unexpected error for {props}: {error}"
            );
        }
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

    #[test]
    fn extracts_one_search_record_per_stable_block_in_document_order() {
        let blocks = extract_searchable_blocks(
            r#"{"schemaVersion":1,"blocks":[{"id":"heading","type":"heading","props":{"level":1},"content":[{"type":"text","text":"Vectors"}]},{"id":"paragraph","type":"paragraph","content":[{"type":"text","text":"Magnitude "},{"type":"inlineMath","props":{"latex":"\\lVert x \\rVert"}}],"children":[{"id":"nested","type":"bulletListItem","content":[{"type":"text","text":"Normalize first"}]}]},{"id":"image","type":"koshImage","props":{"attachmentId":"019f547b-6200-7000-8000-000000002001","altText":"Unit sphere","caption":"Geometry"}}]}"#,
        )
        .unwrap();
        assert_eq!(
            blocks
                .iter()
                .map(|block| block.block_id.as_str())
                .collect::<Vec<_>>(),
            ["heading", "paragraph", "nested", "image"]
        );
        assert_eq!(blocks[1].authored_text, "Magnitude $\\lVert x \\rVert$");
        assert_eq!(blocks[1].heading_context, ["Vectors"]);
        assert_eq!(blocks[2].heading_context, ["Vectors"]);
        assert_eq!(blocks[3].authored_text, "Unit sphere Geometry");
        assert_eq!(
            blocks[3].attachment_id.as_deref(),
            Some("019f547b-6200-7000-8000-000000002001")
        );
    }

    #[test]
    fn empty_blocks_do_not_inherit_heading_text_as_authored_content() {
        let blocks = extract_searchable_blocks(
            r#"{"schemaVersion":1,"blocks":[{"id":"heading","type":"heading","props":{"level":2},"content":[{"type":"text","text":"Context"}]},{"id":"empty","type":"paragraph","content":[]}]}"#,
        )
        .unwrap();
        assert!(blocks[1].authored_text.is_empty());
        assert_eq!(blocks[1].heading_context, ["Context"]);
    }

    #[test]
    fn inherited_heading_context_is_bounded_without_splitting_utf8() {
        let heading = "力".repeat(MAX_HEADING_CONTEXT_BYTES_PER_LEVEL);
        let document = serde_json::json!({
            "schemaVersion": 1,
            "blocks": [
                {
                    "id": "heading",
                    "type": "heading",
                    "props": {"level": 1},
                    "content": [{"type": "text", "text": heading}],
                },
                {"id": "paragraph", "type": "paragraph", "content": []},
            ],
        })
        .to_string();

        let blocks = extract_searchable_blocks(&document).expect("searchable blocks");
        let context = &blocks[1].heading_context[0];
        assert_eq!(context.len(), 255);
        assert_eq!(context.chars().count(), 85);
        assert!(heading.starts_with(context));
        assert_eq!(blocks[0].authored_text, heading);
    }
}
