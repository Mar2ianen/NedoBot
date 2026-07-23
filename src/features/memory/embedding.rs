use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{config::Config, http};

pub const RUBERT_TINY2_DIMENSIONS: usize = 312;

#[derive(Serialize)]
struct EmbedRequest<'a> {
    inputs: &'a str,
    truncate: bool,
}

#[derive(Serialize)]
struct EmbedBatchRequest<'a> {
    inputs: &'a [&'a str],
    truncate: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum EmbedResponse {
    Single(Vec<f32>),
    Batch(Vec<Vec<f32>>),
}

pub async fn embed_text(config: &Config, text: &str) -> anyhow::Result<Vec<f32>> {
    let started = Instant::now();
    let response = http::client(Duration::from_secs(config.rag_embedding_timeout_sec))?
        .post(format!(
            "{}/embed",
            config.rag_embedding_url.trim_end_matches('/')
        ))
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
    tracing::info!(
        model = %config.rag_embedding_model,
        dimensions = embedding.len(),
        latency_ms = started.elapsed().as_millis(),
        "RAG embedding completed"
    );
    Ok(embedding)
}

pub async fn embed_text_batch(config: &Config, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let started = Instant::now();
    let response = http::client(Duration::from_secs(config.rag_embedding_timeout_sec))?
        .post(format!(
            "{}/embed",
            config.rag_embedding_url.trim_end_matches('/')
        ))
        .json(&EmbedBatchRequest {
            inputs: texts,
            truncate: true,
        })
        .send()
        .await?
        .error_for_status()?
        .json::<EmbedResponse>()
        .await?;

    let embeddings = match response {
        EmbedResponse::Batch(rows) if rows.len() == texts.len() => rows,
        EmbedResponse::Batch(rows) => {
            anyhow::bail!(
                "embedding service returned {} rows for {} inputs",
                rows.len(),
                texts.len()
            )
        }
        EmbedResponse::Single(_) => {
            anyhow::bail!("embedding service returned one row for batch input")
        }
    };
    for embedding in &embeddings {
        validate_embedding(embedding)?;
    }
    tracing::info!(
        model = %config.rag_embedding_model,
        inputs = texts.len(),
        latency_ms = started.elapsed().as_millis(),
        "RAG embedding batch completed"
    );
    Ok(embeddings)
}

pub fn pgvector_literal(values: &[f32]) -> anyhow::Result<String> {
    validate_embedding(values)?;
    let body = values
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!("[{body}]"))
}

fn validate_embedding(values: &[f32]) -> anyhow::Result<()> {
    if values.len() != RUBERT_TINY2_DIMENSIONS {
        anyhow::bail!(
            "unexpected embedding dimensions: expected {}, got {}",
            RUBERT_TINY2_DIMENSIONS,
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
}
