use teloxide::{prelude::*, types::ParseMode};
use tg_ai_bot_teloxide::{
    config::Config,
    db::{build_pool, migrate},
    features::voice::{
        asr::transcribe_audio,
        cleanup::cleanup_transcript,
        download::download_media_file,
        types::{VoiceMedia, VoiceMediaKind},
    },
};

type VoiceJobRow = (
    i64,
    i32,
    Option<i64>,
    String,
    Option<String>,
    String,
    Option<i32>,
    Option<i64>,
    Option<String>,
);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    ensure_latest_argument()?;

    let config = Config::from_env()?;
    config.validate_runtime_secrets()?;
    let pool = build_pool().await?;
    migrate(&pool).await?;
    let media = load_latest_sent_voice(&pool).await?;
    let bot = Bot::from_env().parse_mode(ParseMode::Html);

    let downloaded = download_media_file(&bot, &media).await?;
    let transcript = transcribe_audio(
        &config,
        &downloaded.path,
        &downloaded.filename,
        downloaded.mime_type.as_deref(),
    )
    .await?;
    let cleanup = cleanup_transcript(&config, &transcript).await?;

    println!(
        "voice smoke passed: source_message_id={} media_kind={} asr={}/{} raw_chars={} cleanup={}/{} cleaned_chars={} chapters={}",
        media.message_id,
        media.kind.as_str(),
        transcript.provider,
        transcript.model,
        transcript.text.chars().count(),
        cleanup.provider,
        cleanup.model,
        cleanup.transcript.text.chars().count(),
        cleanup.transcript.chapters.len(),
    );
    Ok(())
}

fn ensure_latest_argument() -> anyhow::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--latest") && std::env::args().nth(2).is_none() {
        return Ok(());
    }

    anyhow::bail!("Usage: voice_smoke --latest")
}

async fn load_latest_sent_voice(pool: &sqlx::PgPool) -> anyhow::Result<VoiceMedia> {
    let row: Option<VoiceJobRow> = sqlx::query_as(
        r#"
        select chat_id, message_id, user_id, file_id, file_unique_id, media_kind,
               duration_sec, file_size, mime_type
        from voice_transcription_jobs
        where status = 'sent'
        order by id desc
        limit 1
        "#,
    )
    .fetch_optional(pool)
    .await?;
    let Some((
        chat_id,
        message_id,
        user_id,
        file_id,
        file_unique_id,
        media_kind,
        duration_sec,
        file_size,
        mime_type,
    )) = row
    else {
        anyhow::bail!("no sent voice transcription jobs found")
    };

    let kind = match media_kind.as_str() {
        "voice" => VoiceMediaKind::Voice,
        "audio" => VoiceMediaKind::Audio,
        "video_note" => VoiceMediaKind::VideoNote,
        _ => anyhow::bail!("latest voice job has unsupported media_kind"),
    };

    Ok(VoiceMedia {
        chat_id,
        message_id,
        user_id,
        kind,
        file_id,
        file_unique_id,
        duration_sec: duration_sec.map(|value| value as u32),
        file_size: file_size.map(|value| value as u64),
        mime_type,
    })
}
