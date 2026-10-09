-- user_totals recursively walks post threads on every /userstatus. Restrict
-- the seed scan to forwarded channel-post roots. The release applies this
-- migration before the service resumes polling, so the build does not block ingestion.
CREATE INDEX IF NOT EXISTS telegram_messages_source_channel_root_idx
    ON telegram_messages (chat_id, message_id)
    WHERE source_channel_id IS NOT NULL;
