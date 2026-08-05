use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{config::Config, http};

pub const RUBERT_TINY2_DIMENSIONS: usize = 312;
pub const CHAT_EMBEDDING_DIMENSIONS: usize = 768;

#[derive(Serialize)]
struct EmbedRequest<'a> {
    inputs: &'a str,
    truncate: bool,
}

#[derive(Serialize)]
struct LlamaEmbeddingsRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct LlamaEmbeddingData {
    index: usize,
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct LlamaEmbeddingsResponse {
    data: Vec<LlamaEmbeddingData>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum EmbedResponse {
    Single(Vec<f32>),
    Batch(Vec<Vec<f32>>),
}

pub async fn embed_text(config: &Config, text: &str) -> anyhow::Result<Vec<f32>> {
    let embedding = embed_text_at(
        &config.rag_embedding_url,
        config.rag_embedding_timeout_sec,
        text,
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

pub async fn embed_chat_query(config: &Config, text: &str) -> anyhow::Result<Vec<f32>> {
    embed_chat_query_at(
        &config.chat_retrieval_embedding_url,
        config.chat_retrieval_embedding_timeout_sec,
        &config.chat_retrieval_embedding_model,
        &config.chat_retrieval_embedding_query_prefix,
        text,
    )
    .await
}

pub async fn embed_chat_query_at(
    embedding_url: &str,
    timeout_sec: u64,
    model: &str,
    query_prefix: &str,
    text: &str,
) -> anyhow::Result<Vec<f32>> {
    let input = format!("{query_prefix}{text}");
    let mut embeddings =
        request_llama_embeddings(embedding_url, timeout_sec, model, vec![input]).await?;
    let embedding = embeddings
        .pop()
        .ok_or_else(|| anyhow::anyhow!("llama.cpp embedding response is empty"))?;
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
    let embeddings = request_llama_embeddings(
        &config.chat_retrieval_embedding_url,
        config.chat_retrieval_embedding_timeout_sec,
        &config.chat_retrieval_embedding_model,
        inputs,
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
    let mut embeddings = Vec::with_capacity(texts.len());
    for text in texts {
        embeddings.push(embed_chat_query(config, text).await?);
    }
    Ok(embeddings)
}

async fn request_llama_embeddings(
    embedding_url: &str,
    timeout_sec: u64,
    model: &str,
    inputs: Vec<String>,
) -> anyhow::Result<Vec<Vec<f32>>> {
    let response = http::client(Duration::from_secs(timeout_sec))?
        .post(format!(
            "{}/v1/embeddings",
            embedding_url.trim_end_matches('/')
        ))
        .json(&LlamaEmbeddingsRequest {
            model,
            input: &inputs,
        })
        .send()
        .await?
        .error_for_status()?
        .json::<LlamaEmbeddingsResponse>()
        .await?;

    if response.data.len() != inputs.len() {
        anyhow::bail!(
            "llama.cpp embedding response contains {} rows for {} inputs",
            response.data.len(),
            inputs.len()
        );
    }
    let mut embeddings = response
        .data
        .into_iter()
        .map(|row| (row.index, row.embedding))
        .collect::<Vec<_>>();
    embeddings.sort_by_key(|(index, _)| *index);
    if embeddings
        .iter()
        .enumerate()
        .any(|(expected, (actual, _))| *actual != expected)
    {
        anyhow::bail!("llama.cpp embedding response indexes are not contiguous");
    }
    Ok(embeddings
        .into_iter()
        .map(|(_, embedding)| embedding)
        .collect())
}

pub fn pgvector_literal(values: &[f32]) -> anyhow::Result<String> {
    validate_embedding_dimensions(values, RUBERT_TINY2_DIMENSIONS)?;
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
    validate_embedding_dimensions(values, RUBERT_TINY2_DIMENSIONS)
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
    fn pgvector_literal_requires_rubert_dimensions() {
        let error = pgvector_literal(&[0.1, 0.2]).unwrap_err();
        assert!(error.to_string().contains("expected 312"));
    }

    #[test]
    fn pgvector_literal_rejects_non_finite_values() {
        let mut values = vec![0.0; RUBERT_TINY2_DIMENSIONS];
        values[4] = f32::NAN;
        assert!(pgvector_literal(&values).is_err());
    }

    #[test]
    fn pgvector_literal_formats_valid_vector() {
        let values = vec![0.25; RUBERT_TINY2_DIMENSIONS];
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
