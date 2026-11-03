use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::Duration;
const CHUNK_CHARS: usize = 1800;
const CHUNK_OVERLAP: usize = 200;
#[derive(Clone)]
pub struct EmbeddingClient {
    pub name: String,
    pub key: String,
    pub min_similarity: f64,
    query_prefix: String,
    document_prefix: String,
    provider: rig::providers::openai::OpenAI,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Deserialize)]
struct EmbeddingItem {
    index: usize,
    embedding: Vec<f32>,
}

fn validate_response(response: EmbeddingResponse, count: usize) -> Result<Vec<Vec<f32>>> {
    ensure!(
        response.data.len() == count,
        "Embedding response count mismatch"
    );
    let mut ordered = vec![None; count];
    let mut dimensions = None;
    for item in response.data {
        ensure!(item.index < count, "Embedding response index out of range");
        ensure!(
            ordered[item.index].is_none(),
            "Duplicate embedding response index"
        );
        ensure!(
            (1..=16000).contains(&item.embedding.len()),
            "Invalid embedding dimensions"
        );
        ensure!(
            item.embedding.iter().all(|v| v.is_finite()),
            "Non-finite embedding value"
        );
        ensure!(
            item.embedding.iter().any(|v| *v != 0.0),
            "Zero embedding vector"
        );
        let expected = dimensions.get_or_insert(item.embedding.len());
        ensure!(
            *expected == item.embedding.len(),
            "Inconsistent embedding dimensions"
        );
        ordered[item.index] = Some(item.embedding);
    }
    ordered
        .into_iter()
        .map(|v| v.context("Missing embedding response index"))
        .collect()
}

// Bound requests without dropping the end of long provisions or splitting UTF-8.
fn chunks(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut result = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + CHUNK_CHARS).min(chars.len());
        result.push(chars[start..end].iter().collect());
        if end == chars.len() {
            break;
        }
        start = end - CHUNK_OVERLAP;
    }
    result
}

impl EmbeddingClient {
    pub fn document_inputs(&self, text: &str) -> Vec<String> {
        chunks(text)
            .into_iter()
            .map(|chunk| format!("{}{chunk}", self.document_prefix))
            .collect()
    }

    pub fn from_env() -> Result<Option<Self>> {
        let base_url = std::env::var("EMBEDDING_BASE_URL").unwrap_or_default();
        let name = std::env::var("EMBEDDING_MODEL").unwrap_or_default();
        if base_url.is_empty() && name.is_empty() {
            return Ok(None);
        }
        ensure!(
            !base_url.is_empty() && !name.is_empty(),
            "Set both EMBEDDING_BASE_URL and EMBEDDING_MODEL"
        );
        let url = reqwest::Url::parse(&base_url).context("Invalid embedding endpoint")?;
        ensure!(
            matches!(url.scheme(), "http" | "https"),
            "Embedding endpoint must be HTTP(S)"
        );
        ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "Use EMBEDDING_API_KEY for credentials; endpoint must have no query or fragment"
        );
        let endpoint = format!("{}/embeddings", base_url.trim_end_matches('/'));
        let query_prefix = std::env::var("EMBEDDING_QUERY_PREFIX").unwrap_or_default();
        let document_prefix = std::env::var("EMBEDDING_DOCUMENT_PREFIX").unwrap_or_default();
        let revision = std::env::var("EMBEDDING_REVISION").unwrap_or_default();
        let min_similarity = std::env::var("EMBEDDING_MIN_SIMILARITY")
            .unwrap_or_else(|_| "0.35".into())
            .parse::<f64>()
            .context("EMBEDDING_MIN_SIMILARITY must be a number")?;
        ensure!(
            (-1.0..=1.0).contains(&min_similarity),
            "EMBEDDING_MIN_SIMILARITY must be between -1 and 1"
        );
        // API key rotation preserves the index; a model/revision/prefix change
        // creates an independent vector space, even if dimensions are identical.
        let identity = json!([
            "legal-context-v1",
            endpoint,
            name,
            revision,
            query_prefix,
            document_prefix,
            CHUNK_CHARS,
            CHUNK_OVERLAP
        ]);
        let key = format!("{:x}", Sha256::digest(identity.to_string().as_bytes()));
        Ok(Some(Self {
            name,
            key,
            query_prefix,
            document_prefix,
            min_similarity,
            provider: super::http_connection::provider(
                &base_url,
                &std::env::var("EMBEDDING_API_KEY").unwrap_or_default(),
                Duration::from_secs(120),
            )?,
        }))
    }

    pub(crate) async fn embed(&self, input: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut model = self.provider.embedding(&self.name, None);
        // Compatible local providers may omit billing metadata. Vector indices
        // and dimensions are still checked below, independently of usage.
        model.wire.provider.dialect.quirks.embedding.requires_usage = false;
        model.wire = model
            .wire
            .with_encoding_format(rig::providers::openai::EncodingFormat::Float);
        let response = model
            .call(input.to_vec())
            .await
            .map_err(super::http_connection::provider_failure)?;
        // Rig owns transport and decoding. Retain our stricter index, dimension
        // and finite/nonzero-vector checks before any vector reaches storage.
        let response = serde_json::from_value::<EmbeddingResponse>(response.raw)
            .map_err(|_| anyhow::anyhow!("Invalid embedding response"))?;
        validate_response(response, input.len())
    }

    pub async fn query_vector(&self, text: &str, dimensions: i32) -> Result<String> {
        let input = vec![format!("{}{text}", self.query_prefix)];
        // The lexical path stays responsive when the provider is unavailable.
        let vectors = tokio::time::timeout(Duration::from_secs(5), self.embed(&input))
            .await
            .context("Embedding query timed out")??;
        ensure!(
            vectors[0].len() == dimensions as usize,
            "Embedding model dimensions changed; set a new EMBEDDING_REVISION and reindex"
        );
        Ok(serde_json::to_string(&vectors[0])?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_provider_indices_dimensions_and_vectors() {
        let response =
            |data| serde_json::from_value::<EmbeddingResponse>(json!({"data":data})).unwrap();
        let vectors = validate_response(
            response(json!([
                {"index":1,"embedding":[0.0,1.0]}, {"index":0,"embedding":[1.0,0.0]}
            ])),
            2,
        )
        .unwrap();
        assert_eq!(vectors[0], vec![1.0, 0.0]);
        for data in [
            json!([]),
            json!([{"index":0,"embedding":[0.0,0.0]}]),
            json!([{"index":1,"embedding":[1.0,0.0]}]),
            json!([{"index":0,"embedding":[]}]),
        ] {
            assert!(validate_response(response(data), 1).is_err());
        }
        for data in [
            json!([{"index":0,"embedding":[1.0]},{"index":0,"embedding":[1.0]}]),
            json!([{"index":0,"embedding":[1.0]},{"index":1,"embedding":[1.0,0.0]}]),
        ] {
            assert!(validate_response(response(data), 2).is_err());
        }
        assert!(
            validate_response(
                EmbeddingResponse {
                    data: vec![EmbeddingItem {
                        index: 0,
                        embedding: vec![f32::NAN]
                    }]
                },
                1
            )
            .is_err()
        );
    }

    #[test]
    fn long_slovak_text_is_covered_without_truncation() {
        let original = "Žaloba má účinky. ".repeat(500);
        let pieces = chunks(&original);
        assert!(pieces.len() > 1);
        let mut rebuilt = pieces[0].clone();
        for piece in &pieces[1..] {
            assert!(piece.chars().count() <= CHUNK_CHARS);
            rebuilt.extend(piece.chars().skip(CHUNK_OVERLAP));
        }
        assert_eq!(rebuilt, original);
    }
}
