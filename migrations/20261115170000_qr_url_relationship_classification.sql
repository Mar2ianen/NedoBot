alter table public.telegram_message_url_evidence
    add column qr_relation text;

alter table public.telegram_message_url_evidence
    add constraint telegram_message_url_evidence_qr_relation_check
    check (qr_relation is null or qr_relation in ('affiliated', 'external', 'unresolved'));

alter table public.telegram_message_url_evidence
    add constraint telegram_message_url_evidence_qr_relation_source_check
    check (qr_relation is null or (source = 'qr' and kind = 'url'));

create index telegram_message_url_evidence_external_qr_idx
    on public.telegram_message_url_evidence (chat_id, message_id)
    where source = 'qr' and kind = 'url' and qr_relation = 'external';
