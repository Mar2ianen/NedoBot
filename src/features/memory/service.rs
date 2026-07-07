use sqlx::PgPool;

use crate::config::Config;
use crate::llm::service::generate_text;
use crate::text::first_text_chars;

#[derive(Debug)]
pub struct MemoryNote {
    pub title: String,
    pub summary: String,
    pub cautions: String,
    pub keywords: Vec<String>,
}

pub async fn load_relevant_memory_notes(
    pool: &PgPool,
    post_text: &str,
) -> anyhow::Result<Vec<MemoryNote>> {
    let post_keywords = extract_keywords(post_text);
    if post_keywords.is_empty() {
        return Ok(Vec::new());
    }

    let rows = sqlx::query_as::<_, (i64, String, String, String, Vec<String>)>(
        r#"
        select id, title, summary, cautions, keywords
        from post_memory_notes
        order by created_at desc
        limit 80
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut scored = rows
        .into_iter()
        .filter_map(|(_id, title, summary, cautions, keywords)| {
            let score = keywords
                .iter()
                .filter(|keyword| post_keywords.contains(keyword))
                .count();

            (score > 0).then_some((
                score,
                MemoryNote {
                    title,
                    summary,
                    cautions,
                    keywords,
                },
            ))
        })
        .collect::<Vec<_>>();

    scored.sort_by(|(left_score, _), (right_score, _)| right_score.cmp(left_score));

    Ok(scored.into_iter().take(5).map(|(_, note)| note).collect())
}

pub async fn remember_post(
    pool: &PgPool,
    config: &Config,
    source_channel_id: i64,
    source_message_id: i32,
    post_text: &str,
) -> anyhow::Result<()> {
    let note_prompt = build_memory_note_prompt(post_text);
    let raw_note = generate_text(
        config,
        &note_prompt,
        None,
        config.memory_llm_temperature,
        config.memory_llm_max_tokens,
    )
    .await?;
    let mut note = parse_memory_note(&raw_note.content, post_text);
    note.keywords = merge_keywords(note.keywords, extract_keywords(post_text));

    if let Some(existing) = find_merge_candidate(pool, &note.keywords).await? {
        let merged = merge_memory_notes(existing, note);
        sqlx::query(
            r#"
            update post_memory_notes
            set title = $2,
                summary = $3,
                cautions = $4,
                keywords = $5,
                raw_note = concat(raw_note, E'\n\n--- merged note ---\n', $6),
                merged_source_posts = merged_source_posts + 1,
                last_source_channel_id = $7,
                last_source_message_id = $8,
                updated_at = now()
            where id = $1
            "#,
        )
        .bind(merged.id)
        .bind(&merged.note.title)
        .bind(&merged.note.summary)
        .bind(&merged.note.cautions)
        .bind(&merged.note.keywords)
        .bind(&raw_note.content)
        .bind(source_channel_id)
        .bind(source_message_id)
        .execute(pool)
        .await?;

        return Ok(());
    }

    sqlx::query(
        r#"
        insert into post_memory_notes
            (source_channel_id, source_message_id, title, summary, cautions, keywords, raw_note, last_source_channel_id, last_source_message_id)
        values ($1, $2, $3, $4, $5, $6, $7, $1, $2)
        on conflict (source_channel_id, source_message_id) do update set
            title = excluded.title,
            summary = excluded.summary,
            cautions = excluded.cautions,
            keywords = excluded.keywords,
            raw_note = excluded.raw_note,
            updated_at = now()
        "#,
    )
    .bind(source_channel_id)
    .bind(source_message_id)
    .bind(&note.title)
    .bind(&note.summary)
    .bind(&note.cautions)
    .bind(&note.keywords)
    .bind(&raw_note.content)
    .execute(pool)
    .await?;

    Ok(())
}

struct MergeCandidate {
    id: i64,
    note: MemoryNote,
    score: usize,
}

async fn find_merge_candidate(
    pool: &PgPool,
    new_keywords: &[String],
) -> anyhow::Result<Option<MergeCandidate>> {
    if new_keywords.is_empty() {
        return Ok(None);
    }

    let rows = sqlx::query_as::<_, (i64, String, String, String, Vec<String>)>(
        r#"
        select id, title, summary, cautions, keywords
        from post_memory_notes
        where keywords && $1
        order by updated_at desc
        limit 30
        "#,
    )
    .bind(new_keywords)
    .fetch_all(pool)
    .await?;

    let mut candidates = rows
        .into_iter()
        .filter_map(|(id, title, summary, cautions, keywords)| {
            let score = keywords
                .iter()
                .filter(|keyword| new_keywords.contains(keyword))
                .count();

            (score >= 3).then_some(MergeCandidate {
                id,
                note: MemoryNote {
                    title,
                    summary,
                    cautions,
                    keywords,
                },
                score,
            })
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| right.score.cmp(&left.score));

    Ok(candidates.into_iter().next())
}

fn merge_memory_notes(existing: MergeCandidate, new_note: MemoryNote) -> MergeCandidate {
    let mut merged_note = MemoryNote {
        title: choose_memory_title(&existing.note.title, &new_note.title),
        summary: merge_text_lines(&existing.note.summary, &new_note.summary, 420),
        cautions: merge_text_lines(&existing.note.cautions, &new_note.cautions, 260),
        keywords: merge_keywords(existing.note.keywords, new_note.keywords),
    };

    if merged_note.cautions.trim().is_empty() {
        merged_note.cautions = "Не делать выводы шире фактов из поста.".to_string();
    }

    MergeCandidate {
        id: existing.id,
        note: merged_note,
        score: existing.score,
    }
}

fn choose_memory_title(existing: &str, new_title: &str) -> String {
    if existing.chars().count() <= 80 {
        existing.to_string()
    } else {
        first_text_chars(new_title, 80)
    }
}

fn merge_text_lines(existing: &str, new_text: &str, limit: usize) -> String {
    let mut parts = Vec::new();
    for part in [existing, new_text]
        .into_iter()
        .flat_map(|text| text.split(['\n', ';']))
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if !parts.iter().any(|saved: &String| saved == part) {
            parts.push(part.to_string());
        }
    }

    first_text_chars(&parts.join("; "), limit)
}

fn build_memory_note_prompt(post_text: &str) -> String {
    format!(
        r#"Сделай короткую заметку памяти для будущих комментариев под техно-новостями.
Не добавляй факты, которых нет в посте. Не пересказывай рекламный хвост. Не пиши стиль комментария.

Формат строго такой:
TITLE: короткая тема до 80 символов
KEYWORDS: 5-10 ключей через запятую, нижний регистр
SUMMARY: 1-2 коротких факта из поста
CAUTIONS: что нельзя утверждать без данных, одной фразой

Пост:
{post_text}"#
    )
}

fn parse_memory_note(raw_note: &str, post_text: &str) -> MemoryNote {
    let title = field_value(raw_note, "TITLE").unwrap_or_else(|| fallback_title(post_text));
    let keywords = field_value(raw_note, "KEYWORDS")
        .map(|value| {
            value
                .split(',')
                .map(normalize_keyword)
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let summary =
        field_value(raw_note, "SUMMARY").unwrap_or_else(|| first_text_chars(post_text, 220));
    let cautions = field_value(raw_note, "CAUTIONS").unwrap_or_default();

    MemoryNote {
        title,
        summary,
        cautions,
        keywords,
    }
}

fn field_value(raw_note: &str, field: &str) -> Option<String> {
    raw_note.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(field)
            .then(|| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn fallback_title(post_text: &str) -> String {
    post_text
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| first_text_chars(line, 80))
        .unwrap_or_else(|| "Без темы".to_string())
}

fn merge_keywords(mut left: Vec<String>, right: Vec<String>) -> Vec<String> {
    for keyword in right {
        if !left.contains(&keyword) {
            left.push(keyword);
        }
    }

    left.truncate(16);
    left
}

pub(crate) fn extract_keywords(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut keywords = Vec::new();

    for phrase in [
        "switch 2",
        "playstation 5 pro",
        "ps5 pro",
        "xbox series",
        "gta 6",
        "rtx 50",
        "radeon",
        "rx 9000",
        "rx 9070",
        "ryzen",
        "windows 10",
        "windows 11",
        "smart access memory",
        "sam",
        "amd",
        "nvidia",
        "intel",
        "apple",
        "microsoft",
        "xbox",
        "playstation",
        "nintendo",
        "драйвер",
        "fps",
        "предзаказ",
        "цена",
        "память",
        "видеокарта",
    ] {
        if keyword_phrase_matches(&lower, phrase) {
            keywords.push(phrase.to_string());
        }
    }

    for token in lower
        .split(|ch: char| !ch.is_alphanumeric())
        .map(normalize_keyword)
        .filter(|token| token.chars().count() >= 4)
    {
        if !is_stop_keyword(&token) && !keywords.contains(&token) {
            keywords.push(token);
        }
    }

    keywords.truncate(24);
    keywords
}

fn keyword_phrase_matches(text: &str, phrase: &str) -> bool {
    if phrase.chars().count() <= 3 && phrase.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return text
            .split(|ch: char| !ch.is_alphanumeric())
            .any(|token| token == phrase);
    }

    text.contains(phrase)
}

fn normalize_keyword(value: impl AsRef<str>) -> String {
    value
        .as_ref()
        .trim()
        .trim_matches(|ch: char| !ch.is_alphanumeric())
        .to_lowercase()
}

fn is_stop_keyword(token: &str) -> bool {
    matches!(
        token,
        "это"
            | "что"
            | "как"
            | "для"
            | "или"
            | "еще"
            | "уже"
            | "если"
            | "также"
            | "которые"
            | "после"
            | "сейчас"
            | "будет"
            | "стало"
            | "стали"
            | "может"
            | "около"
            | "ранее"
            | "не"
            | "на"
            | "по"
            | "из"
            | "под"
            | "над"
            | "без"
            | "при"
            | "все"
            | "the"
            | "and"
            | "with"
            | "from"
    )
}
