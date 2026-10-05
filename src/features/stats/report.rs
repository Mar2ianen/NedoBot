use sqlx::PgPool;
use teloxide::prelude::*;
use teloxide::types::{
    FileId, InputFile, InputMediaPhoto, InputRichMessageMedia, InputRichMessageMediaContent,
};
use teloxide::utils::time::TimeContext;

use crate::config::Config;
use crate::features::stats::render_html;
use crate::features::stats::render_rich;
use crate::features::stats::service::{self, HTML_TOP_LIMIT, RICH_TOP_LIMIT};
use crate::features::stats::types::{StatsPeriod, StatsRender};
use crate::telegram::render::{send_html, send_rich_html, send_rich_html_with_media};

/// Transport wiring for stats commands. Data is assembled in `service`; output is
/// formatted in the selected renderer. Neither renderer has database access.
pub async fn send_chat_stats(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
    time_context: &TimeContext,
    period: StatsPeriod,
    render: StatsRender,
) -> ResponseResult<()> {
    let data = service::chat_stats_report_data(pool, config, period)
        .await
        .map_err(stats_error("failed to build chat stats"))?;
    let report = match render {
        StatsRender::Html => render_html::chat_stats(&data, time_context),
        StatsRender::Rich => {
            render_rich::chat_stats(&data, config.discussion_chat_id, time_context)
        }
    };
    send_stats_report(bot, chat_id, report, render).await?;
    Ok(())
}

pub async fn send_top_messages(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
    render: StatsRender,
) -> ResponseResult<()> {
    service::refresh_top_message_users(bot, pool, config).await;
    let limit = match render {
        StatsRender::Html => HTML_TOP_LIMIT,
        StatsRender::Rich => RICH_TOP_LIMIT,
    };
    let data = service::top_messages_report_data(pool, config, limit)
        .await
        .map_err(stats_error("failed to build top messages report"))?;
    let report = match render {
        StatsRender::Html => render_html::top_messages(&data),
        StatsRender::Rich => render_rich::top_messages(&data),
    };
    send_stats_report(bot, chat_id, report, render).await?;
    Ok(())
}

pub async fn send_top_reacted(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
    render: StatsRender,
) -> ResponseResult<()> {
    service::refresh_top_reacted_users(bot, pool, config).await;
    let limit = match render {
        StatsRender::Html => HTML_TOP_LIMIT,
        StatsRender::Rich => RICH_TOP_LIMIT,
    };
    let data = service::top_reacted_report_data(pool, config, limit)
        .await
        .map_err(stats_error("failed to build top reacted report"))?;
    let report = match render {
        StatsRender::Html => render_html::top_reacted(&data, config.discussion_chat_id),
        StatsRender::Rich => render_rich::top_reacted(&data, config.discussion_chat_id),
    };
    send_stats_report(bot, chat_id, report, render).await?;
    Ok(())
}

pub async fn send_user_stats(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
    target: Option<&str>,
    reply_user_id: Option<i64>,
    render: StatsRender,
) -> ResponseResult<()> {
    if let Some(user_id) = numeric_target_user_id(target).or(reply_user_id) {
        service::refresh_user_profile(bot, pool, config, user_id).await;
    }
    let data = service::user_stats_report_data(pool, config, target, reply_user_id)
        .await
        .map_err(stats_error("failed to build user stats"))?;
    let avatar_file_id = (render == StatsRender::Rich)
        .then(|| {
            data.as_ref()
                .and_then(|data| data.profile_photo_file_id.clone())
        })
        .flatten();
    let report = match render {
        StatsRender::Html => {
            render_html::user_stats(data.as_ref(), target, config.discussion_chat_id)
        }
        StatsRender::Rich => render_rich::user_stats(
            data.as_ref(),
            target,
            config.discussion_chat_id,
            avatar_file_id
                .as_ref()
                .map(|_| render_rich::USER_STATS_AVATAR_MEDIA_ID),
        ),
    };
    if let Some(file_id) = avatar_file_id {
        send_rich_html_with_media(bot, chat_id, report, [user_stats_avatar_media(file_id)]).await?;
    } else {
        send_stats_report(bot, chat_id, report, render).await?;
    }
    Ok(())
}

async fn send_stats_report(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    report: String,
    render: StatsRender,
) -> ResponseResult<Message> {
    match render {
        StatsRender::Html => send_html(bot, chat_id, report).await,
        StatsRender::Rich => send_rich_html(bot, chat_id, report).await,
    }
}

fn stats_error(message: &'static str) -> impl FnOnce(anyhow::Error) -> teloxide::RequestError {
    move |err| {
        tracing::error!(%err, "{message}");
        teloxide::RequestError::Io(std::io::Error::other("stats failed").into())
    }
}

fn numeric_target_user_id(target: Option<&str>) -> Option<i64> {
    target?.parse().ok()
}

fn user_stats_avatar_media(file_id: String) -> InputRichMessageMedia {
    InputRichMessageMedia::new(
        render_rich::USER_STATS_AVATAR_MEDIA_ID,
        InputRichMessageMediaContent::Photo(InputMediaPhoto::new(InputFile::file_id(FileId(
            file_id,
        )))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::stats::render_html::message_preview;
    use crate::features::stats::types::MessageMediaPreview;

    #[test]
    fn message_preview_falls_back_to_media() {
        assert_eq!(
            message_preview(
                None,
                MessageMediaPreview {
                    has_voice: true,
                    ..Default::default()
                }
            ),
            "медиа: голосовое"
        );
    }

    #[test]
    fn user_stats_avatar_media_matches_the_html_reference() {
        let media = user_stats_avatar_media("avatar-file-id".to_owned());
        let message = teloxide::types::InputRichMessage::html(format!(
            "<img src=\"tg://photo?id={}\">",
            render_rich::USER_STATS_AVATAR_MEDIA_ID
        ))
        .media([media]);
        let value = serde_json::to_value(message).expect("serialize rich user stats message");

        assert_eq!(
            value["html"],
            format!(
                "<img src=\"tg://photo?id={}\">",
                render_rich::USER_STATS_AVATAR_MEDIA_ID
            )
        );
        assert_eq!(
            value["media"][0]["id"],
            render_rich::USER_STATS_AVATAR_MEDIA_ID
        );
        assert_eq!(value["media"][0]["media"]["type"], "photo");
        assert_eq!(value["media"][0]["media"]["media"], "avatar-file-id");
    }
}
