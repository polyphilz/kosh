use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(super) const CONSTRUCTION_VERSION: &str = "markdown-blocks-v2";

const TARGET_PASSAGE_CHARS: usize = 700;
const MAX_PROSE_SEGMENT_CHARS: usize = 1_000;
const PROSE_OVERLAP_CHARS: usize = 100;
const MAX_ATOMIC_CODE_CHARS: usize = 1_600;
const CODE_TARGET_CHARS: usize = 1_200;
const CODE_OVERLAP_CHARS: usize = 100;
const CODE_OVERLAP_LINES: usize = 3;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarkdownLocator {
    pub start: u32,
    pub end: u32,
    #[serde(default)]
    pub source_start_byte: Option<u64>,
    #[serde(default)]
    pub source_end_byte: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_char: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_char: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BuiltPassage {
    pub ordinal: u32,
    pub content: String,
    pub content_hash: [u8; 32],
    pub heading_context: Vec<String>,
    pub locator: MarkdownLocator,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BlockKind {
    Code,
    DisplayMath,
    Empty,
    Heading(u8),
    Html,
    Prose,
    Rule,
    Table,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceBlock {
    ordinal: u32,
    end_ordinal: u32,
    kind: BlockKind,
    content: String,
    heading_context: Vec<String>,
    source_start_byte: usize,
    source_end_byte: usize,
}

struct Capture {
    kind: BlockKind,
    end_tag: TagEnd,
    nested_depth: usize,
    content: String,
    source_start_byte: usize,
}

struct SuspendedCapture {
    end_tag: TagEnd,
    nested_depth: usize,
}

pub(crate) fn build_markdown_passages(markdown: &str) -> Vec<BuiltPassage> {
    let blocks = parse_source_blocks(markdown);
    let mut pending = Vec::new();
    let mut passages = Vec::new();

    for block in blocks {
        match block.kind {
            BlockKind::Code => {
                flush_group(&mut pending, &mut passages);
                passages.extend(split_code_block(block));
            }
            BlockKind::DisplayMath
            | BlockKind::Heading(_)
            | BlockKind::Html
            | BlockKind::Rule
            | BlockKind::Table => {
                flush_group(&mut pending, &mut passages);
                passages.push(single_block_passage(block));
            }
            BlockKind::Empty => pending.push(block),
            BlockKind::Prose if char_count(&block.content) > MAX_PROSE_SEGMENT_CHARS => {
                flush_group(&mut pending, &mut passages);
                passages.extend(split_prose_block(block));
            }
            BlockKind::Prose => {
                let projected = pending
                    .iter()
                    .map(|candidate: &SourceBlock| char_count(&candidate.content))
                    .sum::<usize>()
                    + pending.len().saturating_mul(2)
                    + char_count(&block.content);
                let heading_changed = pending
                    .first()
                    .is_some_and(|candidate| candidate.heading_context != block.heading_context);
                if projected > TARGET_PASSAGE_CHARS || heading_changed {
                    flush_group(&mut pending, &mut passages);
                }
                pending.push(block);
            }
        }
    }
    flush_group(&mut pending, &mut passages);

    for (ordinal, passage) in passages.iter_mut().enumerate() {
        passage.ordinal = u32::try_from(ordinal).expect("passage count fits in u32");
    }
    passages
}

fn parse_source_blocks(markdown: &str) -> Vec<SourceBlock> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_GFM);
    options.insert(Options::ENABLE_MATH);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut blocks = Vec::new();
    let mut capture: Option<Capture> = None;
    let mut suspended_capture: Option<SuspendedCapture> = None;
    let mut headings: Vec<Option<String>> = vec![None; 6];
    let parser = Parser::new_ext(markdown, options).into_offset_iter();

    for (event, range) in parser {
        match event {
            Event::Start(tag) => {
                let nested_atomic_kind = capture
                    .as_ref()
                    .filter(|active| active.kind == BlockKind::Prose)
                    .and_then(|_| captured_block_kind(&tag))
                    .filter(|kind| *kind != BlockKind::Prose);
                if let Some(kind) = nested_atomic_kind {
                    let outer = capture.take().expect("captured list item");
                    suspended_capture = Some(SuspendedCapture {
                        end_tag: outer.end_tag,
                        nested_depth: outer.nested_depth,
                    });
                    finish_capture(outer, range.start, &mut headings, &mut blocks);
                    capture = Some(Capture {
                        kind,
                        end_tag: tag.to_end(),
                        nested_depth: 0,
                        content: String::new(),
                        source_start_byte: range.start,
                    });
                    continue;
                }
                if let Some(active) = capture.as_mut() {
                    active.nested_depth += 1;
                    continue;
                }
                if let Some(kind) = captured_block_kind(&tag) {
                    capture = Some(Capture {
                        kind,
                        end_tag: tag.to_end(),
                        nested_depth: 0,
                        content: String::new(),
                        source_start_byte: range.start,
                    });
                }
            }
            Event::End(tag) => {
                let Some(active) = capture.as_mut() else {
                    continue;
                };
                append_end_separator(active, tag);
                if active.nested_depth > 0 {
                    active.nested_depth -= 1;
                } else if active.end_tag == tag {
                    let finished = capture.take().expect("active Markdown capture");
                    let resumes_outer = suspended_capture.is_some();
                    finish_capture(finished, range.end, &mut headings, &mut blocks);
                    if resumes_outer {
                        if let Some(suspended) = suspended_capture.take() {
                            capture = Some(Capture {
                                kind: BlockKind::Prose,
                                end_tag: suspended.end_tag,
                                nested_depth: suspended.nested_depth,
                                content: String::new(),
                                source_start_byte: range.end,
                            });
                        }
                    }
                }
            }
            Event::DisplayMath(formula)
                if capture
                    .as_ref()
                    .is_some_and(|active| active.kind == BlockKind::Prose)
                    && capture
                        .as_ref()
                        .is_some_and(|active| active.end_tag == TagEnd::Item) =>
            {
                let outer = capture.take().expect("captured list item");
                let resume = SuspendedCapture {
                    end_tag: outer.end_tag,
                    nested_depth: outer.nested_depth,
                };
                finish_capture(outer, range.start, &mut headings, &mut blocks);
                push_standalone_block(
                    BlockKind::DisplayMath,
                    format!("$${formula}$$"),
                    range.start,
                    range.end,
                    &headings,
                    &mut blocks,
                );
                capture = Some(Capture {
                    kind: BlockKind::Prose,
                    end_tag: resume.end_tag,
                    nested_depth: resume.nested_depth,
                    content: String::new(),
                    source_start_byte: range.end,
                });
            }
            Event::DisplayMath(formula) if capture.is_none() => {
                let content = format!("$${formula}$$");
                push_standalone_block(
                    BlockKind::DisplayMath,
                    content,
                    range.start,
                    range.end,
                    &headings,
                    &mut blocks,
                );
            }
            Event::Rule if capture.is_none() => {
                push_standalone_block(
                    BlockKind::Rule,
                    "—".into(),
                    range.start,
                    range.end,
                    &headings,
                    &mut blocks,
                );
            }
            event => {
                if let Some(active) = capture.as_mut() {
                    append_event_content(active, event);
                }
            }
        }
    }

    if let Some(active) = capture {
        finish_capture(active, markdown.len(), &mut headings, &mut blocks);
    }
    if blocks.is_empty() {
        let source = markdown.trim();
        if !source.is_empty() {
            let passage_source = source
                .lines()
                .filter(|line| !crate::database::markdown::is_kosh_structure_marker(line))
                .collect::<Vec<_>>()
                .join("\n");
            if passage_source.trim().is_empty() {
                return blocks;
            }
            let normalized = authored_text(passage_source.trim());
            let content = if normalized.trim().is_empty() {
                // A media node is authored structure, but its opaque storage ID is
                // not authored evidence. Keep a non-tokenizing object marker so
                // media-only revisions still have a stable citation substrate.
                "\u{fffc}"
            } else {
                normalized.trim()
            };
            let source_start_byte = markdown.find(source).unwrap_or(0);
            blocks.push(SourceBlock {
                ordinal: 0,
                end_ordinal: 0,
                kind: BlockKind::Prose,
                content: content.into(),
                heading_context: Vec::new(),
                source_start_byte,
                source_end_byte: source_start_byte + source.len(),
            });
        }
    }
    assign_editor_ordinals(markdown, &mut blocks);
    blocks
}

fn assign_editor_ordinals(markdown: &str, blocks: &mut [SourceBlock]) {
    let starts = editor_block_starts(markdown);
    for block in blocks {
        let ordinals = starts
            .iter()
            .enumerate()
            .filter_map(|(ordinal, start)| {
                (*start >= block.source_start_byte && *start < block.source_end_byte)
                    .then_some(u32::try_from(ordinal).expect("Markdown block count fits in u32"))
            })
            .collect::<Vec<_>>();
        let fallback = starts
            .partition_point(|start| *start <= block.source_start_byte)
            .saturating_sub(1);
        block.ordinal = ordinals
            .first()
            .copied()
            .unwrap_or_else(|| u32::try_from(fallback).expect("Markdown block count fits in u32"));
        block.end_ordinal = ordinals.last().copied().unwrap_or(block.ordinal);
    }
}

fn editor_block_starts(markdown: &str) -> Vec<usize> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_GFM);
    options.insert(Options::ENABLE_MATH);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut starts = Vec::new();
    let mut list_depth = 0usize;
    let mut structured_child_depths = Vec::new();
    for (event, range) in Parser::new_ext(markdown, options).into_offset_iter() {
        let is_direct_block = || {
            list_depth == 0
                || structured_child_depths
                    .last()
                    .is_some_and(|depth| *depth == list_depth)
        };
        match event {
            Event::Start(Tag::Item) => {
                starts.push(range.start);
                list_depth += 1;
            }
            Event::Start(
                Tag::Paragraph | Tag::Heading { .. } | Tag::CodeBlock(_) | Tag::Table(_),
            ) if is_direct_block() => starts.push(range.start),
            Event::End(TagEnd::Item) => list_depth = list_depth.saturating_sub(1),
            Event::Html(value) => match value.trim() {
                crate::database::markdown::EMPTY_BLOCK_MARKER => starts.push(range.start),
                crate::database::markdown::CHILDREN_START_MARKER => {
                    structured_child_depths.push(list_depth);
                }
                crate::database::markdown::CHILDREN_END_MARKER => {
                    structured_child_depths.pop();
                }
                _ if is_direct_block() => starts.push(range.start),
                _ => {}
            },
            Event::DisplayMath(_) | Event::Rule if is_direct_block() => starts.push(range.start),
            _ => {}
        }
    }
    starts
}

fn captured_block_kind(tag: &Tag<'_>) -> Option<BlockKind> {
    match tag {
        Tag::Paragraph => Some(BlockKind::Prose),
        Tag::Heading { level, .. } => Some(BlockKind::Heading(heading_level(*level))),
        Tag::CodeBlock(_) => Some(BlockKind::Code),
        Tag::HtmlBlock => Some(BlockKind::Html),
        Tag::Item => Some(BlockKind::Prose),
        Tag::Table(_) => Some(BlockKind::Table),
        _ => None,
    }
}

fn append_event_content(capture: &mut Capture, event: Event<'_>) {
    match event {
        Event::Text(value) if capture.kind == BlockKind::Prose => {
            capture.content.push_str(&authored_text(&value));
        }
        Event::Text(value) => capture.content.push_str(&value),
        Event::Code(value) => capture.content.push_str(&value),
        Event::InlineMath(value) => {
            capture.content.push('$');
            capture.content.push_str(&value);
            capture.content.push('$');
        }
        Event::DisplayMath(value) => {
            if capture.kind == BlockKind::Prose && capture.content.trim().is_empty() {
                capture.kind = BlockKind::DisplayMath;
            }
            capture.content.push_str("$$");
            capture.content.push_str(&value);
            capture.content.push_str("$$");
        }
        Event::Html(value)
            if capture.kind == BlockKind::Html
                || !crate::database::markdown::is_kosh_structure_marker(&value) =>
        {
            capture.content.push_str(&value);
        }
        Event::Html(_) => {}
        Event::InlineHtml(value) => append_inline_html(capture, &value),
        Event::FootnoteReference(value) => {
            capture.content.push_str("[^");
            capture.content.push_str(&value);
            capture.content.push(']');
        }
        Event::SoftBreak | Event::HardBreak => capture.content.push('\n'),
        Event::Rule => capture.content.push('—'),
        Event::TaskListMarker(checked) => {
            capture
                .content
                .push_str(if checked { "[x] " } else { "[ ] " });
        }
        Event::Start(_) | Event::End(_) => {}
    }
}

fn authored_text(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(offset) = value[cursor..].find("{{kosh:") {
        let start = cursor + offset;
        normalized.push_str(&value[cursor..start]);
        let Some(end_offset) = value[start..].find("}}") else {
            normalized.push_str(&value[start..]);
            return normalized;
        };
        let end = start + end_offset + 2;
        let token = &value[start..end];
        if let Some(replacement) = authored_media_token_text(token) {
            normalized.push_str(&replacement);
        } else {
            normalized.push_str(token);
        }
        cursor = end;
    }
    normalized.push_str(&value[cursor..]);
    normalized
}

fn authored_media_token_text(token: &str) -> Option<String> {
    let payload = token.strip_prefix("{{kosh:")?.strip_suffix("}}")?;
    if let Some(payload) = payload.strip_prefix("attachment:") {
        let mut fields = payload.split(';');
        canonical_uuid_v7(fields.next()?)?;
        let caption = match fields.next() {
            Some(field) => crate::database::media::decode_canonical_token_field(
                field.strip_prefix("caption=")?,
                2_000,
            )?,
            None => String::new(),
        };
        return fields.next().is_none().then_some(caption);
    }
    if let Some(id) = payload.strip_prefix("pdf:") {
        canonical_uuid_v7(id)?;
        return Some(String::new());
    }
    let payload = payload.strip_prefix("image:")?;
    let mut fields = payload.split(';');
    canonical_uuid_v7(fields.next()?)?;
    let raw_width = fields.next()?.strip_prefix("width=")?.strip_suffix('%')?;
    let width = raw_width.parse::<u32>().ok()?;
    if !(10..=100).contains(&width) || width.to_string() != raw_width {
        return None;
    }
    let mut metadata = Vec::new();
    let mut saw_alt = false;
    let mut saw_caption = false;
    for field in fields {
        if let Some(value) = field.strip_prefix("alt=") {
            if saw_alt || saw_caption {
                return None;
            }
            metadata.push(crate::database::media::decode_canonical_token_field(
                value, 500,
            )?);
            saw_alt = true;
        } else {
            let value = field.strip_prefix("caption=")?;
            if saw_caption {
                return None;
            }
            metadata.push(crate::database::media::decode_canonical_token_field(
                value, 2_000,
            )?);
            saw_caption = true;
        }
    }
    Some(metadata.join(" "))
}

fn canonical_uuid_v7(value: &str) -> Option<()> {
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|id| id.get_version_num() == 7 && id.hyphenated().to_string().as_str() == value)
        .map(|_| ())
}

fn append_inline_html(capture: &mut Capture, value: &str) {
    let tag = value
        .trim()
        .strip_prefix('<')
        .map(|value| value.trim_start_matches('/').trim_start())
        .and_then(|value| {
            let end = value
                .find(|character: char| character.is_whitespace() || matches!(character, '/' | '>'))
                .unwrap_or(value.len());
            (end > 0).then(|| &value[..end])
        });
    if tag.is_some_and(|tag| tag.eq_ignore_ascii_case("br") || tag.eq_ignore_ascii_case("hr")) {
        capture.content.push('\n');
    }
}

fn append_end_separator(capture: &mut Capture, tag: TagEnd) {
    match capture.kind {
        BlockKind::Table => match tag {
            TagEnd::TableCell => capture.content.push('\t'),
            TagEnd::TableHead | TagEnd::TableRow => capture.content.push('\n'),
            _ => {}
        },
        BlockKind::Prose => match tag {
            TagEnd::Paragraph => capture.content.push_str("\n\n"),
            TagEnd::Item => capture.content.push('\n'),
            _ => {}
        },
        _ => {}
    }
}

fn finish_capture(
    capture: Capture,
    source_end_byte: usize,
    headings: &mut [Option<String>],
    blocks: &mut Vec<SourceBlock>,
) {
    let content = normalize_block_content(&capture.content, capture.kind);
    if capture.kind == BlockKind::Html
        && crate::database::markdown::is_kosh_structure_marker(&content)
    {
        if content.trim() == crate::database::markdown::EMPTY_BLOCK_MARKER {
            push_standalone_block(
                BlockKind::Empty,
                String::new(),
                capture.source_start_byte,
                source_end_byte,
                headings,
                blocks,
            );
        }
        return;
    }
    if content.is_empty() {
        return;
    }
    let heading_context = current_heading_context(headings);
    let heading_update = match capture.kind {
        BlockKind::Heading(level) => Some((level, content.clone())),
        _ => None,
    };
    let ordinal = u32::try_from(blocks.len()).expect("Markdown block count fits in u32");
    blocks.push(SourceBlock {
        ordinal,
        end_ordinal: ordinal,
        kind: capture.kind,
        content,
        heading_context,
        source_start_byte: capture.source_start_byte,
        source_end_byte,
    });
    if let Some((level, heading)) = heading_update {
        let level_index = usize::from(level.saturating_sub(1));
        for value in headings.iter_mut().skip(level_index) {
            *value = None;
        }
        headings[level_index] = Some(heading);
    }
}

fn push_standalone_block(
    kind: BlockKind,
    content: String,
    source_start_byte: usize,
    source_end_byte: usize,
    headings: &[Option<String>],
    blocks: &mut Vec<SourceBlock>,
) {
    let ordinal = u32::try_from(blocks.len()).expect("Markdown block count fits in u32");
    blocks.push(SourceBlock {
        ordinal,
        end_ordinal: ordinal,
        kind,
        content,
        heading_context: current_heading_context(headings),
        source_start_byte,
        source_end_byte,
    });
}

fn normalize_block_content(content: &str, kind: BlockKind) -> String {
    if kind == BlockKind::Code {
        return content.trim_matches('\n').to_owned();
    }
    let mut normalized = Vec::new();
    let mut prior_blank = false;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !prior_blank && !normalized.is_empty() {
                normalized.push(String::new());
            }
            prior_blank = true;
        } else {
            normalized.push(line.to_owned());
            prior_blank = false;
        }
    }
    while normalized.last().is_some_and(String::is_empty) {
        normalized.pop();
    }
    normalized.join("\n")
}

fn current_heading_context(headings: &[Option<String>]) -> Vec<String> {
    headings.iter().filter_map(Clone::clone).collect()
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn flush_group(pending: &mut Vec<SourceBlock>, passages: &mut Vec<BuiltPassage>) {
    if pending.is_empty() {
        return;
    }
    let first = pending.first().expect("nonempty passage group");
    let last = pending.last().expect("nonempty passage group");
    let content = pending
        .iter()
        .filter_map(|block| (!block.content.is_empty()).then_some(block.content.as_str()))
        .collect::<Vec<_>>()
        .join("\n\n");
    if content.is_empty() {
        pending.clear();
        return;
    }
    passages.push(finish_passage(
        content,
        first.heading_context.clone(),
        MarkdownLocator {
            start: first.ordinal,
            end: last.end_ordinal,
            source_start_byte: Some(
                u64::try_from(first.source_start_byte).expect("source offset fits in u64"),
            ),
            source_end_byte: Some(
                u64::try_from(last.source_end_byte).expect("source offset fits in u64"),
            ),
            start_char: None,
            end_char: None,
            start_line: None,
            end_line: None,
        },
    ));
    pending.clear();
}

fn single_block_passage(block: SourceBlock) -> BuiltPassage {
    let locator = block_locator(&block, None, None, None, None);
    finish_passage(block.content, block.heading_context, locator)
}

fn split_prose_block(block: SourceBlock) -> Vec<BuiltPassage> {
    let characters = block.content.chars().collect::<Vec<_>>();
    let ranges = prose_ranges(&characters);
    ranges
        .into_iter()
        .map(|(start, end)| {
            let content = characters[start..end].iter().collect::<String>();
            finish_passage(
                content,
                block.heading_context.clone(),
                block_locator(
                    &block,
                    Some(u32::try_from(start).expect("character offset fits in u32")),
                    Some(u32::try_from(end).expect("character offset fits in u32")),
                    None,
                    None,
                ),
            )
        })
        .collect()
}

fn prose_ranges(characters: &[char]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < characters.len() {
        let remaining = characters.len() - start;
        let raw_end = if remaining <= MAX_PROSE_SEGMENT_CHARS {
            characters.len()
        } else {
            prose_boundary(characters, start)
        };
        let mut trimmed_start = start;
        while trimmed_start < raw_end && characters[trimmed_start].is_whitespace() {
            trimmed_start += 1;
        }
        let mut trimmed_end = raw_end;
        while trimmed_end > trimmed_start && characters[trimmed_end - 1].is_whitespace() {
            trimmed_end -= 1;
        }
        if trimmed_start < trimmed_end {
            ranges.push((trimmed_start, trimmed_end));
        }
        if raw_end == characters.len() {
            break;
        }
        let overlap_target = raw_end.saturating_sub(PROSE_OVERLAP_CHARS);
        let next = (overlap_target..raw_end)
            .find(|position| characters[*position].is_whitespace())
            .map_or(overlap_target, |position| position + 1);
        start = next.max(start + 1);
    }
    ranges
}

fn prose_boundary(characters: &[char], start: usize) -> usize {
    let target = (start + TARGET_PASSAGE_CHARS).min(characters.len());
    let maximum = (start + MAX_PROSE_SEGMENT_CHARS).min(characters.len());
    let minimum = (start + TARGET_PASSAGE_CHARS / 2).min(maximum);
    let sentence = (minimum..maximum)
        .filter(|position| is_sentence_boundary(characters, *position))
        .min_by_key(|position| position.abs_diff(target));
    if let Some(position) = sentence {
        return position;
    }
    (target..maximum)
        .rev()
        .find(|position| characters[*position].is_whitespace())
        .unwrap_or(maximum)
}

fn is_sentence_boundary(characters: &[char], position: usize) -> bool {
    position > 0
        && position < characters.len()
        && matches!(characters[position - 1], '.' | '!' | '?' | '\n')
        && characters[position].is_whitespace()
}

fn split_code_block(block: SourceBlock) -> Vec<BuiltPassage> {
    if char_count(&block.content) <= MAX_ATOMIC_CODE_CHARS {
        return vec![single_block_passage(block)];
    }
    let lines = block.content.split('\n').collect::<Vec<_>>();
    let mut passages = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        if char_count(lines[start]) > MAX_ATOMIC_CODE_CHARS {
            passages.extend(split_oversized_code_line(&block, &lines, start));
            start += 1;
            continue;
        }
        let mut end = start;
        let mut length = 0;
        while end < lines.len() {
            if char_count(lines[end]) > MAX_ATOMIC_CODE_CHARS {
                break;
            }
            let projected = length + lines[end].chars().count() + usize::from(end > start);
            if end > start && projected > CODE_TARGET_CHARS {
                break;
            }
            length = projected;
            end += 1;
        }
        if end == start {
            end += 1;
        }
        passages.push(finish_passage(
            lines[start..end].join("\n"),
            block.heading_context.clone(),
            block_locator(
                &block,
                None,
                None,
                Some(u32::try_from(start + 1).expect("line offset fits in u32")),
                Some(u32::try_from(end).expect("line offset fits in u32")),
            ),
        ));
        if end == lines.len() {
            break;
        }
        start = if char_count(lines[end]) > MAX_ATOMIC_CODE_CHARS {
            end
        } else {
            end.saturating_sub(CODE_OVERLAP_LINES).max(start + 1)
        };
    }
    passages
}

fn split_oversized_code_line(
    block: &SourceBlock,
    lines: &[&str],
    line_index: usize,
) -> Vec<BuiltPassage> {
    let line_char_offset = lines[..line_index]
        .iter()
        .map(|line| char_count(line) + 1)
        .sum::<usize>();
    let characters = lines[line_index].chars().collect::<Vec<_>>();
    let mut passages = Vec::new();
    let mut start = 0;
    while start < characters.len() {
        let end = (start + CODE_TARGET_CHARS).min(characters.len());
        passages.push(finish_passage(
            characters[start..end].iter().collect(),
            block.heading_context.clone(),
            block_locator(
                block,
                Some(
                    u32::try_from(line_char_offset + start)
                        .expect("code character offset fits in u32"),
                ),
                Some(
                    u32::try_from(line_char_offset + end)
                        .expect("code character offset fits in u32"),
                ),
                Some(u32::try_from(line_index + 1).expect("line offset fits in u32")),
                Some(u32::try_from(line_index + 1).expect("line offset fits in u32")),
            ),
        ));
        if end == characters.len() {
            break;
        }
        start = end.saturating_sub(CODE_OVERLAP_CHARS).max(start + 1);
    }
    passages
}

fn block_locator(
    block: &SourceBlock,
    start_char: Option<u32>,
    end_char: Option<u32>,
    start_line: Option<u32>,
    end_line: Option<u32>,
) -> MarkdownLocator {
    MarkdownLocator {
        start: block.ordinal,
        end: block.end_ordinal,
        source_start_byte: Some(
            u64::try_from(block.source_start_byte).expect("source offset fits in u64"),
        ),
        source_end_byte: Some(
            u64::try_from(block.source_end_byte).expect("source offset fits in u64"),
        ),
        start_char,
        end_char,
        start_line,
        end_line,
    }
}

fn finish_passage(
    content: String,
    heading_context: Vec<String>,
    locator: MarkdownLocator,
) -> BuiltPassage {
    let content_hash = Sha256::digest(content.as_bytes()).into();
    BuiltPassage {
        ordinal: 0,
        content,
        content_hash,
        heading_context,
        locator,
    }
}

fn char_count(value: &str) -> usize {
    value.chars().count()
}

#[cfg(test)]
mod tests {
    use super::{
        build_markdown_passages, char_count, MarkdownLocator, CONSTRUCTION_VERSION,
        MAX_ATOMIC_CODE_CHARS, MAX_PROSE_SEGMENT_CHARS,
    };

    #[test]
    fn semantic_blocks_group_only_within_the_same_heading_context() {
        let markdown = [
            "# Thermodynamics",
            "",
            "Heat is impatient motion.",
            "",
            "Temperature measures a distribution.",
            "",
            "## Equations",
            "",
            "$$E = mc^2$$",
            "",
            "After the equation.",
        ]
        .join("\n");

        let first = build_markdown_passages(&markdown);
        let second = build_markdown_passages(&markdown);

        assert_eq!(first, second);
        assert_eq!(CONSTRUCTION_VERSION, "markdown-blocks-v2");
        assert_eq!(first.len(), 5);
        assert_eq!(first[0].content, "Thermodynamics");
        assert_eq!(
            first[1].content,
            "Heat is impatient motion.\n\nTemperature measures a distribution."
        );
        assert_eq!(first[1].heading_context, vec!["Thermodynamics"]);
        assert_eq!(first[2].content, "Equations");
        assert_eq!(first[2].heading_context, vec!["Thermodynamics"]);
        assert_eq!(first[3].content, "$$E = mc^2$$");
        assert_eq!(
            first[3].heading_context,
            vec!["Thermodynamics", "Equations"]
        );
        assert_eq!(
            first
                .iter()
                .map(|passage| passage.ordinal)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
        assert_valid_source_ranges(&markdown, first.iter().map(|passage| &passage.locator));
    }

    #[test]
    fn long_prose_splits_at_sentence_boundaries_with_bounded_overlap() {
        let sentence = "A deterministic sentence carries exact evidence. ";
        let markdown = sentence.repeat(40);
        let passages = build_markdown_passages(&markdown);

        assert!(passages.len() > 1);
        for passage in &passages {
            assert!(char_count(&passage.content) <= MAX_PROSE_SEGMENT_CHARS);
            assert_eq!(passage.locator.start, 0);
            assert_eq!(passage.locator.end, 0);
            let start = passage.locator.start_char.expect("split start character");
            let end = passage.locator.end_char.expect("split end character");
            assert!(start < end);
            assert_eq!(
                markdown
                    .chars()
                    .skip(start as usize)
                    .take((end - start) as usize)
                    .collect::<String>(),
                passage.content
            );
        }
        for pair in passages.windows(2) {
            assert!(
                pair[1].locator.start_char.expect("next start")
                    < pair[0].locator.end_char.expect("prior end"),
                "long prose passages retain limited overlap"
            );
        }
    }

    #[test]
    fn modest_code_is_atomic_and_large_code_splits_on_exact_line_ranges() {
        let modest = "```rust\nfn answer() -> i32 { 42 }\n```";
        let modest_passages = build_markdown_passages(modest);
        assert_eq!(modest_passages.len(), 1);
        assert_eq!(modest_passages[0].content, "fn answer() -> i32 { 42 }");
        assert_eq!(modest_passages[0].locator.start_line, None);

        let line = "let exact_identifier = compute_retrieval_evidence();";
        let code = std::iter::repeat_n(line, MAX_ATOMIC_CODE_CHARS / line.len() + 20)
            .collect::<Vec<_>>()
            .join("\n");
        let markdown = format!("# Implementation\n\n```rust\n{code}\n```");
        let passages = build_markdown_passages(&markdown);
        let code_passages = &passages[1..];
        assert!(code_passages.len() > 1);
        for passage in code_passages {
            assert_eq!(passage.heading_context, vec!["Implementation"]);
            let start = passage.locator.start_line.expect("code start line");
            let end = passage.locator.end_line.expect("code end line");
            assert!(start <= end);
            assert_eq!(
                code.lines()
                    .skip(start as usize - 1)
                    .take((end - start + 1) as usize)
                    .collect::<Vec<_>>()
                    .join("\n"),
                passage.content
            );
        }
        for pair in code_passages.windows(2) {
            assert!(
                pair[1].locator.start_line.expect("next line")
                    <= pair[0].locator.end_line.expect("prior line"),
                "large code passages overlap complete lines"
            );
        }
        assert_valid_source_ranges(&markdown, passages.iter().map(|passage| &passage.locator));
    }

    #[test]
    fn oversized_single_code_lines_split_on_exact_character_ranges() {
        let code = format!(
            "{{\"payload\":\"{}\"}}",
            "λ".repeat(MAX_ATOMIC_CODE_CHARS + 900)
        );
        let markdown = format!("# Embedded data\n\n```json\n{code}\n```");
        let passages = build_markdown_passages(&markdown);
        let code_passages = &passages[1..];

        assert!(code_passages.len() > 1);
        for passage in code_passages {
            assert!(char_count(&passage.content) <= MAX_ATOMIC_CODE_CHARS);
            assert_eq!(passage.heading_context, vec!["Embedded data"]);
            assert_eq!(passage.locator.start_line, Some(1));
            assert_eq!(passage.locator.end_line, Some(1));
            let start = passage.locator.start_char.expect("code start character");
            let end = passage.locator.end_char.expect("code end character");
            assert_eq!(
                code.chars()
                    .skip(start as usize)
                    .take((end - start) as usize)
                    .collect::<String>(),
                passage.content
            );
        }
        for pair in code_passages.windows(2) {
            assert!(
                pair[1].locator.start_char.expect("next start")
                    < pair[0].locator.end_char.expect("prior end"),
                "oversized code-line passages retain limited overlap"
            );
        }
        assert_valid_source_ranges(&markdown, passages.iter().map(|passage| &passage.locator));
    }

    #[test]
    fn nested_list_code_preserves_indentation_and_splits_by_line() {
        let code = std::iter::once("def retained_indentation():".to_owned())
            .chain((0..160).map(|index| format!("    value_{index} = {index}")))
            .collect::<Vec<_>>()
            .join("\n");
        let nested_code = code
            .lines()
            .map(|line| format!("  {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let markdown =
            format!("- Python evidence:\n\n  ```python\n{nested_code}\n  ```\n\n  Closing prose.");
        let passages = build_markdown_passages(&markdown);
        let code_passages = passages
            .iter()
            .filter(|passage| passage.locator.start_line.is_some())
            .collect::<Vec<_>>();

        assert!(code_passages.len() > 1);
        assert!(passages
            .iter()
            .any(|passage| passage.content == "Python evidence:"));
        assert!(passages
            .iter()
            .any(|passage| passage.content == "Closing prose."));
        for passage in code_passages {
            let start = passage.locator.start_line.expect("nested code start");
            let end = passage.locator.end_line.expect("nested code end");
            assert_eq!(
                code.lines()
                    .skip(start as usize - 1)
                    .take((end - start + 1) as usize)
                    .collect::<Vec<_>>()
                    .join("\n"),
                passage.content
            );
        }
        assert_valid_source_ranges(&markdown, passages.iter().map(|passage| &passage.locator));
    }

    #[test]
    fn nested_list_display_math_remains_an_atomic_semantic_block() {
        let markdown = "- Before equation.\n\n  $$x^2 + y^2$$\n\n  After equation.";
        let passages = build_markdown_passages(markdown);

        assert!(passages
            .iter()
            .any(|passage| passage.content == "Before equation."));
        assert!(passages
            .iter()
            .any(|passage| passage.content == "$$x^2 + y^2$$"));
        assert!(passages
            .iter()
            .any(|passage| passage.content == "After equation."));
        assert_valid_source_ranges(markdown, passages.iter().map(|passage| &passage.locator));
    }

    #[test]
    fn tables_tasks_and_unicode_have_stable_locator_json() {
        let markdown = [
            "## Δ observations",
            "",
            "- [x] measured",
            "- [ ] verify",
            "",
            "| Symbol | Meaning |",
            "| --- | --- |",
            "| λ | wavelength |",
        ]
        .join("\n");
        let passages = build_markdown_passages(&markdown);

        assert!(passages
            .iter()
            .any(|passage| passage.content.contains("[x]")));
        assert!(passages
            .iter()
            .any(|passage| passage.content.contains("wavelength")));
        for passage in &passages {
            let encoded = serde_json::to_string(&passage.locator).expect("serialize locator");
            let decoded: MarkdownLocator =
                serde_json::from_str(&encoded).expect("deserialize locator");
            assert_eq!(decoded, passage.locator);
        }
        assert_valid_source_ranges(&markdown, passages.iter().map(|passage| &passage.locator));
    }

    #[test]
    fn loose_and_nested_list_items_preserve_prose_boundaries() {
        let markdown = [
            "- First paragraph.",
            "",
            "  Second paragraph.",
            "",
            "  - Nested alpha.",
            "  - Nested beta.",
        ]
        .join("\n");

        let passages = build_markdown_passages(&markdown);
        let content = passages
            .iter()
            .map(|passage| passage.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(content.contains("First paragraph.\n\nSecond paragraph."));
        assert!(content.contains("Nested alpha.\nNested beta."));
        assert!(!content.contains("paragraph.Second"));
        assert!(!content.contains("alpha.Nested"));
    }

    #[test]
    fn inline_html_breaks_preserve_text_boundaries_without_retaining_markup() {
        let markdown = "alpha<br>beta <span class=\"term\">gamma</span><BR />delta<hr>epsilon";
        let passages = build_markdown_passages(markdown);

        assert_eq!(
            passages
                .iter()
                .map(|passage| passage.content.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            "alpha\nbeta gamma\ndelta\nepsilon"
        );
    }

    #[test]
    fn editor_structure_markers_are_not_searchable_passages() {
        let markdown = [
            "Before.",
            "",
            "<!-- kosh:block:empty -->",
            "",
            "<!-- kosh:children:start -->",
            "",
            "Nested evidence.",
            "",
            "<!-- kosh:children:end -->",
            "",
            "After.",
        ]
        .join("\n");
        let passages = build_markdown_passages(&markdown);
        let content = passages
            .iter()
            .map(|passage| passage.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(content.contains("Before."));
        assert!(content.contains("Nested evidence."));
        assert!(content.contains("After."));
        assert!(!content.contains("kosh:"));
        assert_eq!(passages[0].locator.start, 0);
        assert_eq!(passages[0].locator.end, 3);
        assert_valid_source_ranges(&markdown, passages.iter().map(|passage| &passage.locator));
    }

    #[test]
    fn empty_blocks_count_toward_grouped_passage_locators() {
        let markdown = ["Before.", "", "<!-- kosh:block:empty -->", "", "After."].join("\n");
        let passages = build_markdown_passages(&markdown);

        assert_eq!(passages.len(), 1);
        assert_eq!(passages[0].content, "Before.\n\nAfter.");
        assert_eq!(passages[0].locator.start, 0);
        assert_eq!(passages[0].locator.end, 2);
        assert_valid_source_ranges(&markdown, passages.iter().map(|passage| &passage.locator));
    }

    #[test]
    fn structure_markers_nested_in_lists_are_not_searchable() {
        let markdown = [
            "- Parent",
            "",
            "  <!-- kosh:children:start -->",
            "",
            "  Nested evidence.",
            "",
            "  <!-- kosh:children:end -->",
        ]
        .join("\n");
        let content = build_markdown_passages(&markdown)
            .into_iter()
            .map(|passage| passage.content)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(content.contains("Parent"));
        assert!(content.contains("Nested evidence."));
        assert!(!content.contains("kosh:"));
    }

    #[test]
    fn nested_list_blocks_count_toward_passage_locators() {
        let markdown = ["- Parent", "  - Nested alpha", "    - Nested beta"].join("\n");
        let passages = build_markdown_passages(&markdown);

        assert_eq!(passages.len(), 1);
        assert_eq!(passages[0].locator.start, 0);
        assert_eq!(passages[0].locator.end, 2);
    }

    #[test]
    fn nested_prose_and_empty_blocks_count_toward_passage_locators() {
        let markdown = [
            "- Parent",
            "",
            "  <!-- kosh:children:start -->",
            "",
            "  Nested evidence.",
            "",
            "  <!-- kosh:block:empty -->",
            "",
            "  <!-- kosh:children:end -->",
        ]
        .join("\n");
        let passages = build_markdown_passages(&markdown);

        assert_eq!(passages.len(), 1);
        assert_eq!(passages[0].locator.start, 0);
        assert_eq!(passages[0].locator.end, 2);
        assert!(!passages[0].content.contains("kosh:"));
    }

    #[test]
    fn fenced_code_preserves_reserved_structure_marker_text() {
        let markdown = "```text\n<!-- kosh:block:empty -->\n```";
        let passages = build_markdown_passages(markdown);

        assert_eq!(passages.len(), 1);
        assert_eq!(passages[0].content, "<!-- kosh:block:empty -->");
    }

    #[test]
    fn marker_only_revisions_do_not_produce_searchable_passages() {
        let markdown = [
            "<!-- kosh:block:empty -->",
            "",
            "<!-- kosh:children:start -->",
            "",
            "<!-- kosh:block:empty -->",
            "",
            "<!-- kosh:children:end -->",
        ]
        .join("\n");

        assert!(build_markdown_passages(&markdown).is_empty());
    }

    #[test]
    fn canonical_media_tokens_index_authored_metadata_without_opaque_ids() {
        let attachment_id = "019f547b-6200-7000-8000-000000000771";
        let image_id = "019f547b-6200-7000-8000-000000000772";
        let markdown = format!(
            "Nearby context {{{{kosh:attachment:{attachment_id};caption=Useful%20appendix}}}} \
             and {{{{kosh:image:{image_id};width=70%;alt=Diagram;caption=Chapter%202}}}}."
        );
        let content = build_markdown_passages(&markdown)
            .into_iter()
            .map(|passage| passage.content)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(content.contains("Nearby context Useful appendix and Diagram Chapter 2."));
        assert!(!content.contains(attachment_id));
        assert!(!content.contains(image_id));
    }

    #[test]
    fn media_only_blocks_keep_structure_without_indexing_opaque_ids() {
        let attachment_id = "019f547b-6200-7000-8000-000000000773";
        let markdown = format!("{{{{kosh:attachment:{attachment_id}}}}}");
        let passages = build_markdown_passages(&markdown);

        assert_eq!(passages.len(), 1);
        assert_eq!(passages[0].content, "\u{fffc}");
        assert!(!passages[0].content.contains(attachment_id));
        assert_eq!(
            passages[0].locator.source_start_byte,
            Some(0),
            "the object marker still cites the original media node"
        );
        assert_eq!(
            passages[0].locator.source_end_byte,
            Some(markdown.len() as u64)
        );
    }

    #[test]
    fn fenced_code_preserves_literal_media_tokens() {
        let attachment_id = "019f547b-6200-7000-8000-000000000774";
        let markdown = format!("```text\n{{{{kosh:attachment:{attachment_id}}}}}\n```");
        let passages = build_markdown_passages(&markdown);

        assert_eq!(passages.len(), 1);
        assert_eq!(
            passages[0].content,
            format!("{{{{kosh:attachment:{attachment_id}}}}}")
        );
    }

    fn assert_valid_source_ranges<'a>(
        markdown: &str,
        locators: impl IntoIterator<Item = &'a MarkdownLocator>,
    ) {
        for locator in locators {
            let start = usize::try_from(
                locator
                    .source_start_byte
                    .expect("generated locator start byte"),
            )
            .expect("start byte");
            let end = usize::try_from(locator.source_end_byte.expect("generated locator end byte"))
                .expect("end byte");
            assert!(start < end);
            assert!(markdown.get(start..end).is_some());
        }
    }
}
