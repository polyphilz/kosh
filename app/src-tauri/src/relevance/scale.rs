use std::time::Instant;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{RelevanceError, Result};

pub const SCALE_SCHEMA_VERSION: u32 = 1;
const SCALE_GENERATOR_VERSION: &str = "kosh-scale-v1";
const DEFAULT_SCALE_SEED: u64 = 0x4b4f_5348_5f56_3108;
const DEFAULT_SCALE_COUNT: usize = 10_000;
const BASE_TIMESTAMP_MS: u64 = 1_785_139_200_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScaleGenerationOptions {
    pub seed: u64,
    pub count: usize,
}

impl Default for ScaleGenerationOptions {
    fn default() -> Self {
        Self {
            seed: DEFAULT_SCALE_SEED,
            count: DEFAULT_SCALE_COUNT,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScaleCorpus {
    pub schema_version: u32,
    pub generator_version: String,
    pub seed: String,
    pub tidbits: Vec<ScaleTidbit>,
    pub stats: ScaleStats,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScaleTidbit {
    pub id: String,
    pub revision_id: String,
    pub created_at_ms: u64,
    pub title: Option<String>,
    pub body_markdown: String,
    pub length_class: ScaleLengthClass,
    pub sources: Vec<ScaleSource>,
    pub attachments: Vec<ScaleAttachment>,
    pub exact_duplicate_of: Option<String>,
    pub near_duplicate_of: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScaleSource {
    pub label: String,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScaleAttachment {
    pub id: String,
    pub filename: String,
    pub media_type: String,
    pub byte_length: u64,
    pub extraction_placeholder: AttachmentExtractionPlaceholder,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AttachmentExtractionPlaceholder {
    None,
    Ocr,
    PdfText,
    Text,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScaleLengthClass {
    Long,
    Medium,
    Short,
    VeryLong,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScaleStats {
    pub tidbit_count: u32,
    pub short_count: u32,
    pub medium_count: u32,
    pub long_count: u32,
    pub very_long_count: u32,
    pub exact_duplicate_count: u32,
    pub near_duplicate_count: u32,
    pub with_code_count: u32,
    pub with_formula_count: u32,
    pub with_source_count: u32,
    pub with_attachment_count: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScalePerformanceReport {
    pub schema_version: u32,
    pub workload: String,
    pub generator_version: String,
    pub seed: String,
    pub tidbit_count: u32,
    pub serialized_bytes: u64,
    pub generation_duration_ms: f64,
    pub runtime: RuntimeMetadata,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeMetadata {
    pub operating_system: String,
    pub architecture: String,
    pub logical_cpu_count: u32,
    pub build_profile: String,
    pub app_version: String,
    pub reference_hardware: Option<String>,
}

pub fn generate_scale_corpus(options: ScaleGenerationOptions) -> Result<ScaleCorpus> {
    if options.count == 0 {
        return Err(RelevanceError::InvalidFixture(
            "scale corpus count must be positive".into(),
        ));
    }
    if options.count > u32::MAX as usize {
        return Err(RelevanceError::InvalidFixture(
            "scale corpus count exceeds report capacity".into(),
        ));
    }

    let mut random = SplitMix64::new(options.seed);
    let mut tidbits = Vec::<ScaleTidbit>::with_capacity(options.count);
    let mut stats = ScaleStats::default();
    for index in 0..options.count {
        let mut length_class = length_class(random.next());
        let requested_code = index.is_multiple_of(11);
        let requested_formula = index.is_multiple_of(13);
        let mut body_markdown = generated_body(
            index,
            length_class,
            requested_code,
            requested_formula,
            random.next(),
        );
        let mut title = (!index.is_multiple_of(4)).then(|| generated_title(index));
        let mut sources = generated_sources(index);
        let mut attachments = generated_attachments(options.seed, index);
        let mut exact_duplicate_of = None;
        let mut near_duplicate_of = None;
        if index > 0 && index.is_multiple_of(50) {
            let previous = &tidbits[index - 1];
            title.clone_from(&previous.title);
            body_markdown.clone_from(&previous.body_markdown);
            length_class = previous.length_class;
            sources.clone_from(&previous.sources);
            attachments.clone_from(&previous.attachments);
            exact_duplicate_of = Some(previous.id.clone());
            stats.exact_duplicate_count += 1;
        } else if index > 0 && index.is_multiple_of(37) {
            let previous = &tidbits[index - 1];
            body_markdown = format!(
                "{}\n\nFollow-up wording {} keeps the evidence nearly duplicated.",
                previous.body_markdown,
                index % 17
            );
            length_class = previous.length_class;
            near_duplicate_of = Some(previous.id.clone());
            stats.near_duplicate_count += 1;
        }

        let with_code = body_markdown.contains("```");
        let with_formula = body_markdown.contains("$$");
        stats.tidbit_count += 1;
        match length_class {
            ScaleLengthClass::Short => stats.short_count += 1,
            ScaleLengthClass::Medium => stats.medium_count += 1,
            ScaleLengthClass::Long => stats.long_count += 1,
            ScaleLengthClass::VeryLong => stats.very_long_count += 1,
        }
        stats.with_code_count += u32::from(with_code);
        stats.with_formula_count += u32::from(with_formula);
        stats.with_source_count += u32::from(!sources.is_empty());
        stats.with_attachment_count += u32::from(!attachments.is_empty());

        tidbits.push(ScaleTidbit {
            id: deterministic_uuid(options.seed, index, 0),
            revision_id: deterministic_uuid(options.seed, index, 1),
            created_at_ms: BASE_TIMESTAMP_MS + index as u64,
            title,
            body_markdown,
            length_class,
            sources,
            attachments,
            exact_duplicate_of,
            near_duplicate_of,
        });
    }

    Ok(ScaleCorpus {
        schema_version: SCALE_SCHEMA_VERSION,
        generator_version: SCALE_GENERATOR_VERSION.into(),
        seed: display_seed(options.seed),
        tidbits,
        stats,
    })
}

pub fn measure_scale_generation(
    options: ScaleGenerationOptions,
) -> Result<(ScaleCorpus, ScalePerformanceReport)> {
    let started = Instant::now();
    let corpus = generate_scale_corpus(options)?;
    let duration = started.elapsed();
    let serialized_bytes = serde_json::to_vec(&corpus)
        .map_err(|source| RelevanceError::Json {
            path: "generated scale corpus".into(),
            source,
        })?
        .len();
    let report = ScalePerformanceReport {
        schema_version: SCALE_SCHEMA_VERSION,
        workload: "deterministic-scale-generation".into(),
        generator_version: corpus.generator_version.clone(),
        seed: corpus.seed.clone(),
        tidbit_count: corpus.stats.tidbit_count,
        serialized_bytes: u64::try_from(serialized_bytes)
            .expect("serialized scale corpus size fits in u64"),
        generation_duration_ms: duration.as_secs_f64() * 1_000.0,
        runtime: RuntimeMetadata::capture(),
    };
    Ok((corpus, report))
}

impl RuntimeMetadata {
    pub(crate) fn capture() -> Self {
        Self {
            operating_system: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            logical_cpu_count: std::thread::available_parallelism()
                .map(|count| u32::try_from(count.get()).unwrap_or(u32::MAX))
                .unwrap_or(1),
            build_profile: if cfg!(debug_assertions) {
                "debug".into()
            } else {
                "release".into()
            },
            app_version: env!("CARGO_PKG_VERSION").into(),
            reference_hardware: std::env::var("KOSH_REFERENCE_HARDWARE")
                .ok()
                .filter(|value| !value.trim().is_empty()),
        }
    }
}

fn generated_body(
    index: usize,
    length_class: ScaleLengthClass,
    with_code: bool,
    with_formula: bool,
    random: u64,
) -> String {
    const TOPICS: [(&str, &str); 8] = [
        (
            "retrieval",
            "Passage retrieval should preserve exact provenance while ranking useful evidence.",
        ),
        (
            "gardening",
            "Basil responds best when soil moisture is checked before another watering.",
        ),
        (
            "distributed systems",
            "A durable queue acknowledges work only after its state transition is committed.",
        ),
        (
            "thermodynamics",
            "Temperature describes a distribution rather than the energy of one particle.",
        ),
        (
            "music",
            "Voice leading sounds calmer when adjacent parts move by the smallest interval.",
        ),
        (
            "language",
            "A useful vocabulary note includes the phrase where the unfamiliar word appeared.",
        ),
        (
            "databases",
            "A covering index can answer a selective query without visiting the base table.",
        ),
        (
            "cooking",
            "Resting dough gives flour time to hydrate before the final kneading pass.",
        ),
    ];
    let topic = TOPICS[(random as usize) % TOPICS.len()];
    let repeats = match length_class {
        ScaleLengthClass::Short => 1,
        ScaleLengthClass::Medium => 4,
        ScaleLengthClass::Long => 18,
        ScaleLengthClass::VeryLong => 60,
    };
    let mut body = format!("# {}\n\n", title_case(topic.0));
    for paragraph in 0..repeats {
        if paragraph > 0 {
            body.push_str("\n\n");
        }
        body.push_str(&format!(
            "{} Observation {}-{} records token kosh_{:05} with enough surrounding context for realistic passage construction.",
            topic.1,
            index,
            paragraph,
            (index * 31 + paragraph) % 100_000,
        ));
    }
    if with_code {
        body.push_str(&format!(
            "\n\n```rust\nfn fixture_key_{index}(ordinal: u32) -> String {{\n    format!(\"tidbit-{index}-{{ordinal}}\")\n}}\n```"
        ));
    }
    if with_formula {
        body.push_str(&format!(
            "\n\n$$score_{{{index}}} = \\frac{{lexical + semantic}}{{1 + rank}}$$"
        ));
    }
    body
}

fn display_seed(seed: u64) -> String {
    format!("0x{seed:016x}")
}

fn generated_title(index: usize) -> String {
    const TITLES: [&str; 8] = [
        "Retrieval note",
        "Garden observation",
        "Queue invariant",
        "Thermal model",
        "Harmony sketch",
        "Vocabulary card",
        "Database detail",
        "Kitchen experiment",
    ];
    format!("{} {:05}", TITLES[index % TITLES.len()], index)
}

fn generated_sources(index: usize) -> Vec<ScaleSource> {
    if !index.is_multiple_of(3) {
        return Vec::new();
    }
    const SOURCES: [(&str, &str); 4] = [
        ("SQLite documentation", "https://sqlite.org/fts5.html"),
        (
            "Rust standard library",
            "https://doc.rust-lang.org/std/index.html",
        ),
        (
            "Local lecture handout",
            "https://notes.example.test/lecture/retrieval",
        ),
        (
            "Reference article",
            "https://knowledge.example.test/evidence",
        ),
    ];
    let source = SOURCES[(index / 3) % SOURCES.len()];
    vec![ScaleSource {
        label: source.0.into(),
        url: format!("{}?fixture={index}", source.1),
    }]
}

fn generated_attachments(seed: u64, index: usize) -> Vec<ScaleAttachment> {
    if !index.is_multiple_of(5) {
        return Vec::new();
    }
    let choice = (index / 5) % 4;
    let (extension, media_type, extraction_placeholder) = match choice {
        0 => ("png", "image/png", AttachmentExtractionPlaceholder::Ocr),
        1 => (
            "pdf",
            "application/pdf",
            AttachmentExtractionPlaceholder::PdfText,
        ),
        2 => ("md", "text/markdown", AttachmentExtractionPlaceholder::Text),
        _ => (
            "zip",
            "application/zip",
            AttachmentExtractionPlaceholder::None,
        ),
    };
    vec![ScaleAttachment {
        id: deterministic_uuid(seed, index, 2),
        filename: format!("fixture-{index:05}.{extension}"),
        media_type: media_type.into(),
        byte_length: 1_024 + ((index as u64 * 7_919) % 4_000_000),
        extraction_placeholder,
    }]
}

fn length_class(random: u64) -> ScaleLengthClass {
    match random % 100 {
        0..=49 => ScaleLengthClass::Short,
        50..=84 => ScaleLengthClass::Medium,
        85..=96 => ScaleLengthClass::Long,
        _ => ScaleLengthClass::VeryLong,
    }
}

fn deterministic_uuid(seed: u64, index: usize, lane: u64) -> String {
    let timestamp = BASE_TIMESTAMP_MS + index as u64;
    let entropy_a = mix64(seed ^ (index as u64).rotate_left(17) ^ lane.rotate_left(41));
    let entropy_b = mix64(entropy_a ^ 0x9e37_79b9_7f4a_7c15);
    let mut bytes = [0_u8; 16];
    let timestamp_bytes = timestamp.to_be_bytes();
    bytes[..6].copy_from_slice(&timestamp_bytes[2..]);
    bytes[6] = 0x70 | ((entropy_a >> 60) as u8 & 0x0f);
    bytes[7] = (entropy_a >> 52) as u8;
    bytes[8] = 0x80 | ((entropy_a >> 46) as u8 & 0x3f);
    bytes[9..16].copy_from_slice(&entropy_b.to_be_bytes()[1..]);
    Uuid::from_bytes(bytes).to_string()
}

fn title_case(value: &str) -> String {
    let mut characters = value.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + characters.as_str()
    })
}

fn mix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        mix64(self.state)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use uuid::{Uuid, Version};

    use super::{
        generate_scale_corpus, measure_scale_generation, ScaleGenerationOptions, ScaleLengthClass,
        SCALE_SCHEMA_VERSION,
    };

    #[test]
    fn same_seed_produces_identical_corpus_and_ids() {
        let options = ScaleGenerationOptions {
            seed: 0x1234_5678,
            count: 256,
        };
        let first = generate_scale_corpus(options).expect("first scale corpus");
        let second = generate_scale_corpus(options).expect("second scale corpus");

        assert_eq!(first, second);
        assert_eq!(first.schema_version, SCALE_SCHEMA_VERSION);
        for tidbit in &first.tidbits {
            assert_eq!(
                Uuid::parse_str(&tidbit.id)
                    .expect("tidbit fixture UUID")
                    .get_version(),
                Some(Version::SortRand)
            );
            assert_eq!(
                Uuid::parse_str(&tidbit.revision_id)
                    .expect("revision fixture UUID")
                    .get_version(),
                Some(Version::SortRand)
            );
        }
    }

    #[test]
    fn default_workload_has_ten_thousand_realistically_mixed_tidbits() {
        let corpus =
            generate_scale_corpus(ScaleGenerationOptions::default()).expect("10k scale corpus");

        assert_eq!(corpus.tidbits.len(), 10_000);
        assert_eq!(corpus.stats.tidbit_count, 10_000);
        assert!(corpus.stats.short_count > corpus.stats.medium_count);
        assert!(corpus.stats.medium_count > corpus.stats.long_count);
        assert!(corpus.stats.long_count > corpus.stats.very_long_count);
        assert!(corpus.stats.exact_duplicate_count > 100);
        assert!(corpus.stats.near_duplicate_count > 100);
        assert!(corpus.stats.with_code_count > 800);
        assert!(corpus.stats.with_formula_count > 700);
        assert_eq!(corpus.stats.with_attachment_count, 1_801);
        assert!(corpus
            .tidbits
            .iter()
            .any(|tidbit| tidbit.length_class == ScaleLengthClass::VeryLong));
        assert!(corpus
            .tidbits
            .iter()
            .flat_map(|tidbit| &tidbit.attachments)
            .any(|attachment| attachment.media_type == "application/pdf"));

        let tidbits_by_id = corpus
            .tidbits
            .iter()
            .map(|tidbit| (tidbit.id.as_str(), tidbit))
            .collect::<HashMap<_, _>>();
        for duplicate in corpus
            .tidbits
            .iter()
            .filter(|tidbit| tidbit.exact_duplicate_of.is_some())
        {
            let original = tidbits_by_id
                .get(
                    duplicate
                        .exact_duplicate_of
                        .as_deref()
                        .expect("exact duplicate reference"),
                )
                .expect("referenced exact duplicate");
            assert_eq!(duplicate.title, original.title);
            assert_eq!(duplicate.body_markdown, original.body_markdown);
            assert_eq!(duplicate.length_class, original.length_class);
            assert_eq!(duplicate.sources, original.sources);
            assert_eq!(duplicate.attachments, original.attachments);
        }
    }

    #[test]
    fn performance_report_records_runtime_without_enforcing_wall_clock() {
        let (corpus, report) = measure_scale_generation(ScaleGenerationOptions {
            seed: 42,
            count: 50,
        })
        .expect("measured scale corpus");

        assert_eq!(report.tidbit_count, 50);
        assert!(report.serialized_bytes > 0);
        assert!(report.generation_duration_ms >= 0.0);
        assert!(!report.runtime.operating_system.is_empty());
        assert!(!report.runtime.architecture.is_empty());
        assert!(report.runtime.logical_cpu_count > 0);
        assert_eq!(corpus.seed, report.seed);
        let corpus_json = serde_json::to_string(&corpus).expect("serialize scale corpus");
        let decoded = serde_json::from_str(&corpus_json).expect("deserialize scale corpus");
        assert_eq!(corpus, decoded);
    }
}
