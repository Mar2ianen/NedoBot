use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};

use super::types::{
    ChatInteraction, ChatMessage, ChatUserProfile, MessageMatch, MessageSearchPage,
    MessageSearchRequest, RecentMessagesRequest, SemanticSearchConfig,
};

use crate::features::memory::embedding::{
    CHAT_EMBEDDING_DIMENSIONS, embed_chat_query_at, pgvector_literal_for_dimensions,
};

const MAX_QUERY_CHARS: usize = 240;
const MAX_RESULT_LIMIT: i64 = 50;
pub(crate) const MAX_SEARCH_OFFSET: i64 = 10_000;
const MAX_CONTEXT_MESSAGES: i64 = 5;
const MAX_MESSAGE_PREVIEW_CHARS: usize = 4_096;
const MAX_SEMANTIC_CANDIDATES: i64 = 5_000;

#[derive(FromRow)]
struct MessageRow {
    message_id: i32,
    user_id: Option<i64>,
    author: String,
    author_username: Option<String>,
    is_forwarded: bool,
    forwarded_from: Option<String>,
    text: String,
    reply_to_message_id: Option<i32>,
    created_at: DateTime<Utc>,
    relevance: f32,
}

#[derive(FromRow)]
struct SearchPageRow {
    message_id: Option<i32>,
    user_id: Option<i64>,
    author: Option<String>,
    author_username: Option<String>,
    is_forwarded: Option<bool>,
    forwarded_from: Option<String>,
    text: Option<String>,
    reply_to_message_id: Option<i32>,
    created_at: Option<DateTime<Utc>>,
    relevance: Option<f32>,
    total_count: i64,
}

#[derive(FromRow)]
struct InteractionRow {
    message_id: i32,
    user_id: Option<i64>,
    author: String,
    author_username: Option<String>,
    is_forwarded: bool,
    forwarded_from: Option<String>,
    text: String,
    reply_to_message_id: Option<i32>,
    created_at: DateTime<Utc>,
    replied_to_message_id: Option<i32>,
    replied_to_user_id: Option<i64>,
    replied_to_author: Option<String>,
    replied_to_username: Option<String>,
    replied_to_is_forwarded: Option<bool>,
    replied_to_forwarded_from: Option<String>,
    replied_to_text: Option<String>,
    replied_to_created_at: Option<DateTime<Utc>>,
}

pub async fn search_messages(
    pool: &PgPool,
    chat_id: i64,
    request: &MessageSearchRequest,
) -> anyhow::Result<MessageSearchPage> {
    search_messages_with_semantic(pool, chat_id, request, None).await
}

pub async fn search_messages_with_semantic(
    pool: &PgPool,
    chat_id: i64,
    request: &MessageSearchRequest,
    semantic_config: Option<&SemanticSearchConfig>,
) -> anyhow::Result<MessageSearchPage> {
    let query = normalized_query(&request.query)?;
    let ts_query = full_text_query(&query, &request.match_mode);
    let whole_word_pattern = whole_word_pattern(&query);
    let query_embedding = query_embedding(semantic_config, request, &query).await?;
    let query_embedding = query_embedding.as_deref();
    let rows = sqlx::query_as::<_, SearchPageRow>(
        r#"
        with semantic_candidates as materialized (
            select
                e.chat_id,
                e.message_id,
                greatest(
                    1.0 - (e.embedding <=> $26::halfvec),
                    0.0
                )::real as semantic_relevance
            from telegram_message_embeddings_gemma e
            where e.chat_id = $1
              and e.status = 'ready'
              and e.embedding_model = $27
              and $26 is not null
            order by (e.embedding <=> $26::halfvec) + 0.0
            limit $28
        ),
        candidate_ids as materialized (
            select
                m.chat_id,
                m.message_id
            from mcp_public.telegram_messages m
            left join mcp_public.telegram_user_profiles p on p.telegram_user_id = m.user_id
            where m.chat_id = $1
              and m.text is not null
              and m.deleted_by_bot_at is null
              and m.spam_marked_at is null
              and (
                  m.user_id is not null
                  or ($21::boolean and (
                      coalesce(m.is_forwarded, false)
                      or coalesce(m.is_automatic_forward, false)
                  ))
              )
              and not coalesce(p.is_bot, false)
              and ($21::boolean or not (
                  coalesce(m.is_forwarded, false)
                  or coalesce(m.is_automatic_forward, false)
              ))
              and ($5::bigint is null or m.user_id = $5)
              and ($6::timestamptz is null or m.created_at >= $6)
              and ($7::timestamptz is null or m.created_at <= $7)
              and ($8::integer is null or m.reply_to_message_id = $8)
              and ($9::boolean is null or m.has_links = $9)
              and (
                  $10::boolean is null
                  or (m.has_photo or m.has_video or m.has_document or m.has_audio
                      or m.has_voice or m.has_sticker or m.has_animation) = $10
              )
              and ($11::boolean is null or m.has_photo = $11)
              and ($12::boolean is null or m.has_video = $12)
              and ($13::boolean is null or m.has_document = $13)
              and ($14::boolean is null or m.has_audio = $14)
              and ($15::boolean is null or m.has_voice = $15)
              and ($16::boolean is null or m.has_sticker = $16)
              and ($17::boolean is null or m.has_animation = $17)
              and ($23::boolean is null or m.is_automatic_forward = $23)
              and ($24::boolean is null or (m.reply_to_message_id is not null) = $24)
              and ($25::boolean is null or m.is_forwarded = $25)
              and (
                  ($20 in ('hybrid', 'full_text', 'any_terms') and (
                       to_tsvector('russian', coalesce(m.text, '')) @@ websearch_to_tsquery('russian', $2)
                    or to_tsvector('simple', coalesce(m.text, '')) @@ websearch_to_tsquery('simple', $2)
                  ))
                  or ($20 = 'hybrid' and lower($3) <% lower(m.text))
                  or ($20 = 'literal' and position(lower($3) in lower(m.text)) > 0)
                  or ($20 = 'whole_word' and m.text ~* $4)
              )
            union
            select
                semantic.chat_id,
                semantic.message_id
            from semantic_candidates semantic
            join mcp_public.telegram_messages m
              on m.chat_id = semantic.chat_id
             and m.message_id = semantic.message_id
            left join mcp_public.telegram_user_profiles p on p.telegram_user_id = m.user_id
            where m.chat_id = $1
              and m.text is not null
              and m.deleted_by_bot_at is null
              and m.spam_marked_at is null
              and (
                  m.user_id is not null
                  or ($21::boolean and (
                      coalesce(m.is_forwarded, false)
                      or coalesce(m.is_automatic_forward, false)
                  ))
              )
              and not coalesce(p.is_bot, false)
              and ($21::boolean or not (
                  coalesce(m.is_forwarded, false)
                  or coalesce(m.is_automatic_forward, false)
              ))
              and ($5::bigint is null or m.user_id = $5)
              and ($6::timestamptz is null or m.created_at >= $6)
              and ($7::timestamptz is null or m.created_at <= $7)
              and ($8::integer is null or m.reply_to_message_id = $8)
              and ($9::boolean is null or m.has_links = $9)
              and (
                  $10::boolean is null
                  or (m.has_photo or m.has_video or m.has_document or m.has_audio
                      or m.has_voice or m.has_sticker or m.has_animation) = $10
              )
              and ($11::boolean is null or m.has_photo = $11)
              and ($12::boolean is null or m.has_video = $12)
              and ($13::boolean is null or m.has_document = $13)
              and ($14::boolean is null or m.has_audio = $14)
              and ($15::boolean is null or m.has_sticker = $15)
              and ($16::boolean is null or m.has_animation = $16)
              and ($23::boolean is null or m.is_automatic_forward = $23)
              and ($24::boolean is null or (m.reply_to_message_id is not null) = $24)
              and ($25::boolean is null or m.is_forwarded = $25)
        ),
        matched as materialized (
            select
                m.message_id,
                m.user_id,
                coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''),
                         nullif(p.username, ''),
                         'Неизвестный пользователь') as author,
                nullif(p.username, '') as author_username,
                m.is_forwarded,
                m.forwarded_from,
                m.text,
                m.reply_to_message_id,
               m.created_at,
                (
                    case
                        when $20 = 'hybrid' and $26 is not null then
                            least(greatest(
                                ts_rank_cd(to_tsvector('russian', coalesce(m.text, '')), websearch_to_tsquery('russian', $2)),
                                ts_rank_cd(to_tsvector('simple', coalesce(m.text, '')), websearch_to_tsquery('simple', $2))
                            ), 1.0) * 0.45
                        when $20 in ('full_text', 'any_terms') then greatest(
                            ts_rank_cd(to_tsvector('russian', coalesce(m.text, '')), websearch_to_tsquery('russian', $2)),
                            ts_rank_cd(to_tsvector('simple', coalesce(m.text, '')), websearch_to_tsquery('simple', $2))
                        )
                        when $20 = 'literal' and position(lower($3) in lower(m.text)) > 0 then 1.0
                        when $20 = 'whole_word' and m.text ~* $4 then 1.0
                        else 0.0
                    end
                    + case when $20 = 'hybrid' and lower($3) <% lower(m.text)
                           then greatest(word_similarity(lower($3), lower(m.text)) - 0.6, 0.0) *
                                case when $26 is not null then 0.10 else 0.25 end
                           else 0.0
                      end
                    + case when $20 = 'hybrid'
                                and $26 is not null
                           then coalesce(semantic.semantic_relevance, 0.0) * 0.55
                           else 0.0
                      end
                )::real as relevance
            from candidate_ids candidate
            join mcp_public.telegram_messages m
              on m.chat_id = candidate.chat_id
             and m.message_id = candidate.message_id
            left join mcp_public.telegram_user_profiles p on p.telegram_user_id = m.user_id
            left join semantic_candidates semantic
              on semantic.chat_id = m.chat_id
             and semantic.message_id = m.message_id
        )
        select
            page.message_id,
            page.user_id,
            page.author,
            page.author_username,
            page.is_forwarded,
            page.forwarded_from,
            page.text,
            page.reply_to_message_id,
            page.created_at,
            page.relevance,
            total.total_count
        from (
            select count(*)::bigint as total_count
            from matched
        ) total
        left join lateral (
            select
                selected.*,
                row_number() over (
                    order by
                        case when $18 = 'newest' then selected.created_at end desc,
                        case when $18 = 'oldest' then selected.created_at end asc,
                        selected.relevance desc,
                        selected.created_at desc,
                        selected.message_id desc
                ) as page_position
            from (
                select *
                from matched
                order by
                    case when $18 = 'newest' then created_at end desc,
                    case when $18 = 'oldest' then created_at end asc,
                    relevance desc,
                    created_at desc,
                    message_id desc
                limit $19
                offset $22
            ) selected
        ) page on true
        order by page.page_position
        "#,
    )
    .bind(chat_id)
    .bind(&ts_query)
    .bind(&query)
    .bind(&whole_word_pattern)
    .bind(request.user_id)
    .bind(request.date_from)
    .bind(request.date_to)
    .bind(request.reply_to_message_id)
    .bind(request.has_links)
    .bind(request.has_media)
    .bind(request.has_photo)
    .bind(request.has_video)
    .bind(request.has_document)
    .bind(request.has_audio)
    .bind(request.has_voice)
    .bind(request.has_sticker)
    .bind(request.has_animation)
    .bind(request.sort.as_str())
    .bind(request.limit.clamp(1, MAX_RESULT_LIMIT))
    .bind(request.match_mode.as_str())
    .bind(request.include_forwards)
    .bind(request.offset.clamp(0, MAX_SEARCH_OFFSET))
    .bind(request.is_automatic_forward)
    .bind(request.has_reply)
    .bind(request.is_forwarded)
    .bind(query_embedding)
    .bind(
        semantic_config
            .map(|config| config.embedding_model.as_str())
            .unwrap_or_default(),
    )
    .bind(MAX_SEMANTIC_CANDIDATES)
    .fetch_all(pool)
    .await?;

    let (total_count, messages) = map_search_page_rows(chat_id, rows);
    let offset = request.offset.clamp(0, MAX_SEARCH_OFFSET);
    let (has_more, next_offset, scan_limit_reached) =
        page_metadata(total_count, offset, messages.len());
    Ok(MessageSearchPage {
        has_more,
        messages,
        total_count,
        next_offset,
        scan_limit_reached,
    })
}

async fn query_embedding(
    semantic_config: Option<&SemanticSearchConfig>,
    request: &MessageSearchRequest,
    query: &str,
) -> anyhow::Result<Option<String>> {
    if query.is_empty() || !matches!(request.match_mode, MessageMatch::Hybrid) {
        return Ok(None);
    }
    let Some(config) = semantic_config else {
        return Ok(None);
    };

    match embed_chat_query_at(
        &config.embedding_url,
        config.timeout_sec,
        &config.embedding_model,
        &config.query_prefix,
        query,
    )
    .await
    {
        Ok(embedding) => {
            match pgvector_literal_for_dimensions(&embedding, CHAT_EMBEDDING_DIMENSIONS) {
                Ok(literal) => Ok(Some(literal)),
                Err(error) => {
                    tracing::warn!(%error, "semantic chat query vector is invalid; continuing with lexical search");
                    Ok(None)
                }
            }
        }
        Err(error) => {
            tracing::warn!(%error, "semantic chat query failed; continuing with lexical search");
            Ok(None)
        }
    }
}

pub async fn count_messages(
    pool: &PgPool,
    chat_id: i64,
    request: &MessageSearchRequest,
) -> anyhow::Result<i64> {
    let query = if request.query.trim().is_empty() {
        String::new()
    } else {
        normalized_query(&request.query)?
    };
    let ts_query = full_text_query(&query, &request.match_mode);
    let whole_word_pattern = whole_word_pattern(&query);
    count_matching_messages(
        pool,
        chat_id,
        request,
        &ts_query,
        &query,
        &whole_word_pattern,
    )
    .await
}

async fn count_matching_messages(
    pool: &PgPool,
    chat_id: i64,
    request: &MessageSearchRequest,
    ts_query: &str,
    query: &str,
    whole_word_pattern: &str,
) -> anyhow::Result<i64> {
    sqlx::query_scalar::<_, i64>(
        r#"
        select count(*)::bigint
        from mcp_public.telegram_messages m
        left join mcp_public.telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1
          and ($3 = '' or m.text is not null)
          and m.deleted_by_bot_at is null
          and m.spam_marked_at is null
          and (
              m.user_id is not null
              or ($19::boolean and (
                  coalesce(m.is_forwarded, false)
                  or coalesce(m.is_automatic_forward, false)
              ))
          )
          and not coalesce(p.is_bot, false)
          and ($19::boolean or not (
              coalesce(m.is_forwarded, false)
              or coalesce(m.is_automatic_forward, false)
          ))
          and ($5::bigint is null or m.user_id = $5)
          and ($6::timestamptz is null or m.created_at >= $6)
          and ($7::timestamptz is null or m.created_at <= $7)
          and ($8::integer is null or m.reply_to_message_id = $8)
          and ($9::boolean is null or m.has_links = $9)
          and (
              $10::boolean is null
                  or (m.has_photo or m.has_video or m.has_document or m.has_audio
                      or m.has_voice or m.has_sticker or m.has_animation) = $10
          )
          and ($11::boolean is null or m.has_photo = $11)
          and ($12::boolean is null or m.has_video = $12)
          and ($13::boolean is null or m.has_document = $13)
          and ($14::boolean is null or m.has_audio = $14)
          and ($15::boolean is null or m.has_voice = $15)
          and ($16::boolean is null or m.has_sticker = $16)
          and ($17::boolean is null or m.has_animation = $17)
          and ($20::boolean is null or m.is_automatic_forward = $20)
          and ($21::boolean is null or (m.reply_to_message_id is not null) = $21)
          and ($22::boolean is null or m.is_forwarded = $22)
          and (
              $3 = ''
              or ($18 in ('hybrid', 'full_text', 'any_terms') and (
                   to_tsvector('russian', coalesce(m.text, '')) @@ websearch_to_tsquery('russian', $2)
                or to_tsvector('simple', coalesce(m.text, '')) @@ websearch_to_tsquery('simple', $2)
              ))
              or ($18 = 'hybrid' and lower($3) <% lower(m.text))
              or ($18 = 'literal' and position(lower($3) in lower(m.text)) > 0)
              or ($18 = 'whole_word' and m.text ~* $4)
          )
        "#,
    )
    .bind(chat_id)
    .bind(ts_query)
    .bind(query)
    .bind(whole_word_pattern)
    .bind(request.user_id)
    .bind(request.date_from)
    .bind(request.date_to)
    .bind(request.reply_to_message_id)
    .bind(request.has_links)
    .bind(request.has_media)
    .bind(request.has_photo)
    .bind(request.has_video)
    .bind(request.has_document)
    .bind(request.has_audio)
    .bind(request.has_voice)
    .bind(request.has_sticker)
    .bind(request.has_animation)
    .bind(request.match_mode.as_str())
    .bind(request.include_forwards)
    .bind(request.is_automatic_forward)
    .bind(request.has_reply)
    .bind(request.is_forwarded)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

pub async fn recent_messages(
    pool: &PgPool,
    chat_id: i64,
    request: &RecentMessagesRequest,
) -> anyhow::Result<Vec<ChatMessage>> {
    let rows = sqlx::query_as::<_, MessageRow>(
        r#"
        select m.message_id, m.user_id,
               coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''), nullif(p.username, ''), 'Неизвестный пользователь') as author,
               nullif(p.username, '') as author_username,
               m.is_forwarded,
               m.forwarded_from,
               coalesce(m.text, '[медиа без текста]') as text,
               m.reply_to_message_id, m.created_at, 0::real as relevance
        from mcp_public.telegram_messages m
        left join mcp_public.telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1
          and m.deleted_by_bot_at is null
          and m.spam_marked_at is null
          and (
              m.user_id is not null
              or ($16::boolean and (
                  coalesce(m.is_forwarded, false)
                  or coalesce(m.is_automatic_forward, false)
              ))
          )
          and not coalesce(p.is_bot, false)
          and ($16::boolean or not (
              coalesce(m.is_forwarded, false)
              or coalesce(m.is_automatic_forward, false)
          ))
          and ($2::bigint is null or m.user_id = $2)
          and ($3::timestamptz is null or m.created_at >= $3)
          and ($4::timestamptz is null or m.created_at <= $4)
          and ($5::boolean is null or m.has_links = $5)
          and ($6::boolean is null or (m.has_photo or m.has_video or m.has_document or m.has_audio or m.has_voice or m.has_sticker or m.has_animation) = $6)
          and ($7::boolean is null or m.has_photo = $7)
          and ($8::boolean is null or m.has_video = $8)
          and ($9::boolean is null or m.has_document = $9)
          and ($10::boolean is null or m.has_audio = $10)
          and ($11::boolean is null or m.has_voice = $11)
          and ($12::boolean is null or m.has_sticker = $12)
          and ($13::boolean is null or m.has_animation = $13)
          and ($17::boolean is null or m.is_automatic_forward = $17)
          and ($18::boolean is null or (m.reply_to_message_id is not null) = $18)
          and ($19::boolean is null or m.is_forwarded = $19)
        order by
            case when $14 = 'oldest' then m.created_at end asc,
            case when $14 <> 'oldest' then m.created_at end desc,
            case when $14 = 'oldest' then m.message_id end asc,
            m.message_id desc
        limit $15
        "#,
    )
    .bind(chat_id)
    .bind(request.user_id)
    .bind(request.date_from)
    .bind(request.date_to)
    .bind(request.has_links)
    .bind(request.has_media)
    .bind(request.has_photo)
    .bind(request.has_video)
    .bind(request.has_document)
    .bind(request.has_audio)
    .bind(request.has_voice)
    .bind(request.has_sticker)
    .bind(request.has_animation)
    .bind(request.sort.as_str())
    .bind(request.limit.clamp(1, MAX_RESULT_LIMIT))
    .bind(request.include_forwards)
    .bind(request.is_automatic_forward)
    .bind(request.has_reply)
    .bind(request.is_forwarded)
    .fetch_all(pool)
    .await?;
    Ok(map_rows(chat_id, rows))
}

pub async fn message_context(
    pool: &PgPool,
    chat_id: i64,
    message_id: i32,
    before: i64,
    after: i64,
) -> anyhow::Result<Vec<ChatMessage>> {
    let rows = sqlx::query_as::<_, MessageRow>(
        r#"
        select
            m.message_id,
            m.user_id,
            coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''),
                     nullif(p.username, ''),
                     'Неизвестный пользователь') as author,
            nullif(p.username, '') as author_username,
            m.is_forwarded,
            m.forwarded_from,
            coalesce(m.text, '[медиа без текста]') as text,
            m.reply_to_message_id,
            m.created_at,
            0::real as relevance
        from mcp_public.telegram_messages m
        left join mcp_public.telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1
          and m.deleted_by_bot_at is null
          and m.spam_marked_at is null
          and m.message_id between $2 - $3 and $2 + $4
        order by m.message_id asc
        "#,
    )
    .bind(chat_id)
    .bind(message_id)
    .bind(before.clamp(0, MAX_CONTEXT_MESSAGES) as i32)
    .bind(after.clamp(0, MAX_CONTEXT_MESSAGES) as i32)
    .fetch_all(pool)
    .await?;

    Ok(map_rows(chat_id, rows))
}

pub async fn reply_thread(
    pool: &PgPool,
    chat_id: i64,
    message_id: i32,
) -> anyhow::Result<Vec<ChatMessage>> {
    let rows = sqlx::query_as::<_, MessageRow>(r#"
        with recursive ancestors as (
            select m.message_id, m.user_id, m.text, m.reply_to_message_id, m.created_at,
                   m.is_forwarded, m.forwarded_from, 0 as depth
            from mcp_public.telegram_messages m
            where m.chat_id = $1 and m.message_id = $2 and m.deleted_by_bot_at is null and m.spam_marked_at is null
            union all
            select parent.message_id, parent.user_id, parent.text, parent.reply_to_message_id,
                   parent.created_at, parent.is_forwarded, parent.forwarded_from, ancestors.depth + 1
            from mcp_public.telegram_messages parent join ancestors on ancestors.reply_to_message_id = parent.message_id
            where parent.chat_id = $1 and ancestors.depth < 5 and parent.deleted_by_bot_at is null and parent.spam_marked_at is null
        ), descendants as (
            select m.message_id, m.user_id, m.text, m.reply_to_message_id, m.created_at,
                   m.is_forwarded, m.forwarded_from, 0 as depth
            from mcp_public.telegram_messages m
            where m.chat_id = $1 and m.message_id = $2 and m.deleted_by_bot_at is null and m.spam_marked_at is null
            union all
            select child.message_id, child.user_id, child.text, child.reply_to_message_id,
                   child.created_at, child.is_forwarded, child.forwarded_from, descendants.depth + 1
            from descendants
            join lateral (
                select candidate.message_id, candidate.user_id, candidate.text,
                       candidate.reply_to_message_id, candidate.created_at,
                       candidate.is_forwarded, candidate.forwarded_from
                from mcp_public.telegram_messages candidate
                where candidate.chat_id = $1
                  and candidate.reply_to_message_id = descendants.message_id
                  and candidate.deleted_by_bot_at is null
                  and candidate.spam_marked_at is null
                order by candidate.created_at asc, candidate.message_id asc
                limit 5
            ) child on true
            where descendants.depth < 3
        ), thread as (
            select message_id, user_id, text, reply_to_message_id, created_at,
                   is_forwarded, forwarded_from from ancestors
            union
            select message_id, user_id, text, reply_to_message_id, created_at,
                   is_forwarded, forwarded_from from descendants
        )
        select thread.message_id, thread.user_id, coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''), nullif(p.username, ''), 'Неизвестный пользователь') as author,
               nullif(p.username, '') as author_username,
               thread.is_forwarded,
               thread.forwarded_from,
               coalesce(thread.text, '[медиа без текста]') as text, thread.reply_to_message_id, thread.created_at, 0::real as relevance
        from thread left join mcp_public.telegram_user_profiles p on p.telegram_user_id = thread.user_id
        order by thread.created_at asc, thread.message_id asc
        limit 20
    "#).bind(chat_id).bind(message_id).fetch_all(pool).await?;
    Ok(map_rows(chat_id, rows))
}

pub async fn user_interactions(
    pool: &PgPool,
    chat_id: i64,
    first_user_id: i64,
    second_user_id: i64,
    limit: i64,
) -> anyhow::Result<Vec<ChatInteraction>> {
    let rows = sqlx::query_as::<_, InteractionRow>(
        r#"
        select m.message_id, m.user_id,
               coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''),
                        nullif(p.username, ''), 'Неизвестный пользователь') as author,
               nullif(p.username, '') as author_username,
               m.is_forwarded,
               m.forwarded_from,
               coalesce(m.text, '[медиа без текста]') as text,
               m.reply_to_message_id, m.created_at,
               replied.message_id as replied_to_message_id,
               replied.user_id as replied_to_user_id,
               coalesce(nullif(concat_ws(' ', replied_profile.first_name, replied_profile.last_name), ''),
                        nullif(replied_profile.username, ''), 'Неизвестный пользователь') as replied_to_author,
               nullif(replied_profile.username, '') as replied_to_username,
               replied.is_forwarded as replied_to_is_forwarded,
               replied.forwarded_from as replied_to_forwarded_from,
               coalesce(replied.text, '[медиа без текста]') as replied_to_text,
               replied.created_at as replied_to_created_at
        from mcp_public.telegram_messages m
        left join mcp_public.telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join mcp_public.telegram_messages replied
          on replied.chat_id = m.chat_id and replied.message_id = m.reply_to_message_id
        left join mcp_public.telegram_user_profiles replied_profile on replied_profile.telegram_user_id = replied.user_id
        where m.chat_id = $1
          and m.deleted_by_bot_at is null
          and m.spam_marked_at is null
          and ((m.user_id = $2 and m.reply_to_user_id = $3)
            or (m.user_id = $3 and m.reply_to_user_id = $2))
        order by m.created_at desc, m.message_id desc
        limit $4
        "#,
    )
    .bind(chat_id)
    .bind(first_user_id)
    .bind(second_user_id)
    .bind(limit.clamp(1, MAX_RESULT_LIMIT))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let author_name = row.author;
            let message = ChatMessage {
                source_id: source_id(row.message_id),
                message_url: message_url(chat_id, row.message_id),
                relevance: 0,
                message_id: row.message_id,
                user_id: row.user_id,
                author_name: author_name.clone(),
                author: author_name,
                author_url: author_url(row.author_username.as_deref()),
                is_forwarded: row.is_forwarded,
                forwarded_from: row.forwarded_from,
                text: first_chars(&row.text, MAX_MESSAGE_PREVIEW_CHARS),
                reply_to_message_id: row.reply_to_message_id,
                created_at: row.created_at.to_rfc3339(),
            };
            let replied_to = row.replied_to_message_id.map(|message_id| {
                let author_name = row
                    .replied_to_author
                    .unwrap_or_else(|| "Неизвестный пользователь".to_string());
                ChatMessage {
                    source_id: source_id(message_id),
                    message_url: message_url(chat_id, message_id),
                    relevance: 0,
                    message_id,
                    user_id: row.replied_to_user_id,
                    author_name: author_name.clone(),
                    author: author_name,
                    author_url: author_url(row.replied_to_username.as_deref()),
                    is_forwarded: row.replied_to_is_forwarded.unwrap_or(false),
                    forwarded_from: row.replied_to_forwarded_from,
                    text: first_chars(
                        row.replied_to_text
                            .as_deref()
                            .unwrap_or("[медиа без текста]"),
                        MAX_MESSAGE_PREVIEW_CHARS,
                    ),
                    reply_to_message_id: None,
                    created_at: row
                        .replied_to_created_at
                        .map(|value| value.to_rfc3339())
                        .unwrap_or_default(),
                }
            });
            ChatInteraction {
                message,
                replied_to,
            }
        })
        .collect())
}

pub async fn user_profile(
    pool: &PgPool,
    chat_id: i64,
    telegram_user_id: i64,
) -> anyhow::Result<Option<ChatUserProfile>> {
    let mut profile = sqlx::query_as::<_, ChatUserProfile>(
        r#"
        select p.telegram_user_id, nullif(p.username, '') as username,
               coalesce(nullif(concat_ws(' ', p.first_name, p.last_name), ''), nullif(p.username, ''), 'Неизвестный пользователь') as display_name,
               null::text as author_url, nullif(p.bio, '') as bio, p.is_bot, p.is_premium, p.language_code,
               coalesce(cu.message_count, 0) as message_count, coalesce(cu.reply_count, 0) as reply_count,
               1 + (
                   select count(*)
                   from mcp_public.telegram_chat_users ranked
                   left join mcp_public.telegram_user_profiles ranked_profile
                     on ranked_profile.telegram_user_id = ranked.telegram_user_id
                   where ranked.chat_id = $1
                     and not coalesce(ranked_profile.is_bot, false)
                     and ranked.message_count > coalesce(cu.message_count, 0)
               ) as message_rank,
               coalesce(cu.link_count, 0) as link_count, coalesce(cu.media_count, 0) as media_count,
               cu.first_seen_at::text as first_seen_at, cu.last_seen_at::text as last_seen_at,
               cu.member_status, coalesce(cu.is_admin, false) as is_admin,
               member_snapshot.admin_title,
               cu.is_present
        from mcp_public.telegram_user_profiles p
        left join mcp_public.telegram_chat_users cu on cu.chat_id = $1 and cu.telegram_user_id = p.telegram_user_id
        left join mcp_public.telegram_chat_member_snapshots member_snapshot
          on member_snapshot.chat_id = $1 and member_snapshot.telegram_user_id = p.telegram_user_id
        where p.telegram_user_id = $2
          and exists (select 1 from mcp_public.telegram_messages m where m.chat_id = $1 and m.user_id = p.telegram_user_id)
        "#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .fetch_optional(pool)
    .await?;
    if let Some(profile) = profile.as_mut() {
        profile.author_url = author_url(profile.username.as_deref());
    }
    Ok(profile)
}

fn full_text_query(query: &str, mode: &MessageMatch) -> String {
    if matches!(mode, MessageMatch::AnyTerms) {
        query
            .split_whitespace()
            .map(|term| term.to_owned())
            .collect::<Vec<_>>()
            .join(" OR ")
    } else {
        query.to_owned()
    }
}

fn whole_word_pattern(query: &str) -> String {
    format!(
        r"(?i)(^|[^[:alnum:]_]){}($|[^[:alnum:]_])",
        regex_escape(query)
    )
}

fn page_metadata(total_count: i64, offset: i64, message_count: usize) -> (bool, Option<i64>, bool) {
    let offset = offset.clamp(0, MAX_SEARCH_OFFSET);
    let page_end = offset + message_count as i64;
    let has_more = total_count > page_end;
    let scan_limit_reached = has_more && page_end > MAX_SEARCH_OFFSET;
    let next_offset = (has_more && page_end <= MAX_SEARCH_OFFSET).then_some(page_end);
    (has_more, next_offset, scan_limit_reached)
}

fn regex_escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| {
            if r"\.^$|()[]{}*+?".contains(character) {
                vec!['\\', character]
            } else {
                vec![character]
            }
        })
        .collect()
}

fn map_search_page_rows(chat_id: i64, rows: Vec<SearchPageRow>) -> (i64, Vec<ChatMessage>) {
    let total_count = rows.first().map_or(0, |row| row.total_count);
    let messages = rows
        .into_iter()
        .filter_map(|row| {
            let author_name = row.author?;
            Some(ChatMessage {
                source_id: source_id(row.message_id?),
                message_url: message_url(chat_id, row.message_id?),
                relevance: (row.relevance? * 1000.0).round() as i32,
                message_id: row.message_id?,
                user_id: row.user_id,
                author_name: author_name.clone(),
                author: author_name,
                author_url: author_url(row.author_username.as_deref()),
                is_forwarded: row.is_forwarded.unwrap_or(false),
                forwarded_from: row.forwarded_from,
                text: first_chars(&row.text?, MAX_MESSAGE_PREVIEW_CHARS),
                reply_to_message_id: row.reply_to_message_id,
                created_at: row.created_at?.to_rfc3339(),
            })
        })
        .collect();
    (total_count, messages)
}

fn map_rows(chat_id: i64, rows: Vec<MessageRow>) -> Vec<ChatMessage> {
    rows.into_iter()
        .map(|row| {
            let author_name = row.author;
            ChatMessage {
                source_id: source_id(row.message_id),
                message_url: message_url(chat_id, row.message_id),
                relevance: (row.relevance * 1000.0).round() as i32,
                message_id: row.message_id,
                user_id: row.user_id,
                author_name: author_name.clone(),
                author: author_name,
                author_url: author_url(row.author_username.as_deref()),
                is_forwarded: row.is_forwarded,
                forwarded_from: row.forwarded_from,
                text: first_chars(&row.text, MAX_MESSAGE_PREVIEW_CHARS),
                reply_to_message_id: row.reply_to_message_id,
                created_at: row.created_at.to_rfc3339(),
            }
        })
        .collect()
}

pub fn source_id(message_id: i32) -> String {
    format!("chat:{message_id}")
}

pub fn message_url(chat_id: i64, message_id: i32) -> Option<String> {
    let internal_id = chat_id.to_string().strip_prefix("-100")?.to_string();
    Some(format!("https://t.me/c/{internal_id}/{message_id}"))
}

fn author_url(username: Option<&str>) -> Option<String> {
    let username = username?.trim();
    let valid = (5..=32).contains(&username.len())
        && username
            .bytes()
            .all(|character| character.is_ascii_alphanumeric() || character == b'_');
    valid.then(|| format!("https://t.me/{username}"))
}

fn normalized_query(query: &str) -> anyhow::Result<String> {
    let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
    if query.is_empty() {
        anyhow::bail!("message search query must not be empty");
    }
    Ok(first_chars(&query, MAX_QUERY_CHARS))
}

fn first_chars(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        let visible_limit = limit.saturating_sub(1);
        let visible: String = truncated.chars().take(visible_limit).collect();
        format!("{visible}…")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_url_accepts_supergroup_id() {
        assert_eq!(
            message_url(-1001932061163, 42).as_deref(),
            Some("https://t.me/c/1932061163/42")
        );
    }

    #[test]
    fn message_url_rejects_non_supergroup_id() {
        assert_eq!(message_url(-12345, 42), None);
    }

    #[test]
    fn author_url_accepts_telegram_username() {
        assert_eq!(
            author_url(Some("pasha_3060")),
            Some("https://t.me/pasha_3060".to_string())
        );
    }

    #[test]
    fn author_url_rejects_unsafe_username() {
        assert_eq!(author_url(Some("pasha/3060")), None);
    }

    #[test]
    fn normalizes_and_limits_query() {
        assert_eq!(normalized_query("  Rust   MCP ").unwrap(), "Rust MCP");
        assert!(normalized_query(&"x ".repeat(400)).unwrap().chars().count() <= MAX_QUERY_CHARS);
    }

    #[test]
    fn rejects_empty_query() {
        assert!(normalized_query(" \n ").is_err());
    }

    #[test]
    fn any_terms_builds_or_query_for_alternatives() {
        assert_eq!(
            full_text_query("броня защита", &MessageMatch::AnyTerms),
            "броня OR защита"
        );
        assert_eq!(
            full_text_query("броня защита", &MessageMatch::FullText),
            "броня защита"
        );
    }

    #[test]
    fn whole_word_pattern_escapes_regex_metacharacters() {
        let pattern = whole_word_pattern("Rust 1.85");
        assert!(pattern.contains(r"Rust 1\.85"));
        assert!(pattern.starts_with(r"(?i)(^|[^[:alnum:]_])"));
    }

    #[test]
    fn page_metadata_preserves_empty_page_count_and_scan_ceiling() {
        assert_eq!(page_metadata(3, 3, 0), (false, None, false));
        assert_eq!(page_metadata(4, 4, 0), (false, None, false));
        assert_eq!(
            page_metadata(10_001, 9_950, 50),
            (true, Some(10_000), false)
        );
        assert_eq!(page_metadata(10_002, 10_000, 1), (true, None, true));
        assert_eq!(page_metadata(3, 0, 1), (true, Some(1), false));
    }
}
