use serde::{Deserialize, Serialize};

pub(crate) const JINA_V1_MANIFEST_JSON: &str =
    include_str!("../resources/embedding-indexes/jina-v1.json");
pub(crate) const JINA_V1_GOLDEN_JSON: &str =
    include_str!("../resources/embedding-indexes/jina-v1-golden.json");
pub(crate) const LLAMA_SERVER_V1_PIN_JSON: &str =
    include_str!("../resources/sidecars/llama-server-v1.json");

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextEmbeddingManifest {
    pub manifest_version: u32,
    pub id: String,
    pub created_at: i64,
    pub index_key: String,
    pub model_name: String,
    pub model_revision: String,
    pub model_file_sha256: String,
    pub dimension: u32,
    pub distance_metric: String,
    pub normalized: bool,
    pub index_schema_version: u32,
    pub config: TextEmbeddingConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextEmbeddingConfig {
    pub schema_version: u32,
    pub model_file: String,
    pub model_file_size: u64,
    pub quantization: String,
    pub pooling: String,
    pub normalization: String,
    pub query_prefix: String,
    pub document_prefix: String,
    pub document_construction_version: u32,
}

impl TextEmbeddingManifest {
    pub(crate) fn query_input(&self, query: &str) -> String {
        format!("{}{}", self.config.query_prefix, query)
    }

    pub(crate) fn document_input(&self, document: &str) -> String {
        format!("{}{}", self.config.document_prefix, document)
    }

    fn validate(&self) -> Result<(), String> {
        if self.manifest_version != 1
            || self.config.schema_version != 1
            || self.index_schema_version != 1
        {
            return Err("unsupported embedding manifest schema".into());
        }
        if self.index_key != "jina_v1"
            || self.dimension != 768
            || self.distance_metric != "COSINE"
            || !self.normalized
        {
            return Err("unsupported embedding index contract".into());
        }
        if self.model_name != "jinaai/jina-embeddings-v5-text-nano-retrieval-GGUF"
            || self.model_revision != "59cfaceeeb7d738c404659435af4c0da74d06c96"
            || self.model_file_sha256
                != "86b6e6279e9b9e71389f02a082764a2ac2b15a50e37482c26f98d69092f12442"
            || self.config.model_file != "v5-nano-retrieval-Q8_0.gguf"
            || self.config.model_file_size != 232_883_776
        {
            return Err("unexpected embedding model artifact".into());
        }
        if self.config.quantization != "Q8_0"
            || self.config.pooling != "last"
            || self.config.normalization != "L2"
            || self.config.query_prefix != "Query: "
            || self.config.document_prefix != "Document: "
            || self.config.document_construction_version != 1
        {
            return Err("unexpected embedding inference contract".into());
        }
        Ok(())
    }
}

pub(crate) fn jina_v1_manifest() -> TextEmbeddingManifest {
    let manifest: TextEmbeddingManifest = serde_json::from_str(JINA_V1_MANIFEST_JSON)
        .expect("embedded Jina v1 manifest must be valid");
    manifest
        .validate()
        .expect("embedded Jina v1 manifest must match Kosh's supported contract");
    manifest
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::{
        jina_v1_manifest, JINA_V1_GOLDEN_JSON, JINA_V1_MANIFEST_JSON, LLAMA_SERVER_V1_PIN_JSON,
    };

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct GoldenFixtures {
        fixture_version: u32,
        model_file_sha256: String,
        generated_with: serde_json::Value,
        tolerance: serde_json::Value,
        cases: Vec<GoldenCase>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct GoldenCase {
        name: String,
        input: String,
        embedding: Vec<f32>,
    }

    #[test]
    fn pinned_manifest_and_golden_vectors_define_one_compatible_contract() {
        let manifest = jina_v1_manifest();
        let fixtures: GoldenFixtures =
            serde_json::from_str(JINA_V1_GOLDEN_JSON).expect("golden fixture JSON");

        assert_eq!(fixtures.fixture_version, 1);
        assert_eq!(fixtures.model_file_sha256, manifest.model_file_sha256);
        assert_eq!(fixtures.generated_with["runtime"], "llama.cpp");
        assert_eq!(
            fixtures.tolerance["minimumCosineSimilarity"],
            serde_json::json!(0.9998)
        );
        assert_eq!(
            fixtures
                .cases
                .iter()
                .map(|fixture| fixture.name.as_str())
                .collect::<Vec<_>>(),
            vec!["query", "document"]
        );
        assert_eq!(
            fixtures.cases[0].input,
            manifest.query_input("Why does spaced repetition improve long-term memory?")
        );
        assert_eq!(
            fixtures.cases[1].input,
            manifest.document_input(
                "Spaced repetition improves long-term memory by reviewing information at increasing intervals."
            )
        );
        for fixture in fixtures.cases {
            assert_eq!(fixture.embedding.len(), manifest.dimension as usize);
            assert!(fixture.embedding.iter().all(|value| value.is_finite()));
            let norm = fixture
                .embedding
                .iter()
                .map(|value| f64::from(*value).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!((norm - 1.0).abs() < 0.000_01, "{norm}");
        }
    }

    #[test]
    fn manifests_are_canonical_json_and_sidecar_pin_is_universal_macos() {
        let manifest: serde_json::Value =
            serde_json::from_str(JINA_V1_MANIFEST_JSON).expect("manifest JSON");
        let sidecar: serde_json::Value =
            serde_json::from_str(LLAMA_SERVER_V1_PIN_JSON).expect("sidecar pin JSON");

        assert_eq!(manifest["indexKey"], "jina_v1");
        assert_eq!(sidecar["target"]["operatingSystem"], "macos");
        assert_eq!(
            sidecar["target"]["architectures"],
            serde_json::json!(["arm64", "x86_64"])
        );
        assert_eq!(sidecar["upstream"]["build"], 9860);
    }

    #[test]
    fn prefixes_preserve_input_bytes_and_vector_normalization_is_manifest_owned() {
        let manifest = jina_v1_manifest();

        assert_eq!(manifest.query_input("  café  "), "Query:   café  ");
        assert_eq!(
            manifest.document_input("line 1\r\nline 2"),
            "Document: line 1\r\nline 2"
        );
        assert_eq!(manifest.config.normalization, "L2");
    }
}
