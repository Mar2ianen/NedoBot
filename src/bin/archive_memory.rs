use anyhow::Context;
use sqlx::PgPool;
use tg_ai_bot_teloxide::{
    config::Config,
    db::{build_pool, migrate},
    llm::service::generate_text,
};

const DEFAULT_LIMIT: i64 = 80;
const DEFAULT_MIN_OVERLAP: usize = 4;
const MAX_ARCHIVED_SUMMARY_CHARS: usize = 360;
const MAX_ARCHIVED_CAUTIONS_CHARS: usize = 180;
const MAX_ARCHIVED_KEYWORDS: usize = 12;

#[derive(Debug)]
struct Args {
    limit: i64,
    min_overlap: usize,
    apply: bool,
}

#[derive(Clone, Debug)]
struct MemoryRow {
    id: i64,
    title: String,
    summary: String,
    cautions: String,
    keywords: Vec<String>,
    merged_source_posts: i32,
    source_channel_id: i64,
    source_message_id: i32,
    last_source_channel_id: Option<i64>,
    last_source_message_id: Option<i32>,
}

#[derive(Debug)]
struct ArchiveGroup {
    keeper: MemoryRow,
    merged: Vec<MemoryRow>,
}

#[derive(Debug)]
struct ArchivedNote {
    title: String,
    summary: String,
    cautions: String,
    keywords: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let args = parse_args()?;
    let config = Config::from_env();
    config.validate_runtime_secrets()?;
    let pool = build_pool().await?;
    migrate(&pool).await?;

    let notes = load_memory_notes(&pool, args.limit).await?;
    let groups = build_archive_groups(notes, args.min_overlap);
    println!(
        "archive groups: {} mode={}",
        groups.len(),
        if args.apply { "apply" } else { "dry-run" }
    );

    for group in groups {
        archive_group(&pool, &config, &group, args.apply).await?;
    }

    Ok(())
}

fn parse_args() -> anyhow::Result<Args> {
    let mut limit = DEFAULT_LIMIT;
    let mut min_overlap = DEFAULT_MIN_OVERLAP;
    let mut apply = false;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--limit" => {
                limit = args
                    .next()
                    .context("--limit requires value")?
                    .parse()
                    .context("invalid --limit")?;
            }
            "--min-overlap" => {
                min_overlap = args
                    .next()
                    .context("--min-overlap requires value")?
                    .parse()
                    .context("invalid --min-overlap")?;
            }
            "--apply" => apply = true,
            "-h" | "--help" => {
                println!(
                    "Usage: archive_memory [--limit 80] [--min-overlap 4] [--apply]\n\nDefault mode is dry-run. Use --apply to update post_memory_notes."
                );
                std::process::exit(0);
            }
            _ => anyhow::bail!("unknown option: {arg}"),
        }
    }

    Ok(Args {
        limit,
        min_overlap,
        apply,
    })
}

async fn load_memory_notes(pool: &PgPool, limit: i64) -> anyhow::Result<Vec<MemoryRow>> {
    let rows = sqlx::query_as::<
        _,
        (
            i64,
            String,
            String,
            String,
            Vec<String>,
            i32,
            i64,
            i32,
            Option<i64>,
            Option<i32>,
        ),
    >(
        r#"
        select id,
               title,
               summary,
               cautions,
               keywords,
               merged_source_posts,
               source_channel_id,
               source_message_id,
               last_source_channel_id,
               last_source_message_id
        from post_memory_notes
        order by updated_at desc
        limit $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                title,
                summary,
                cautions,
                keywords,
                merged_source_posts,
                source_channel_id,
                source_message_id,
                last_source_channel_id,
                last_source_message_id,
            )| MemoryRow {
                id,
                title,
                summary,
                cautions,
                keywords,
                merged_source_posts,
                source_channel_id,
                source_message_id,
                last_source_channel_id,
                last_source_message_id,
            },
        )
        .collect())
}

fn build_archive_groups(notes: Vec<MemoryRow>, min_overlap: usize) -> Vec<ArchiveGroup> {
    let mut remaining = notes;
    let mut groups = Vec::new();

    while let Some(keeper) = remaining.pop() {
        let mut merged = Vec::new();
        let mut rest = Vec::new();

        for note in remaining {
            if keyword_overlap(&keeper.keywords, &note.keywords) >= min_overlap {
                merged.push(note);
            } else {
                rest.push(note);
            }
        }

        let keeper_needs_compaction = keeper.summary.chars().count() > MAX_ARCHIVED_SUMMARY_CHARS
            || keeper.cautions.chars().count() > MAX_ARCHIVED_CAUTIONS_CHARS
            || keeper.keywords.len() > MAX_ARCHIVED_KEYWORDS;

        if !merged.is_empty() || keeper_needs_compaction {
            groups.push(ArchiveGroup { keeper, merged });
        }

        remaining = rest;
    }

    groups
}

async fn archive_group(
    pool: &PgPool,
    config: &Config,
    group: &ArchiveGroup,
    apply: bool,
) -> anyhow::Result<()> {
    let prompt = build_archive_prompt(group);
    let generated = generate_text(
        config,
        &prompt,
        None,
        config.memory_llm_temperature,
        config.memory_llm_max_tokens,
    )
    .await?;
    let mut archived = parse_archived_note(&generated.content, group);
    archived.keywords = compact_keywords(archived.keywords, group);

    let merged_ids = group.merged.iter().map(|note| note.id).collect::<Vec<_>>();
    println!(
        "keeper={} merge={:?} title={} summary_len={} cautions_len={} keywords={}",
        group.keeper.id,
        merged_ids,
        archived.title,
        archived.summary.chars().count(),
        archived.cautions.chars().count(),
        archived.keywords.len()
    );

    if !apply {
        return Ok(());
    }

    let merged_source_posts = group.keeper.merged_source_posts
        + group
            .merged
            .iter()
            .map(|note| note.merged_source_posts)
            .sum::<i32>();
    let last_source_channel_id = group
        .merged
        .first()
        .and_then(|note| note.last_source_channel_id)
        .or(group.keeper.last_source_channel_id)
        .unwrap_or(group.keeper.source_channel_id);
    let last_source_message_id = group
        .merged
        .first()
        .and_then(|note| note.last_source_message_id)
        .or(group.keeper.last_source_message_id)
        .unwrap_or(group.keeper.source_message_id);

    sqlx::query(
        r#"
        update post_memory_notes
        set title = $2,
            summary = $3,
            cautions = $4,
            keywords = $5,
            raw_note = concat(raw_note, E'\n\n--- archived note ---\n', $6),
            merged_source_posts = $7,
            last_source_channel_id = $8,
            last_source_message_id = $9,
            updated_at = now()
        where id = $1
        "#,
    )
    .bind(group.keeper.id)
    .bind(&archived.title)
    .bind(&archived.summary)
    .bind(&archived.cautions)
    .bind(&archived.keywords)
    .bind(&generated.content)
    .bind(merged_source_posts)
    .bind(last_source_channel_id)
    .bind(last_source_message_id)
    .execute(pool)
    .await?;

    if !merged_ids.is_empty() {
        sqlx::query(
            r#"
            delete from post_memory_notes
            where id = any($1)
            "#,
        )
        .bind(&merged_ids)
        .execute(pool)
        .await?;
    }

    Ok(())
}

fn build_archive_prompt(group: &ArchiveGroup) -> String {
    let notes = std::iter::once(&group.keeper)
        .chain(group.merged.iter())
        .map(|note| {
            format!(
                "ID: {}\nTITLE: {}\nKEYWORDS: {}\nSUMMARY: {}\nCAUTIONS: {}",
                note.id,
                note.title,
                note.keywords.join(", "),
                note.summary,
                note.cautions
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    format!(
        r#"Ты архивариус памяти техно-новостей НедоNews.
Сожми похожие заметки в одну компактную RAG-запись для будущих коротких комментариев.

Правила:
- не добавляй новых фактов;
- убери повторы, рекламные формулировки и общий шум;
- оставь только факты, которые помогут не ошибиться в будущем;
- CAUTIONS важнее сарказма: запрети чрезмерные выводы шире фактов;
- не делай общий вывод о смерти/крахе компании, если этого нет в заметках;
- SUMMARY до {MAX_ARCHIVED_SUMMARY_CHARS} символов;
- CAUTIONS до {MAX_ARCHIVED_CAUTIONS_CHARS} символов;
- KEYWORDS 6-12 коротких ключей, нижний регистр, через запятую.

Формат строго такой:
TITLE: короткая тема до 80 символов
KEYWORDS: ключи через запятую
SUMMARY: компактные факты
CAUTIONS: что нельзя утверждать без данных

Заметки:
{notes}"#
    )
}

fn parse_archived_note(raw: &str, group: &ArchiveGroup) -> ArchivedNote {
    ArchivedNote {
        title: field_value(raw, "TITLE").unwrap_or_else(|| group.keeper.title.clone()),
        summary: field_value(raw, "SUMMARY")
            .unwrap_or_else(|| truncate_chars(&group.keeper.summary, MAX_ARCHIVED_SUMMARY_CHARS)),
        cautions: field_value(raw, "CAUTIONS")
            .unwrap_or_else(|| "Не делать выводы шире фактов из исходных заметок.".to_string()),
        keywords: field_value(raw, "KEYWORDS")
            .map(|value| value.split(',').map(normalize_keyword).collect())
            .unwrap_or_default(),
    }
}

fn field_value(raw: &str, field: &str) -> Option<String> {
    raw.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(field)
            .then(|| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn compact_keywords(mut generated: Vec<String>, group: &ArchiveGroup) -> Vec<String> {
    generated.retain(|keyword| !keyword.is_empty());

    for keyword in std::iter::once(&group.keeper)
        .chain(group.merged.iter())
        .flat_map(|note| note.keywords.iter())
        .map(normalize_keyword)
    {
        if !keyword.is_empty() && !generated.contains(&keyword) {
            generated.push(keyword);
        }
    }

    generated.truncate(MAX_ARCHIVED_KEYWORDS);
    generated
}

fn keyword_overlap(left: &[String], right: &[String]) -> usize {
    left.iter()
        .filter(|keyword| right.contains(keyword))
        .count()
}

fn normalize_keyword(value: impl AsRef<str>) -> String {
    value
        .as_ref()
        .trim()
        .trim_matches(|ch: char| !ch.is_alphanumeric())
        .to_lowercase()
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}
