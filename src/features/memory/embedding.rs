use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{config::Config, http};

pub const EMBEDDINGGEMMA2_DIMENSIONS: usize = 512;
pub const CHAT_EMBEDDING_DIMENSIONS: usize = EMBEDDINGGEMMA2_DIMENSIONS;
pub const EMBEDDINGGEMMA2_MODEL_ID: &str =
    "onnx-community/embeddinggemma-2-ONNX@daa72c51243991dfcaf9f9137d2c573d8f7790c0:q4";

pub const EMBEDDINGGEMMA2_CLASSIFICATION_PREFIX: &str = "task: classification | query: ";
pub const EMBEDDINGGEMMA2_RAG_QUERY_PREFIX: &str = "task: search result | query: ";
pub const EMBEDDINGGEMMA2_RAG_DOCUMENT_PREFIX: &str = "title: none | text: ";

#[derive(Serialize)]
struct EmbedRequest<'a> {
    inputs: &'a str,
    truncate: bool,
}

#[derive(Serialize)]
struct EmbedImageRequest<'a> {
    image_base64: &'a str,
}

#[derive(Serialize)]
struct EmbedBatchRequest<'a> {
    inputs: &'a [String],
    truncate: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum EmbedResponse {
    Single(Vec<f32>),
    Batch(Vec<Vec<f32>>),
}

pub async fn embed_classification_text(config: &Config, text: &str) -> anyhow::Result<Vec<f32>> {
    embed_text_with_prefix(config, EMBEDDINGGEMMA2_CLASSIFICATION_PREFIX, text).await
}

pub async fn embed_rag_query(config: &Config, text: &str) -> anyhow::Result<Vec<f32>> {
    embed_text_with_prefix(config, EMBEDDINGGEMMA2_RAG_QUERY_PREFIX, text).await
}

pub async fn embed_rag_document(config: &Config, text: &str) -> anyhow::Result<Vec<f32>> {
    embed_text_with_prefix(config, EMBEDDINGGEMMA2_RAG_DOCUMENT_PREFIX, text).await
}

pub async fn embed_profile_image(config: &Config, image_base64: &str) -> anyhow::Result<Vec<f32>> {
    let embedding = http::client(Duration::from_secs(config.rag_embedding_timeout_sec))?
        .post(format!(
            "{}/embed-image",
            config.rag_embedding_url.trim_end_matches('/')
        ))
        .json(&EmbedImageRequest { image_base64 })
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<f32>>()
        .await?;
    validate_embedding(&embedding)?;
    Ok(embedding)
}

async fn embed_text_with_prefix(
    config: &Config,
    prefix: &str,
    text: &str,
) -> anyhow::Result<Vec<f32>> {
    let input = format!("{prefix}{text}");
    let embedding = embed_text_at(
        &config.rag_embedding_url,
        config.rag_embedding_timeout_sec,
        &input,
    )
    .await?;
    tracing::info!(
        model = %config.rag_embedding_model,
        dimensions = embedding.len(),
        "RAG embedding completed"
    );
    Ok(embedding)
}

pub async fn embed_text_at(
    embedding_url: &str,
    timeout_sec: u64,
    text: &str,
) -> anyhow::Result<Vec<f32>> {
    let started = Instant::now();
    let response = http::client(Duration::from_secs(timeout_sec))?
        .post(format!("{}/embed", embedding_url.trim_end_matches('/')))
        .json(&EmbedRequest {
            inputs: text,
            truncate: true,
        })
        .send()
        .await?
        .error_for_status()?
        .json::<EmbedResponse>()
        .await?;

    let embedding = match response {
        EmbedResponse::Single(values) => values,
        EmbedResponse::Batch(mut rows) if rows.len() == 1 => rows.remove(0),
        EmbedResponse::Batch(rows) => {
            anyhow::bail!(
                "embedding service returned {} rows for one input",
                rows.len()
            )
        }
    };
    validate_embedding(&embedding)?;
    tracing::debug!(
        dimensions = embedding.len(),
        latency_ms = started.elapsed().as_millis(),
        "query embedding completed"
    );
    Ok(embedding)
}

pub async fn embed_chat_query_at(
    embedding_url: &str,
    timeout_sec: u64,
    model: &str,
    query_prefix: &str,
    text: &str,
) -> anyhow::Result<Vec<f32>> {
    let input = format!("{query_prefix}{text}");
    anyhow::ensure!(
        model == EMBEDDINGGEMMA2_MODEL_ID,
        "chat retrieval embedding model must match the pinned EmbeddingGemma 2 encoder"
    );
    let embedding = embed_text_at(embedding_url, timeout_sec, &input).await?;
    validate_embedding_dimensions(&embedding, CHAT_EMBEDDING_DIMENSIONS)?;
    Ok(embedding)
}

pub async fn embed_chat_documents_batch(
    config: &Config,
    texts: &[&str],
) -> anyhow::Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let inputs = texts
        .iter()
        .map(|text| format!("{}{text}", config.chat_retrieval_embedding_document_prefix))
        .collect::<Vec<_>>();
    anyhow::ensure!(
        config.chat_retrieval_embedding_model == EMBEDDINGGEMMA2_MODEL_ID,
        "chat retrieval embedding model must match the pinned EmbeddingGemma 2 encoder"
    );
    let embeddings = embed_texts_at(
        &config.chat_retrieval_embedding_url,
        config.chat_retrieval_embedding_timeout_sec,
        &inputs,
    )
    .await?;
    for embedding in &embeddings {
        validate_embedding_dimensions(embedding, CHAT_EMBEDDING_DIMENSIONS)?;
    }
    Ok(embeddings)
}

pub async fn embed_chat_queries_batch(
    config: &Config,
    texts: &[&str],
) -> anyhow::Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    anyhow::ensure!(
        config.chat_retrieval_embedding_model == EMBEDDINGGEMMA2_MODEL_ID,
        "chat retrieval embedding model must match the pinned EmbeddingGemma 2 encoder"
    );
    let inputs = texts
        .iter()
        .map(|text| format!("{}{text}", config.chat_retrieval_embedding_query_prefix))
        .collect::<Vec<_>>();
    let embeddings = embed_texts_at(
        &config.chat_retrieval_embedding_url,
        config.chat_retrieval_embedding_timeout_sec,
        &inputs,
    )
    .await?;
    for embedding in &embeddings {
        validate_embedding_dimensions(embedding, CHAT_EMBEDDING_DIMENSIONS)?;
    }
    Ok(embeddings)
}

pub async fn embed_texts_at(
    embedding_url: &str,
    timeout_sec: u64,
    inputs: &[String],
) -> anyhow::Result<Vec<Vec<f32>>> {
    anyhow::ensure!(!inputs.is_empty(), "embedding batch cannot be empty");
    let response = http::client(Duration::from_secs(timeout_sec))?
        .post(format!("{}/embed", embedding_url.trim_end_matches('/')))
        .json(&EmbedBatchRequest {
            inputs,
            truncate: true,
        })
        .send()
        .await?
        .error_for_status()?
        .json::<EmbedResponse>()
        .await?;

    let embeddings = match response {
        EmbedResponse::Single(values) if inputs.len() == 1 => vec![values],
        EmbedResponse::Batch(values) => values,
        EmbedResponse::Single(values) => vec![values],
    };
    anyhow::ensure!(
        embeddings.len() == inputs.len(),
        "embedding service returned {} rows for {} inputs",
        embeddings.len(),
        inputs.len()
    );
    for embedding in &embeddings {
        validate_embedding(embedding)?;
    }
    Ok(embeddings)
}

pub fn pgvector_literal(values: &[f32]) -> anyhow::Result<String> {
    validate_embedding_dimensions(values, EMBEDDINGGEMMA2_DIMENSIONS)?;
    pgvector_literal_unchecked(values)
}

pub fn pgvector_literal_for_dimensions(
    values: &[f32],
    dimensions: usize,
) -> anyhow::Result<String> {
    validate_embedding_dimensions(values, dimensions)?;
    pgvector_literal_unchecked(values)
}

fn pgvector_literal_unchecked(values: &[f32]) -> anyhow::Result<String> {
    let body = values
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!("[{body}]"))
}

fn validate_embedding(values: &[f32]) -> anyhow::Result<()> {
    validate_embedding_dimensions(values, EMBEDDINGGEMMA2_DIMENSIONS)
}

fn validate_embedding_dimensions(values: &[f32], dimensions: usize) -> anyhow::Result<()> {
    if values.len() != dimensions {
        anyhow::bail!(
            "unexpected embedding dimensions: expected {}, got {}",
            dimensions,
            values.len()
        );
    }
    if values.iter().any(|value| !value.is_finite()) {
        anyhow::bail!("embedding contains a non-finite value");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pgvector_literal_requires_embeddinggemma2_dimensions() {
        let error = pgvector_literal(&[0.1, 0.2]).unwrap_err();
        assert!(error.to_string().contains("expected 512"));
    }

    #[test]
    fn pgvector_literal_rejects_non_finite_values() {
        let mut values = vec![0.0; EMBEDDINGGEMMA2_DIMENSIONS];
        values[4] = f32::NAN;
        assert!(pgvector_literal(&values).is_err());
    }

    #[test]
    fn pgvector_literal_formats_valid_vector() {
        let values = vec![0.25; EMBEDDINGGEMMA2_DIMENSIONS];
        let literal = pgvector_literal(&values).unwrap();
        assert!(literal.starts_with("[0.25,0.25"));
        assert!(literal.ends_with(']'));
    }

    #[test]
    fn chat_pgvector_literal_accepts_full_embedding_dimensions() {
        let literal = pgvector_literal_for_dimensions(
            &vec![0.25; CHAT_EMBEDDING_DIMENSIONS],
            CHAT_EMBEDDING_DIMENSIONS,
        )
        .unwrap();
        assert!(literal.starts_with("[0.25,0.25"));
        assert!(literal.ends_with(']'));
    }
}
