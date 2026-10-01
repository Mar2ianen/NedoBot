drop view public.spam_classifier_training_labels;
drop view public.spam_classifier_user_groups;

create view public.spam_classifier_user_groups as
with decision_candidates as (
    select e.chat_id, e.telegram_user_id, e.label, e.created_at as decided_at,
           3 as source_priority, e.id as source_id
    from public.spam_label_events e

    union all

    select r.chat_id, r.telegram_user_id,
           case r.status
               when 'confirmed_spam' then 'spam'
               when 'confirmed_not_spam' then 'not_spam'
           end as label,
           coalesce(r.reviewed_at, r.notified_at) as decided_at,
           2 as source_priority, r.id as source_id
    from public.spam_review_requests r
    where r.status in ('confirmed_spam', 'confirmed_not_spam')

    union all

    select u.chat_id, u.telegram_user_id, 'spam'::text as label,
           coalesce(u.spam_last_marked_at, u.updated_at, u.created_at) as decided_at,
           1 as source_priority, 0::bigint as source_id
    from public.telegram_chat_users u
    where u.is_spammer
), latest_decisions as (
    select distinct on (chat_id, telegram_user_id)
           chat_id, telegram_user_id, label, decided_at
    from decision_candidates
    order by chat_id, telegram_user_id, decided_at desc, source_priority desc, source_id desc
), evidence as (
    select
        u.chat_id,
        u.telegram_user_id,
        exists (
            select 1 from public.telegram_messages m
            where m.chat_id = u.chat_id and m.user_id = u.telegram_user_id
              and m.source_channel_id is null and m.text ~ '[^[:space:]]'
        ) as has_nonempty_message_text,
        exists (
            select 1 from public.telegram_messages m
            where m.chat_id = u.chat_id and m.user_id = u.telegram_user_id
              and m.source_channel_id is null
              and (m.has_links or m.has_photo or m.has_video or m.has_document
                   or m.has_audio or m.has_voice or m.has_sticker or m.has_animation)
        ) as has_nontext_message_evidence,
        (
            coalesce(p.personal_channel_chat_id, a.personal_channel_chat_id) is not null
            or nullif(btrim(coalesce(p.personal_channel_title, a.personal_channel_title, '')), '') is not null
            or nullif(btrim(coalesce(p.personal_channel_username, a.personal_channel_username, '')), '') is not null
        ) as has_linked_personal_channel,
        (
            concat_ws(' ', p.username, p.first_name, p.last_name, p.bio,
                           p.personal_channel_title, p.personal_channel_username,
                           p.personal_channel_last_text, a.username, a.display_name, a.bio,
                           a.personal_channel_title, a.personal_channel_username,
                           a.personal_channel_last_text)
            ~* '(https?://|www[.]|t[.]me/|telegram[.]me|onlyfans|онлифанс|fansly|казино|букмекер|ставк|крипт|инвест|заработ|доход|впн|vpn|промокод|скидк|в лс|в личк|подписк|реклам|продам|куплю|розыгрыш|giveaway|пиши(те)?[[:space:]]+(мне|в)|марафон|личный[[:space:]]+бренд)'
        ) as has_profile_promotion_evidence,
        exists (
            select 1 from public.telegram_new_user_profile_audits audit
            where audit.chat_id = u.chat_id and audit.telegram_user_id = u.telegram_user_id
              and (
                  audit.personal_channel_has_adult_links
                  or audit.personal_channel_has_invite_link
                  or audit.personal_channel_has_external_link
                  or audit.primary_risk_class in ('adult_personal_channel', 'promo_dm_bait', 'link_dropper')
                  or audit.risk_score >= 70
                  or concat_ws(' ', audit.risk_signal_breakdown::text,
                                      audit.risk_baseline_signals::text,
                                      audit.risk_first_message_signals::text,
                                      audit.risk_avatar_signals::text,
                                      audit.risk_personal_channel_signals::text,
                                      audit.risk_labels::text,
                                      audit.risk_reasons::text)
                     ~* '(explicit_adult_promo_bio|profile_bio_subscription_invite_offer|personal_channel_adult_links|personal_channel_invite_link|personal_channel_external_link|tree_personal_channel_invite_funnel|llm_personal_channel_content_promotion|commercial_or_scam_presentation|bio_or_username_promotion|suggestive_bait)'
              )
        ) as has_audit_evidence,
        exists (
            select 1 from public.spam_review_requests review
            where review.chat_id = u.chat_id and review.telegram_user_id = u.telegram_user_id
              and (
                  review.risk_score >= 70
                  or review.risk_signals::text
                     ~* '(explicit_adult_promo_bio|profile_bio_subscription_invite_offer|personal_channel_adult_links|personal_channel_invite_link|personal_channel_external_link|tree_personal_channel_invite_funnel|llm_personal_channel_content_promotion|commercial_or_scam_presentation|bio_or_username_promotion|suggestive_bait)'
              )
        ) as has_review_request_evidence
    from public.telegram_chat_users u
    left join public.telegram_user_profiles p on p.telegram_user_id = u.telegram_user_id
    left join public.telegram_new_user_profile_audits a
      on a.chat_id = u.chat_id and a.telegram_user_id = u.telegram_user_id
)
select
    e.chat_id,
    e.telegram_user_id,
    d.label as explicit_label,
    d.decided_at,
    e.has_nonempty_message_text,
    e.has_nontext_message_evidence,
    e.has_linked_personal_channel,
    e.has_profile_promotion_evidence,
    e.has_audit_evidence,
    e.has_review_request_evidence,
    case
        when d.label is not null then d.label
        when not e.has_nonempty_message_text
         and not e.has_nontext_message_evidence
         and not e.has_linked_personal_channel
         and not e.has_profile_promotion_evidence
         and not e.has_audit_evidence
         and not e.has_review_request_evidence then 'undetermined'
        else 'needs_review'
    end as classification_group
from evidence e
left join latest_decisions d using (chat_id, telegram_user_id);

comment on view public.spam_classifier_user_groups is
    'Current per-chat moderation group. Weak generic risk signals alone do not make empty accounts reviewable or create negative labels.';

create view public.spam_classifier_training_labels as
select chat_id, telegram_user_id, explicit_label as label, decided_at
from public.spam_classifier_user_groups
where classification_group in ('spam', 'not_spam');

comment on view public.spam_classifier_training_labels is
    'Only explicit spam/not_spam decisions. Undetermined and needs_review users are excluded.';
