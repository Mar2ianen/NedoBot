use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct EmbedBatchRequest<'a> {
    inputs: &'a [&'a str],
    truncate: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum EmbedBatchResponse {
    Single(Vec<f32>),
    Batch(Vec<Vec<f32>>),
}

pub async fn embed_text_batch_at(
    embedding_url: &str,
    timeout_sec: u64,
    inputs: &[&str],
) -> anyhow::Result<Vec<Vec<f32>>> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    let response = tg_ai_bot_teloxide::http::client(Duration::from_secs(timeout_sec))?
        .post(format!("{}/embed", embedding_url.trim_end_matches('/')))
        .json(&EmbedBatchRequest {
            inputs,
            truncate: true,
        })
        .send()
        .await?
        .error_for_status()?
        .json::<EmbedBatchResponse>()
        .await?;
    match response {
        EmbedBatchResponse::Single(embedding) if inputs.len() == 1 => Ok(vec![embedding]),
        EmbedBatchResponse::Single(_) => {
            anyhow::bail!("embedding service returned one row for a batch input")
        }
        EmbedBatchResponse::Batch(embeddings) if embeddings.len() == inputs.len() => Ok(embeddings),
        EmbedBatchResponse::Batch(embeddings) => anyhow::bail!(
            "embedding service returned {} rows for {} inputs",
            embeddings.len(),
            inputs.len()
        ),
    }
}
