drop view public.spam_classifier_training_labels;

alter view public.spam_classifier_user_groups
    rename to spam_classifier_user_groups_evidence;

create view public.spam_classifier_user_groups as
select
    e.chat_id,
    e.telegram_user_id,
    e.explicit_label,
    e.decided_at,
    e.has_nonempty_message_text,
    e.has_nontext_message_evidence,
    e.has_linked_personal_channel,
    e.has_profile_promotion_evidence,
    e.has_audit_evidence,
    e.has_review_request_evidence,
    case
        when e.explicit_label is not null then e.explicit_label
        when not e.has_nonempty_message_text
         and not e.has_profile_promotion_evidence
         and not e.has_audit_evidence
         and not e.has_review_request_evidence then 'undetermined'
        else 'needs_review'
    end as classification_group
from public.spam_classifier_user_groups_evidence e;

comment on view public.spam_classifier_user_groups is
    'Current per-chat moderation group. Empty and media-only accounts without independent promotion evidence remain undetermined; a linked channel alone is not suspicious.';

create view public.spam_classifier_training_labels as
select chat_id, telegram_user_id, explicit_label as label, decided_at
from public.spam_classifier_user_groups
where classification_group in ('spam', 'not_spam');

comment on view public.spam_classifier_training_labels is
    'Only explicit spam/not_spam decisions. Undetermined and needs_review users are excluded.';
