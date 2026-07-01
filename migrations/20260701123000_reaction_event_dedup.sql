create unique index if not exists telegram_message_reactions_unique_event_idx
    on telegram_message_reactions (
        chat_id,
        message_id,
        coalesce(user_id, 0),
        coalesce(actor_chat_id, 0),
        event_at,
        new_reactions
    );
