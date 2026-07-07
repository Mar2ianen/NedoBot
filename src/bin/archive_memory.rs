use std::collections::{HashMap, HashSet};

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
    let groups = build_archive_groups(&config, notes, args.min_overlap).await?;
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

async fn build_archive_groups(
    config: &Config,
    notes: Vec<MemoryRow>,
    min_overlap: usize,
) -> anyhow::Result<Vec<ArchiveGroup>> {
    let by_id = notes
        .iter()
        .cloned()
        .map(|note| (note.id, note))
        .collect::<HashMap<_, _>>();

    let mut groups = match plan_archive_groups_with_llm(config, &notes).await {
        Ok(plan) => materialize_llm_plan(plan, &by_id),
        Err(err) => {
            tracing::warn!(%err, "LLM archive planner failed, falling back to keyword grouping");
            heuristic_archive_groups(notes.clone(), min_overlap)
        }
    };

    let used_ids = groups
        .iter()
        .flat_map(|group| {
            std::iter::once(group.keeper.id).chain(group.merged.iter().map(|note| note.id))
        })
        .collect::<HashSet<_>>();
    groups.extend(bloated_single_groups(notes, &used_ids));

    Ok(groups)
}

#[derive(Debug)]
struct ArchivePlanGroup {
    keeper_id: i64,
    merge_ids: Vec<i64>,
}

async fn plan_archive_groups_with_llm(
    config: &Config,
    notes: &[MemoryRow],
) -> anyhow::Result<Vec<ArchivePlanGroup>> {
    if notes.is_empty() {
        return Ok(Vec::new());
    }

    let prompt = build_archive_plan_prompt(notes);
    let generated = generate_text(
        config,
        &prompt,
        None,
        0.1,
        config.memory_llm_max_tokens.max(700),
    )
    .await?;

    Ok(parse_archive_plan(&generated.content))
}

fn build_archive_plan_prompt(notes: &[MemoryRow]) -> String {
    let notes = notes
        .iter()
        .map(|note| {
            format!(
                "ID: {}\nMERGED_POSTS: {}\nTITLE: {}\nKEYWORDS: {}\nSUMMARY: {}\nCAUTIONS: {}",
                note.id,
                note.merged_source_posts,
                note.title,
                note.keywords.join(", "),
                note.summary,
                note.cautions
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    format!(
        r#"Ты LLM-архивариус RAG-памяти техно-новостей НедоNews.
Твоя задача: найти заметки, которые стоит реструктурировать: слить в одну устойчивую RAG-запись или оставить отдельно.

Группируй только если заметки реально про одну долгоживущую тему, повторяющийся факт/тренд или одну цепочку событий.
Не группируй просто потому, что совпал бренд: Intel, Microsoft, Sony, Nvidia и т.п.
Не группируй разные новости о памяти, если это разные рынки/компании/технологии и из них нельзя сделать одну аккуратную заметку.
Если MERGED_POSTS больше 3, это уже широкий архив: расширяй его только прямым продолжением той же темы, а не соседней темой.
Не сливай общий архив цен/дефицита памяти с отдельными технологиями вроде XBM, CXMT, EUV, HBM-инвестиций, если посты не описывают один причинный ряд.
Не сливай стратегию Sony по цифровым носителям с общей конкуренцией ПК и консолей, если заметка не про физические носители/дисковод/цифровые лицензии.
Лучше вернуть меньше групп, чем создать слишком широкий архив.
Не выдумывай ID. Используй только ID из списка.
Если хороших групп нет, верни только NO_GROUPS.

Формат ответа строго построчный:
GROUP: keeper_id=<ID> merge_ids=<ID,ID,...>

Где keeper_id — лучшая базовая заметка, merge_ids — заметки, которые надо удалить после слияния в keeper.
Не добавляй объяснений.

Заметки:
{notes}"#
    )
}

fn parse_archive_plan(raw: &str) -> Vec<ArchivePlanGroup> {
    raw.lines()
        .filter_map(parse_archive_plan_line)
        .collect::<Vec<_>>()
}

fn parse_archive_plan_line(line: &str) -> Option<ArchivePlanGroup> {
    let line = line.trim();
    if line.eq_ignore_ascii_case("NO_GROUPS") || !line.starts_with("GROUP:") {
        return None;
    }

    let keeper_id = field_after(line, "keeper_id=")?.parse().ok()?;
    let merge_ids = field_after(line, "merge_ids=")?
        .split(',')
        .filter_map(|value| value.trim().parse::<i64>().ok())
        .filter(|id| *id != keeper_id)
        .collect::<Vec<_>>();

    (!merge_ids.is_empty()).then_some(ArchivePlanGroup {
        keeper_id,
        merge_ids,
    })
}

fn field_after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let start = line.find(marker)? + marker.len();
    let rest = &line[start..];
    let end = rest.find(' ').unwrap_or(rest.len());
    Some(rest[..end].trim())
}

fn materialize_llm_plan(
    plan: Vec<ArchivePlanGroup>,
    by_id: &HashMap<i64, MemoryRow>,
) -> Vec<ArchiveGroup> {
    let mut used_ids = HashSet::new();
    let mut groups = Vec::new();

    for planned in plan {
        if used_ids.contains(&planned.keeper_id) {
            continue;
        }
        let Some(keeper) = by_id.get(&planned.keeper_id).cloned() else {
            continue;
        };

        let mut merged = Vec::new();
        for merge_id in planned.merge_ids {
            if used_ids.contains(&merge_id) {
                continue;
            }
            if let Some(note) = by_id.get(&merge_id).cloned() {
                used_ids.insert(merge_id);
                merged.push(note);
            }
        }

        if merged.is_empty() {
            continue;
        }

        used_ids.insert(keeper.id);
        groups.push(ArchiveGroup { keeper, merged });
    }

    groups
}

fn heuristic_archive_groups(notes: Vec<MemoryRow>, min_overlap: usize) -> Vec<ArchiveGroup> {
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

        if !merged.is_empty() {
            groups.push(ArchiveGroup { keeper, merged });
        }

        remaining = rest;
    }

    groups
}

fn bloated_single_groups(notes: Vec<MemoryRow>, used_ids: &HashSet<i64>) -> Vec<ArchiveGroup> {
    notes
        .into_iter()
        .filter(|note| !used_ids.contains(&note.id))
        .filter(|note| {
            note.summary.chars().count() > MAX_ARCHIVED_SUMMARY_CHARS
                || note.cautions.chars().count() > MAX_ARCHIVED_CAUTIONS_CHARS
        })
        .map(|keeper| ArchiveGroup {
            keeper,
            merged: Vec::new(),
        })
        .collect()
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
