use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde::Serialize;

use super::{CitationLocator, CitationResolution};

const MARKER_OPEN: char = '【';
const MARKER_CLOSE: char = '】';
const MAX_DIGITS: usize = 10;
const MAX_MENTIONS: usize = 2_048;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum LegacyEvidenceKind {
    AuthoredTidbit,
    PdfPage,
    ImageOcr,
    TextLines,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LegacyResearchCitation {
    pub number: u32,
    pub label: String,
    pub evidence_kind: LegacyEvidenceKind,
    pub evidence: CitationResolution,
}

pub(super) fn citation(number: u32, evidence: CitationResolution) -> LegacyResearchCitation {
    let evidence_kind = match &evidence.locator {
        CitationLocator::MarkdownBlocks { .. } => LegacyEvidenceKind::AuthoredTidbit,
        CitationLocator::PdfPage { .. } => LegacyEvidenceKind::PdfPage,
        CitationLocator::OcrRegion { .. } => LegacyEvidenceKind::ImageOcr,
        CitationLocator::TextLines { .. } => LegacyEvidenceKind::TextLines,
    };
    let label = match &evidence.locator {
        CitationLocator::MarkdownBlocks { .. } => evidence
            .tidbit
            .as_ref()
            .map(|tidbit| tidbit.display_title.clone())
            .unwrap_or_else(|| "Kosh tidbit".into()),
        CitationLocator::PdfPage { page } => {
            format!(
                "{}, page {page}",
                attachment_label(&evidence, "PDF attachment")
            )
        }
        CitationLocator::OcrRegion { page, .. } => page.map_or_else(
            || attachment_label(&evidence, "image"),
            |page| format!("{}, page {page}", attachment_label(&evidence, "image")),
        ),
        CitationLocator::TextLines {
            start_line,
            end_line,
        } => format!(
            "{}, lines {start_line}–{end_line}",
            attachment_label(&evidence, "text attachment")
        ),
    };
    LegacyResearchCitation {
        number,
        label,
        evidence_kind,
        evidence,
    }
}

fn attachment_label(citation: &CitationResolution, fallback: &str) -> String {
    citation
        .attachment
        .as_ref()
        .map(|attachment| attachment.display_filename.clone())
        .unwrap_or_else(|| fallback.into())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LegacyCitationMention {
    citation_number: u32,
    start_byte: usize,
    end_byte: usize,
}

pub(super) fn citation_mentions(
    markdown: &str,
    citation_count: usize,
) -> Vec<LegacyCitationMention> {
    if citation_count == 0 {
        return Vec::new();
    }
    let mut mentions = Vec::new();
    for range in citation_text_ranges(markdown) {
        let mut cursor = range.start;
        while cursor < range.end && mentions.len() < MAX_MENTIONS {
            let Some(relative_start) = markdown[cursor..range.end].find(MARKER_OPEN) else {
                break;
            };
            let start = cursor + relative_start;
            let body_start = start + MARKER_OPEN.len_utf8();
            let mut search_end = body_start
                .saturating_add(MAX_DIGITS + MARKER_CLOSE.len_utf8())
                .min(range.end);
            while !markdown.is_char_boundary(search_end) {
                search_end -= 1;
            }
            let Some(relative_end) = markdown[body_start..search_end].find(MARKER_CLOSE) else {
                cursor = body_start;
                continue;
            };
            let body_end = body_start + relative_end;
            let end = body_end + MARKER_CLOSE.len_utf8();
            let body = &markdown[body_start..body_end];
            let number = body
                .bytes()
                .all(|byte| byte.is_ascii_digit())
                .then(|| body.parse::<u32>().ok())
                .flatten();
            if let Some(citation_number) = number.filter(|number| {
                *number > 0 && usize::try_from(*number).is_ok_and(|number| number <= citation_count)
            }) {
                mentions.push(LegacyCitationMention {
                    citation_number,
                    start_byte: start,
                    end_byte: end,
                });
            }
            cursor = end;
        }
    }
    mentions
}

fn citation_text_ranges(markdown: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut code_block_depth = 0_u32;
    let mut image_depth = 0_u32;
    let mut link_depth = 0_u32;
    let mut options = Options::empty();
    options.insert(Options::ENABLE_GFM);
    options.insert(Options::ENABLE_MATH);
    for (event, range) in Parser::new_ext(markdown, options).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => code_block_depth += 1,
            Event::Start(Tag::Image { .. }) => image_depth += 1,
            Event::Start(Tag::Link { .. }) => link_depth += 1,
            Event::End(TagEnd::CodeBlock) => code_block_depth = code_block_depth.saturating_sub(1),
            Event::End(TagEnd::Image) => image_depth = image_depth.saturating_sub(1),
            Event::End(TagEnd::Link) => link_depth = link_depth.saturating_sub(1),
            Event::Text(_) if code_block_depth == 0 && image_depth == 0 && link_depth == 0 => {
                ranges.push(range);
            }
            _ => {}
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::citation_mentions;

    #[test]
    fn preserves_only_valid_visible_legacy_markers() {
        let markdown = "Visible.【1】 `[2] 【2】` [linked 【2】](https://example.com)";
        let mentions = serde_json::to_value(citation_mentions(markdown, 2)).expect("mentions");
        assert_eq!(
            mentions,
            serde_json::json!([{
                "citationNumber": 1,
                "startByte": markdown.find('【').expect("marker"),
                "endByte": markdown.find('】').expect("marker") + '】'.len_utf8()
            }])
        );
    }
}
