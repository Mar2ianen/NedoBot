use crate::config::Config;
use crate::features::voice::types::{CleanTranscript, TranscriptChapter, TranscriptRenderMode};
use crate::telegram::html::{self, Html};

pub enum RenderedTranscript {
    Message {
        html: String,
    },
    RichMessage {
        html: String,
        fallback: Box<RenderedTranscript>,
    },
    MessageAndFile {
        html: String,
        filename: String,
        body: String,
    },
}

pub fn render_transcript(clean: &CleanTranscript, config: &Config) -> RenderedTranscript {
    if clean.mode == TranscriptRenderMode::Short
        || clean.text.chars().count() <= config.voice_short_text_max_chars
    {
        return render_plain_text(&clean.text, config);
    }

    let chapters = effective_chapters(clean);
    let html = render_chapters(&chapters, config.voice_render_expandable_chapters);
    if html::is_safe_len(&html) {
        return RenderedTranscript::Message { html };
    }

    RenderedTranscript::RichMessage {
        html: render_rich_chapters(&chapters),
        fallback: Box::new(render_file_fallback(
            render_preview(clean, &chapters, config.voice_render_expandable_chapters),
            render_file_body(clean, &chapters),
            config,
        )),
    }
}

fn render_plain_text(text: &str, config: &Config) -> RenderedTranscript {
    let html = Html::text(text).into_string();
    if html::is_safe_len(&html) {
        return RenderedTranscript::Message { html };
    }

    let preview_text = format!(
        "{}\n\nПолная расшифровка в файле.",
        html::truncate_text(text, 1200)
    );
    RenderedTranscript::RichMessage {
        html: render_rich_text(text),
        fallback: Box::new(render_file_fallback(
            Html::text(preview_text).into_string(),
            text.to_string(),
            config,
        )),
    }
}

fn render_file_fallback(html: String, body: String, config: &Config) -> RenderedTranscript {
    if config.voice_send_full_file {
        RenderedTranscript::MessageAndFile {
            html,
            filename: "voice-transcript.txt".to_string(),
            body,
        }
    } else {
        RenderedTranscript::Message { html }
    }
}

fn effective_chapters(clean: &CleanTranscript) -> Vec<TranscriptChapter> {
    if !clean.chapters.is_empty() {
        return clean.chapters.clone();
    }

    vec![TranscriptChapter {
        title: "Текст".to_string(),
        start_sec: 0.0,
        end_sec: None,
        text: clean.text.clone(),
    }]
}

fn render_chapters(chapters: &[TranscriptChapter], expandable: bool) -> String {
    let mut out = Html::empty();
    out.line(Html::bold("Расшифровка голосового"));
    for chapter in chapters {
        out.blank_line();
        let mut title = Html::empty();
        title.push(Html::bold(&chapter.title));
        out.line(title);
        if expandable {
            out.line(html::expandable_blockquote(&chapter.text));
        } else {
            out.line(Html::text(&chapter.text));
        }
    }
    out.into_string()
}

fn render_rich_text(text: &str) -> String {
    format!(
        "<h3>Расшифровка голосового</h3><details><summary>Полная расшифровка</summary><p>{}</p></details>",
        escape_rich_text(text)
    )
}

fn render_rich_chapters(chapters: &[TranscriptChapter]) -> String {
    let mut html = String::from("<h3>Расшифровка голосового</h3>");
    for chapter in chapters {
        html.push_str("<details><summary>");
        html.push_str(&crate::telegram::html::escape(&chapter.title));
        html.push_str("</summary><p>");
        html.push_str(&escape_rich_text(&chapter.text));
        html.push_str("</p></details>");
    }
    html
}

fn escape_rich_text(text: &str) -> String {
    crate::telegram::html::escape(text).replace('\n', "<br>")
}

fn render_preview(
    clean: &CleanTranscript,
    chapters: &[TranscriptChapter],
    expandable: bool,
) -> String {
    let mut preview = Html::empty();
    preview.line(Html::bold("Расшифровка голосового"));
    if let Some(summary) = clean
        .short_summary
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        preview.blank_line();
        preview.line(Html::text(summary));
    }

    let mut used = preview.as_str().chars().count();
    for chapter in chapters.iter().take(3) {
        let body = html::truncate_text(&chapter.text, 500);
        let chunk = render_one_chapter(chapter, &body, expandable);
        let projected = used + chunk.chars().count() + 2;
        if projected > html::SAFE_TEXT_LIMIT {
            break;
        }
        preview.blank_line();
        preview.push(Html::raw_trusted(chunk));
        used = projected;
    }

    preview.blank_line();
    preview.line(Html::text("Полная расшифровка в файле."));
    preview.into_string()
}

fn render_one_chapter(chapter: &TranscriptChapter, body: &str, expandable: bool) -> String {
    let mut out = Html::empty();
    let mut title = Html::empty();
    title.push(Html::bold(&chapter.title));
    out.line(title);
    if expandable {
        out.line(html::expandable_blockquote(body));
    } else {
        out.line(Html::text(body));
    }
    out.into_string()
}

fn render_file_body(clean: &CleanTranscript, chapters: &[TranscriptChapter]) -> String {
    let mut body = String::from("Расшифровка голосового\n\n");
    if let Some(summary) = clean
        .short_summary
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        body.push_str(summary);
        body.push_str("\n\n");
    }
    for chapter in chapters {
        body.push_str(&format!("{}\n{}\n\n", chapter.title, chapter.text));
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SearchMcpTools;

    fn config() -> Config {
        Config {
            source_channel_id: -1001,
            discussion_chat_id: -1002,
            chat_invite_url: "https://t.me/example".to_string(),
            chat_invite_label: "чат".to_string(),
            post_signature_marker: "marker".to_string(),
            llm_provider: "ollama".to_string(),
            llm_model: None,
            llm_supports_images: None,
            llm_temperature: 0.45,
            llm_max_tokens: 140,
            llm_proxy_url: None,
            memory_llm_temperature: 0.2,
            memory_llm_max_tokens: 220,
            search_enabled: false,
            search_extract_provider: Some("ollama".to_string()),
            search_extract_model: Some("gemma4:31b".to_string()),
            search_extract_temperature: 0.1,
            search_extract_max_tokens: 700,
            search_mcp_command: None,
            search_mcp_args: Vec::new(),
            search_mcp_env: Vec::new(),
            search_mcp_timeout_sec: 8,
            search_mcp_tools: SearchMcpTools {
                web: "web_search".to_string(),
                github: "github_search".to_string(),
                reddit: "reddit_search".to_string(),
            },
            search_mcp_fetch_tool: Some("web_fetch_exa".to_string()),
            search_fetch_top_n: 2,
            search_fetch_max_chars: 6000,
            search_github_mcp_command: None,
            search_github_mcp_args: Vec::new(),
            search_github_mcp_env: vec![
                "PATH".to_string(),
                "HOME".to_string(),
                "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
            ],
            search_github_mcp_tools: vec!["search_issues".to_string(), "search_code".to_string()],
            groq_api_key: String::new(),
            groq_model: None,
            cerebras_api_key: String::new(),
            cerebras_model: None,
            openrouter_api_key: String::new(),
            openrouter_model: None,
            gemini_api_key: String::new(),
            gemini_text_model: "gemini-3.5-flash".to_string(),
            gemini_flash_model: "gemini-3.1-flash-lite".to_string(),
            gemini_tts_model: "gemini-3.1-flash-tts-preview".to_string(),
            gemini_thinking_budget: 1024,
            ollama_base_url: "http://localhost:11434".to_string(),
            ollama_api_key: String::new(),
            openai_compat_base_url: "https://api.openai.com/v1".to_string(),
            openai_compat_api_key: String::new(),
            openai_compat_model: None,
            vision_model: "gemma4:31b".to_string(),
            owner_telegram_id: None,
            send_owner_preview: false,
            profile_refresh_concurrency: 4,
            comment_custom_emoji_id: None,
            first_comment_max_image_mb: 10,
            tech_custom_emoji_id: None,
            amd_custom_emoji_id: None,
            radeon_custom_emoji_id: None,
            ryzen_custom_emoji_id: None,
            voice_transcription_enabled: true,
            voice_auto_transcribe: true,
            voice_max_duration_sec: 600,
            voice_max_file_mb: 20,
            voice_short_text_max_chars: 400,
            voice_language: "ru".to_string(),
            voice_asr_provider: "groq".to_string(),
            voice_asr_model: "whisper-large-v3-turbo".to_string(),
            voice_asr_temperature: 0.0,
            voice_cleanup_provider: None,
            voice_cleanup_model: None,
            voice_cleanup_temperature: 0.2,
            voice_cleanup_max_tokens: 1800,
            voice_render_expandable_chapters: true,
            voice_send_full_file: true,
            public_base_url: None,
            static_files_dir: "/tmp/tg-ai-bot-static".to_string(),
        }
    }

    #[test]
    fn short_transcript_renders_plain_text() {
        let clean = CleanTranscript {
            mode: TranscriptRenderMode::Short,
            text: "<hello>".to_string(),
            chapters: Vec::new(),
            short_summary: None,
        };
        let RenderedTranscript::Message { html } = render_transcript(&clean, &config()) else {
            panic!("short transcript must render as a regular message");
        };

        assert_eq!(html, "&lt;hello&gt;");
    }

    #[test]
    fn long_single_topic_uses_closed_rich_details_with_file_fallback() {
        let clean = CleanTranscript {
            mode: TranscriptRenderMode::Short,
            text: "<голосовое>\n".repeat(500),
            chapters: Vec::new(),
            short_summary: None,
        };

        let rendered = render_transcript(&clean, &config());
        let RenderedTranscript::RichMessage { html, fallback } = rendered else {
            panic!("long transcript must use a rich message");
        };

        assert!(html.contains("<details><summary>Полная расшифровка</summary>"));
        assert!(!html.contains("<details open>"));
        assert!(html.contains("&lt;голосовое&gt;"));
        assert!(matches!(
            *fallback,
            RenderedTranscript::MessageAndFile { .. }
        ));
    }

    #[test]
    fn long_chapters_use_closed_rich_details() {
        let clean = CleanTranscript {
            mode: TranscriptRenderMode::Chapters,
            text: "длинная расшифровка ".repeat(500),
            chapters: vec![
                TranscriptChapter {
                    title: "Первая <тема>".to_string(),
                    start_sec: 0.0,
                    end_sec: Some(60.0),
                    text: "текст ".repeat(700),
                },
                TranscriptChapter {
                    title: "Вторая тема".to_string(),
                    start_sec: 60.0,
                    end_sec: None,
                    text: "ещё текст".to_string(),
                },
            ],
            short_summary: None,
        };

        let rendered = render_transcript(&clean, &config());
        let RenderedTranscript::RichMessage { html, .. } = rendered else {
            panic!("long chapters must use a rich message");
        };

        assert_eq!(html.matches("<details>").count(), 2);
        assert!(html.contains("Первая &lt;тема&gt;"));
        assert!(!html.contains("<details open>"));
    }
}
