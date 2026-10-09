-- Покрывает все поля user_totals для пользовательских сообщений и позволяет
-- читать историю участника индексом без случайных обращений к heap.
create index if not exists telegram_messages_userstatus_totals_idx
    on telegram_messages (chat_id, user_id, created_at)
    include (
        reply_to_message_id,
        has_links,
        has_photo,
        has_video,
        has_document,
        has_audio,
        has_voice,
        has_sticker,
        has_animation
    )
    where source_channel_id is null;
