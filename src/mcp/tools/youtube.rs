//! YouTube subtitle extraction for MCP clients.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::features::search::youtube::{
    YoutubeSubtitlesConfig, get_youtube_subtitles, is_youtube_video_url,
};

use super::{invalid_arguments, read_error};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetSubtitlesInput {
    /// A public YouTube video URL, such as https://www.youtube.com/watch?v=...
    pub url: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetSubtitlesOutput {
    pub url: String,
    pub available: bool,
    pub language: Option<String>,
    pub text: Option<String>,
}

pub(crate) async fn get_subtitles(
    config: &YoutubeSubtitlesConfig,
    input: GetSubtitlesInput,
) -> Result<GetSubtitlesOutput, rmcp::ErrorData> {
    let url = input.url.trim().to_string();
    if !is_youtube_video_url(&url) {
        return Err(invalid_arguments("url must be a public YouTube video URL"));
    }
    if !config.enabled {
        return Err(read_error("YouTube subtitles are disabled"));
    }

    let transcript = get_youtube_subtitles(config, &url).await.map_err(|error| {
        tracing::warn!(%error, "MCP YouTube subtitle extraction failed");
        read_error("YouTube subtitle extraction failed")
    })?;

    Ok(match transcript {
        Some(transcript) => GetSubtitlesOutput {
            url,
            available: true,
            language: Some(if transcript.language == "unknown" {
                "auto".to_string()
            } else {
                transcript.language
            }),
            text: Some(transcript.text),
        },
        None => GetSubtitlesOutput {
            url,
            available: false,
            language: None,
            text: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_non_youtube_urls_without_running_extractor() {
        let error = get_subtitles(
            &YoutubeSubtitlesConfig::disabled(),
            GetSubtitlesInput {
                url: "https://example.com/video".to_string(),
            },
        )
        .await
        .expect_err("non-YouTube URL must be rejected");

        assert_eq!(error.message, "url must be a public YouTube video URL");
    }

    #[tokio::test]
    async fn disabled_extractor_does_not_spawn_process() {
        let error = get_subtitles(
            &YoutubeSubtitlesConfig::disabled(),
            GetSubtitlesInput {
                url: "https://www.youtube.com/watch?v=abc123".to_string(),
            },
        )
        .await
        .expect_err("disabled extractor must return an MCP error");

        assert_eq!(error.message, "YouTube subtitles are disabled");
    }
}
