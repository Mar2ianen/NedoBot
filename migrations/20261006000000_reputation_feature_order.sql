-- Пересчитываем derived scores после исправления порядка Gemma/TF-IDF
-- и has_text. Сохранённый assessment остаётся источником для materialization;
-- LLM-аудит повторно не запускается. Смена версии инвалидирует старые CAS claims.
update new_user_audit_jobs
set materialization_version = 'unified-audit-materialization-v8',
    materialization_status = 'retry_wait',
    materialization_attempts = 0,
    materialization_next_attempt_at = now(),
    materialization_processing_started_at = null,
    materialization_lease_expires_at = null,
    materialization_error_kind = null,
    materialized_at = null,
    updated_at = now()
where status = 'succeeded'
  and assessment_json is not null
  and materialization_version = 'unified-audit-materialization-v7';
