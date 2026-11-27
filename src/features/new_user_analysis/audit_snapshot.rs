use super::*;

pub(super) struct LoadedAnalysisSnapshot {
    pub(super) features: NewUserFeatures,
    pub(super) membership_context: MembershipContext,
}

pub(super) struct SnapshotLoader;

impl SnapshotLoader {
    pub(super) async fn load(
        pool: &PgPool,
        chat_id: i64,
        telegram_user_id: i64,
        instance_id: &str,
    ) -> anyhow::Result<Option<LoadedAnalysisSnapshot>> {
        if crate::telegram::identity::is_channel_sender_proxy(telegram_user_id) {
            return Ok(None);
        }
        let row = sqlx::query(
        r#"
        with user_messages as (
            select *
            from telegram_messages
            where chat_id = $1
              and user_id = $2
              and source_channel_id is null
        ), first_msg as (
            select message_id, text, reply_to_message_id
            from user_messages
            where nullif(btrim(text), '') is not null
            order by created_at asc
            limit 1
        ), last_msg as (
            select message_id, text
            from user_messages
            order by created_at desc
            limit 1
        ), normalized_messages as (
            select
                message_id,
                text,
                created_at,
                nullif(regexp_replace(lower(coalesce(text, '')), '\\s+', ' ', 'g'), '') as normalized_text
            from user_messages
        ), normalized_counts as (
            select normalized_text, count(*)::bigint as reuse_count
            from normalized_messages
            where normalized_text is not null
            group by normalized_text
        ), msg_stats as (
            select
                count(*)::bigint as message_count,
                count(*) filter (where reply_to_message_id is not null)::bigint as reply_count,
                count(*) filter (where has_links)::bigint as link_count,
                count(*) filter (where has_photo or has_video or has_document or has_audio or has_voice or has_sticker or has_animation)::bigint as media_count,
                count(*) filter (where has_voice)::bigint as voice_count,
                count(*) filter (where reply_to_message_id in (select discussion_message_id from post_comment_jobs where discussion_chat_id = $1))::bigint as reply_to_channel_post_count,
                count(*) filter (where reply_to_message_id in (select bot_comment_message_id from post_comment_jobs where discussion_chat_id = $1))::bigint as reply_to_bot_count,
                count(*) filter (where reply_to_message_id is null)::bigint as top_level_message_count,
                count(*) filter (
                    where reply_to_message_id is not null
                      and reply_to_message_id not in (select discussion_message_id from post_comment_jobs where discussion_chat_id = $1)
                      and reply_to_message_id not in (select bot_comment_message_id from post_comment_jobs where discussion_chat_id = $1)
                )::bigint as reply_to_comment_count,
                count(*) filter (where created_at >= now() - interval '24 hours')::bigint as message_count_24h,
                count(*) filter (where has_links and created_at >= now() - interval '24 hours')::bigint as link_count_24h,
                avg(char_length(coalesce(text, '')))::double precision as avg_message_len,
                array_remove(array_agg(text order by created_at desc, message_id desc), null) as recent_message_texts
            from user_messages
        ), texture_stats as (
            select
                coalesce(sum(reuse_count), 0)::bigint as normalized_message_count,
                count(normalized_text)::bigint as distinct_normalized_message_count,
                coalesce(sum(greatest(reuse_count - 1, 0)), 0)::bigint as duplicate_normalized_message_count,
                coalesce(max(reuse_count), 0)::bigint as max_normalized_message_reuse_count
            from normalized_counts
        ), id_rank as (
            select
                case
                    when max(telegram_user_id) = min(telegram_user_id) then 1.0::double precision
                    else (($2::double precision - min(telegram_user_id)::double precision)
                        / nullif(max(telegram_user_id)::double precision - min(telegram_user_id)::double precision, 0))
                end as ratio
            from telegram_user_profiles
            where telegram_user_id > 0 and not coalesce(is_bot, false)
        ), latest_join_event as (
            select invite_link, via_chat_folder_invite_link
            from telegram_chat_member_events
            where chat_id = $1 and telegram_user_id = $2
              and (old_status in ('left','banned','kicked')
                   or (old_status='restricted'
                       and raw_json #> '{old_chat_member,is_member}' = 'false'::jsonb))
              and (new_status in ('member','administrator','creator','owner')
                   or (new_status='restricted'
                       and raw_json #> '{new_chat_member,is_member}' = 'true'::jsonb))
            order by event_at desc, id desc
            limit 1
        )
        select
            cu.chat_id,
            cu.telegram_user_id,
            cu.first_seen_at,
            cu.last_seen_at,
            extract(epoch from (coalesce(cu.last_seen_at, now()) - cu.first_seen_at))::bigint as account_seen_age_sec,
            extract(epoch from (now() - cu.first_seen_at))::bigint as chat_age_sec,
            cu.first_message_id,
            cu.last_message_id,
            coalesce(ms.message_count, cu.message_count, 0)::bigint as message_count,
            coalesce(ms.reply_count, cu.reply_count, 0)::bigint as reply_count,
            coalesce(ms.link_count, cu.link_count, 0)::bigint as link_count,
            coalesce(ms.media_count, cu.media_count, 0)::bigint as media_count,
            coalesce(ms.voice_count, 0)::bigint as voice_count,
            coalesce(ms.reply_to_channel_post_count, cu.reply_to_channel_post_count, 0)::bigint as reply_to_channel_post_count,
            coalesce(ms.reply_to_bot_count, cu.reply_to_bot_count, 0)::bigint as reply_to_bot_count,
            coalesce(ms.top_level_message_count, 0)::bigint as top_level_message_count,
            coalesce(ms.reply_to_comment_count, 0)::bigint as reply_to_comment_count,
            coalesce(ms.message_count_24h, 0)::bigint as message_count_24h,
            coalesce(ms.link_count_24h, 0)::bigint as link_count_24h,
            case
                when cu.first_seen_at is null or cu.last_seen_at is null then null
                when extract(epoch from (cu.last_seen_at - cu.first_seen_at)) <= 0 then null
                else (coalesce(ms.message_count, cu.message_count, 0)::double precision / greatest(extract(epoch from (cu.last_seen_at - cu.first_seen_at)) / 60.0, 1.0))
            end as burst_messages_per_min,
            fm.message_id as first_message_id_from_messages,
            fm.text as first_message_text,
            reply_parent.text as first_message_reply_context,
            lm.message_id as last_message_id_from_messages,
            lm.text as last_message_text,
            coalesce(ms.recent_message_texts, array[]::text[]) as recent_message_texts,
            coalesce(ts.normalized_message_count, 0)::bigint as normalized_message_count,
            coalesce(ts.distinct_normalized_message_count, 0)::bigint as distinct_normalized_message_count,
            coalesce(ts.duplicate_normalized_message_count, 0)::bigint as duplicate_normalized_message_count,
            coalesce(ts.max_normalized_message_reuse_count, 0)::bigint as max_normalized_message_reuse_count,
            ms.avg_message_len,
            ir.ratio as id_rank_ratio,
            p.username,
            coalesce(uns.reuse_count, 0)::bigint as username_reuse_count,
            coalesce(uns.reuse_spammer_count, 0)::bigint as username_reuse_spammer_count,
            coalesce(cu.is_spammer, false) as local_spammer_identity,
            exists (
                select 1
                from shared_spam_reputation r
                where r.telegram_user_id = cu.telegram_user_id
                  and r.source_instance_id <> $3
            ) as shared_spammer_identity,
            p.first_name,
            p.last_name,
            nullif(trim(concat_ws(' ', p.first_name, p.last_name)), '') as display_name,
            coalesce(dns.reuse_count, 0)::bigint as display_name_reuse_count,
            coalesce(dns.reuse_spammer_count, 0)::bigint as display_name_reuse_spammer_count,
            coalesce(p.is_bot, false) as is_bot,
            p.is_premium,
            p.language_code,
            p.bio,
            p.profile_photo_file_id,
            p.profile_photo_file_unique_id,
            p.profile_photo_count,
            coalesce(pr.reuse_count, 0)::bigint as profile_photo_reuse_count,
            p.profile_photo_width,
            p.profile_photo_height,
            p.emoji_status_custom_emoji_id,
            p.profile_accent_color_id,
            p.personal_channel_chat_id,
            p.personal_channel_title,
            coalesce(pctr.reuse_count, 0)::bigint as personal_channel_title_reuse_count,
            coalesce(pctr.reuse_spammer_count, 0)::bigint
                as personal_channel_title_reuse_spammer_count,
            coalesce(ident.snapshot_count, 0)::bigint as identity_snapshot_count,
            coalesce(ident.display_name_count, 0)::bigint as identity_display_name_count,
            coalesce(ident.username_count, 0)::bigint as identity_username_count,
            p.personal_channel_username,
            p.personal_channel_message_count,
            p.personal_channel_last_message_id,
            p.personal_channel_last_message_at,
            p.personal_channel_last_text,
            coalesce(p.personal_channel_has_adult_links, false) as personal_channel_has_adult_links,
            p.personal_channel_refreshed_at,
            p.personal_channel_fetch_error,
            s.status as member_status,
            cu.membership_first_confirmed_at is not null as membership_confirmed,
            s.is_present as member_is_present,
            s.is_admin as member_is_admin,
            exists (select 1 from latest_join_event) as join_event_seen,
            lje.invite_link,
            coalesce(lje.via_chat_folder_invite_link, false) as via_chat_folder_invite_link
        from telegram_chat_users cu
        left join telegram_user_profiles p on p.telegram_user_id = cu.telegram_user_id
        left join telegram_chat_member_snapshots s on s.chat_id = cu.chat_id and s.telegram_user_id = cu.telegram_user_id
        left join msg_stats ms on true
        left join first_msg fm on true
        left join telegram_messages reply_parent
          on reply_parent.chat_id = cu.chat_id
         and reply_parent.message_id = fm.reply_to_message_id
        left join last_msg lm on true
        left join texture_stats ts on true
        left join id_rank ir on true
        left join latest_join_event lje on true
        left join lateral (
            select
                count(*)::bigint as reuse_count,
                count(*) filter (
                    where coalesce(cu2.is_spammer, false)
                       or exists (
                            select 1 from shared_spam_reputation r
                            where r.telegram_user_id = p2.telegram_user_id
                       )
                )::bigint as reuse_spammer_count
            from telegram_user_profiles p2
            left join telegram_chat_users cu2
              on cu2.chat_id = $1 and cu2.telegram_user_id = p2.telegram_user_id
            where nullif(lower(trim(p.username)), '') is not null
              and lower(trim(p2.username)) = lower(trim(p.username))
              and p2.telegram_user_id <> p.telegram_user_id
        ) uns on true
        left join lateral (
            select
                count(*)::bigint as reuse_count,
                count(*) filter (
                    where coalesce(cu2.is_spammer, false)
                       or exists (
                            select 1 from shared_spam_reputation r
                            where r.telegram_user_id = p2.telegram_user_id
                       )
                )::bigint as reuse_spammer_count
            from telegram_user_profiles p2
            left join telegram_chat_users cu2
              on cu2.chat_id = $1 and cu2.telegram_user_id = p2.telegram_user_id
            where nullif(trim(concat_ws(' ', p.first_name, p.last_name)), '') is not null
              and lower(nullif(trim(concat_ws(' ', p2.first_name, p2.last_name)), '')) = lower(nullif(trim(concat_ws(' ', p.first_name, p.last_name)), ''))
              and p2.telegram_user_id <> p.telegram_user_id
        ) dns on true
        left join lateral (
            select count(*)::bigint as reuse_count
            from telegram_user_profiles p2
            where p.profile_photo_file_unique_id is not null
              and p2.profile_photo_file_unique_id = p.profile_photo_file_unique_id
              and p2.telegram_user_id <> p.telegram_user_id
        ) pr on true
        left join lateral (
            select
                count(*)::bigint as reuse_count,
                count(*) filter (
                    where coalesce(cu2.is_spammer, false)
                       or exists (
                            select 1 from shared_spam_reputation r
                            where r.telegram_user_id = p2.telegram_user_id
                       )
                )::bigint as reuse_spammer_count
            from telegram_user_profiles p2
            left join telegram_chat_users cu2
              on cu2.chat_id = $1 and cu2.telegram_user_id = p2.telegram_user_id
            where nullif(lower(trim(p.personal_channel_title)), '') is not null
              and char_length(trim(p.personal_channel_title)) >= 4
              and lower(trim(p2.personal_channel_title)) = lower(trim(p.personal_channel_title))
              and p2.telegram_user_id <> p.telegram_user_id
        ) pctr on true
        left join lateral (
            select
                count(*)::bigint as snapshot_count,
                count(distinct display_name_normalized)::bigint as display_name_count,
                count(distinct username_normalized)::bigint as username_count
            from telegram_profile_identity_observations o
            where o.telegram_user_id = cu.telegram_user_id
        ) ident on true
        where cu.chat_id = $1 and cu.telegram_user_id = $2
        "#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .bind(instance_id)
    .fetch_optional(pool)
    .await?;

        Ok(row.map(|row| {
            let status: Option<String> = row.get("member_status");
            let membership_context = MembershipContext::from_snapshot(
                status.as_deref(),
                row.get("member_is_present"),
                row.get("membership_confirmed"),
            );
            let recent_message_texts = row.get::<Vec<String>, _>("recent_message_texts");
            let message_style = message_style(&recent_message_texts);
            let max_pairwise_similarity = max_pairwise_message_similarity(&recent_message_texts);
            let max_reuse_count = row.get("max_normalized_message_reuse_count");
            let duplicate_normalized_message_count = row.get("duplicate_normalized_message_count");
            let repetitive_pattern = duplicate_normalized_message_count > 0
                || max_reuse_count > 1
                || max_pairwise_similarity.is_some_and(|similarity| similarity >= 0.86);

            let features = NewUserFeatures {
                chat_id: row.get("chat_id"),
                telegram_user_id: row.get("telegram_user_id"),
                first_seen_at: row.get("first_seen_at"),
                last_seen_at: row.get("last_seen_at"),
                account_seen_age_sec: row.get("account_seen_age_sec"),
                chat_age_sec: row.get("chat_age_sec"),
                first_message_id: row
                    .try_get("first_message_id_from_messages")
                    .ok()
                    .or_else(|| row.try_get("first_message_id").ok()),
                last_message_id: row
                    .try_get("last_message_id_from_messages")
                    .ok()
                    .or_else(|| row.try_get("last_message_id").ok()),
                message_count: row.get("message_count"),
                reply_count: row.get("reply_count"),
                link_count: row.get("link_count"),
                media_count: row.get("media_count"),
                voice_count: row.get("voice_count"),
                reply_to_channel_post_count: row.get("reply_to_channel_post_count"),
                reply_to_bot_count: row.get("reply_to_bot_count"),
                top_level_message_count: row.get("top_level_message_count"),
                reply_to_comment_count: row.get("reply_to_comment_count"),
                message_count_24h: row.get("message_count_24h"),
                link_count_24h: row.get("link_count_24h"),
                burst_messages_per_min: row.get("burst_messages_per_min"),
                first_message_text: row.get("first_message_text"),
                first_message_reply_context: row.get("first_message_reply_context"),
                last_message_text: row.get("last_message_text"),
                recent_message_texts,
                text_texture: TextTexture {
                    normalized_count: row.get("normalized_message_count"),
                    distinct_normalized_count: row.get("distinct_normalized_message_count"),
                    duplicate_normalized_count: duplicate_normalized_message_count,
                    max_reuse_count,
                    max_pairwise_similarity,
                    avg_message_len: row.get("avg_message_len"),
                    repetitive_pattern,
                },
                message_style,
                id_rank_ratio: row.get("id_rank_ratio"),
                username: row.get("username"),
                username_reuse_count: row.get("username_reuse_count"),
                username_reuse_spammer_count: row.get("username_reuse_spammer_count"),
                local_spammer_identity: row.get("local_spammer_identity"),
                shared_spammer_identity: row.get("shared_spammer_identity"),
                // Поле внешнего scorer-крейта оставлено выключенным для этого consumer-а.
                lols_spammer_identity: false,
                first_name: row.get("first_name"),
                last_name: row.get("last_name"),
                display_name: row.get("display_name"),
                display_name_reuse_count: row.get("display_name_reuse_count"),
                display_name_reuse_spammer_count: row.get("display_name_reuse_spammer_count"),
                is_bot: row.get("is_bot"),
                is_premium: row.get("is_premium"),
                language_code: row.get("language_code"),
                bio: row.get("bio"),
                profile_photo_file_id: row.get("profile_photo_file_id"),
                profile_photo_file_unique_id: row.get("profile_photo_file_unique_id"),
                profile_photo_count: row.get("profile_photo_count"),
                profile_photo_reuse_count: row.get("profile_photo_reuse_count"),
                profile_photo_width: row.get("profile_photo_width"),
                profile_photo_height: row.get("profile_photo_height"),
                emoji_status_custom_emoji_id: row.get("emoji_status_custom_emoji_id"),
                profile_accent_color_id: row.get("profile_accent_color_id"),
                personal_channel_chat_id: row.get("personal_channel_chat_id"),
                personal_channel_title: row.get("personal_channel_title"),
                personal_channel_title_reuse_count: row.get("personal_channel_title_reuse_count"),
                personal_channel_title_reuse_spammer_count: row
                    .get("personal_channel_title_reuse_spammer_count"),
                identity_snapshot_count: row.get("identity_snapshot_count"),
                identity_display_name_count: row.get("identity_display_name_count"),
                identity_username_count: row.get("identity_username_count"),
                personal_channel_username: row.get("personal_channel_username"),
                personal_channel_message_count: row.get("personal_channel_message_count"),
                personal_channel_last_message_id: row.get("personal_channel_last_message_id"),
                personal_channel_last_message_at: row.get("personal_channel_last_message_at"),
                personal_channel_last_text: row.get("personal_channel_last_text"),
                personal_channel_has_adult_links: row.get("personal_channel_has_adult_links"),
                personal_channel_refreshed_at: row.get("personal_channel_refreshed_at"),
                personal_channel_fetch_error: row.get("personal_channel_fetch_error"),
                member_status: row.get("member_status"),
                member_is_present: row.get("member_is_present"),
                member_is_admin: row.get("member_is_admin"),
                join_event_seen: row.get("join_event_seen"),
                invite_link: row.get("invite_link"),
                via_chat_folder_invite_link: row.get("via_chat_folder_invite_link"),
            };
            LoadedAnalysisSnapshot {
                features,
                membership_context,
            }
        }))
    }
}
