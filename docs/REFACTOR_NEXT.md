# Next refactor plan

Рабочая карта для следующего прохода по рефактору. Цель — не переписать проект, а закрыть оставшиеся места, которые снова начнут раздувать `main.rs` и плодить ручной HTML.

## Текущее состояние

Статус на конец прохода:

- `main.rs` сокращён до wiring, dispatcher и маленьких update handlers;
- `telegram/html.rs` добавлен и используется для безопасной сборки Telegram HTML;
- `telegram/render.rs` отвечает за отправку HTML, отключение preview и guard пустых сообщений;
- first-comment pipeline вынесен в `features/first_comment/pipeline.rs`;
- prompt builder вынесен в `features/first_comment/prompt.rs`;
- SQL для `post_comment_jobs`, `llm_generations` и последних комментариев бота вынесен в `features/first_comment/repo.rs`;
- `features/first_comment/render.rs` переведён на `Html`;
- `features/memory/report.rs` переведён на `Html`;
- пользовательские ссылки в stats переведены на `Html::link`;
- добавлены unit-тесты для escaping, link/code/custom emoji и placeholder `{CHAT_LINK}`.

Исторический контекст перед проходом:

После последнего распила проект уже стал нормальным MVP-монолитом:

- `main.rs` в основном отвечает за wiring и first-comment pipeline;
- конфиг и state вынесены;
- Telegram persistence живёт в `db/telegram.rs`;
- команды вынесены в `telegram/command_handler.rs` и `telegram/commands.rs`;
- память разделена на `features/memory/service.rs` и `features/memory/report.rs`;
- статистика ушла в `features/stats/report.rs` и `features/stats/types.rs`;
- first-comment renderer вынесен в `features/first_comment/render.rs`;
- LLM routing живёт в `llm/service.rs`.

Проблемные зоны, которые закрывались этим проходом:

1. HTML собирался вручную в разных местах.
2. First-comment pipeline частично сидел в `main.rs`.
3. SQL для `post_comment_jobs` и последних комментариев бота был рядом с pipeline-кодом.
4. Не было общего слоя для безопасного Telegram HTML и лимитов сообщений.

## Инварианты рефактора

Не менять поведение без отдельного осознанного коммита.

После каждого маленького шага:

```bash
cargo fmt
cargo check
```

Если есть локальная база и env:

```bash
cargo test
```

Коммиты лучше держать маленькими:

```text
refactor: add telegram html builder
refactor: move first comment prompt builder
refactor: extract first comment repository
refactor: extract first comment pipeline
refactor: use html builder in memory report
refactor: use html builder in stats report
```

## Шаг 1. Общий Telegram HTML renderer

Сейчас есть `telegram/render.rs`, но он делает в основном отправку:

```text
send_html
send_html_reply
escape_html
```

Нужен отдельный слой для сборки HTML:

```text
src/telegram/html.rs
```

### Минимальный API

```rust
#[derive(Clone, Debug, Default)]
pub struct Html(String);

impl Html {
    pub fn empty() -> Self;
    pub fn text(value: impl AsRef<str>) -> Self;
    pub fn raw_trusted(value: impl Into<String>) -> Self;
    pub fn bold(value: impl AsRef<str>) -> Self;
    pub fn code(value: impl AsRef<str>) -> Self;
    pub fn link(label: impl AsRef<str>, url: impl AsRef<str>) -> Self;
    pub fn custom_emoji(emoji_id: impl AsRef<str>, fallback: &str) -> Self;

    pub fn push(&mut self, part: impl Into<Html>);
    pub fn line(&mut self, part: impl Into<Html>);
    pub fn blank_line(&mut self);

    pub fn into_string(self) -> String;
    pub fn as_str(&self) -> &str;
}
```

### Helper-функции

```rust
pub fn escape(text: &str) -> String;
pub fn bold(text: impl AsRef<str>) -> Html;
pub fn code(text: impl AsRef<str>) -> Html;
pub fn link(label: impl AsRef<str>, url: impl AsRef<str>) -> Html;
pub fn lines(parts: impl IntoIterator<Item = Html>) -> Html;
pub fn paragraphs(parts: impl IntoIterator<Item = Html>) -> Html;
```

### Важное правило безопасности

Все публичные конструкторы должны экранировать пользовательский или модельный текст.

```rust
Html::text(user_input)       // escape
Html::bold(user_input)       // <b>escaped</b>
Html::code(user_input)       // <code>escaped</code>
Html::link(label, url)       // escaped label + escaped url
Html::custom_emoji(id, "😎") // escaped id
```

`raw_trusted` использовать только для HTML, который уже собран кодом и не содержит неэкранированного внешнего текста. Лучше сделать его `pub(crate)`, если получится без боли.

### Лимиты Telegram

В этот же модуль можно положить константы:

```rust
pub const TELEGRAM_TEXT_LIMIT: usize = 4096;
pub const SAFE_TEXT_LIMIT: usize = 3900;
```

И минимальные helpers:

```rust
pub fn is_safe_len(html: &str) -> bool {
    html.chars().count() <= SAFE_TEXT_LIMIT
}

pub fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    text.chars().take(max_chars.saturating_sub(1)).collect::<String>() + "…"
}
```

Полноценный splitter по HTML/entities пока не нужен. Для voice transcription позже понадобится более строгий renderer, но сейчас достаточно безопасного лимита и fallback.

### send_html guard

В `telegram/render.rs` стоит добавить защиту от пустых сообщений:

```rust
pub async fn send_html(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    text: impl Into<String>,
) -> ResponseResult<Message> {
    let text = text.into();
    let text = if text.trim().is_empty() {
        "Пустой ответ.".to_string()
    } else {
        text
    };

    bot.send_message(chat_id, text)
        .link_preview_options(disabled_link_preview())
        .await
}
```

И вынести общий `disabled_link_preview()`.

## Шаг 2. Перевести first-comment renderer на Html

Файл:

```text
src/features/first_comment/render.rs
```

Сейчас там вручную собираются:

- `<tg-emoji>`;
- `<a href="...">`;
- escaping через `escape_html`;
- fallback-ссылка на чат.

После `telegram/html.rs` сделать так, чтобы `build_comment_html` возвращал `String`, но внутри пользовался `Html`.

Примерная форма:

```rust
pub fn build_comment_html(llm_body: &str, config: &Config) -> String {
    let clean_body = normalize_ai_markers(&strip_links(llm_body)).trim().to_string();
    if clean_body.is_empty() {
        return String::new();
    }

    let body = render_chat_link_placeholder(&clean_body, config);

    match pick_comment_emoji(llm_body, config) {
        Some(custom_emoji_id) => {
            let mut html = Html::empty();
            html.push(Html::custom_emoji(custom_emoji_id, "😎"));
            html.push(Html::raw_trusted(" "));
            html.push(body);
            html.into_string()
        }
        None => body.into_string(),
    }
}
```

`render_chat_link_placeholder` лучше вернуть `Html`, а не `String`.

Проверить кейсы:

- LLM вернул `{CHAT_LINK}`;
- LLM не вернул `{CHAT_LINK}`;
- LLM вернул ссылку, которую `strip_links` должен удалить;
- LLM вернул `<b>сырой html</b>` — это должно стать текстом, а не HTML;
- custom emoji id пустой или отсутствует.

## Шаг 3. Перевести memory report на Html

Файл:

```text
src/features/memory/report.rs
```

Сейчас:

```rust
format!(
    "<b>{}</b>\n{}\n<code>{}</code>",
    escape_html(&title),
    escape_html(&summary),
    escape_html(&keywords)
)
```

Должно стать примерно:

```rust
let text = Html::paragraphs(notes.into_iter().map(|(title, summary, keywords)| {
    Html::lines([
        Html::bold(title),
        Html::text(summary),
        Html::code(keywords),
    ])
}))
.into_string();
```

Цель: убрать ручной HTML-format из report-кода.

## Шаг 4. Перевести stats report на Html постепенно

Файл:

```text
src/features/stats/report.rs
```

Там большой отчёт, поэтому не надо сразу переписывать весь файл красивым builder-ом.

Приоритет:

1. `UserPresentation::linked_name()` должен использовать `Html::link`.
2. Ошибка `Не нашёл пользователя <code>...</code>` должна использовать `Html::code`.
3. `top_bot_comments_for_period` должен использовать `Html::text`/`escape` через общий API.
4. Большой `format!` в `build_chat_stats_report` можно оставить до отдельного коммита, если переписывание начинает расползаться.

Хорошая цель на один коммит: убрать прямой импорт `escape_html` из `stats/types.rs` и заменить его на `telegram::html`.

## Шаг 5. Вынести first-comment prompt

Сейчас prompt builder всё ещё в `main.rs`:

```text
build_llm_prompt
render_memory_context
render_recent_comment_context
load_recent_bot_comments
strip_html_tags
```

Нужно разрезать:

```text
src/features/first_comment/prompt.rs
```

Туда:

```rust
pub fn build_llm_prompt(
    post_text: &str,
    chat_member_count: Option<u32>,
    memory_notes: &[MemoryNote],
    recent_comments: &[String],
) -> String
```

И приватные helpers:

```rust
fn render_memory_context(memory_notes: &[MemoryNote]) -> String;
fn render_recent_comment_context(recent_comments: &[String]) -> String;
fn strip_html_tags(text: &str) -> String;
```

`strip_html_tags` можно потом перенести в `text.rs`, но сейчас пусть живёт рядом с prompt, потому что это именно подготовка контекста для модели.

## Шаг 6. Вынести first-comment repository

Файл:

```text
src/features/first_comment/repo.rs
```

Туда вынести SQL, связанный именно с первым комментарием:

```rust
pub async fn create_post_comment_job(
    pool: &PgPool,
    discussion_chat_id: i64,
    discussion_message_id: i32,
    source_channel_id: i64,
    source_message_id: i32,
    cleaned_post_text: &str,
) -> anyhow::Result<Option<i64>>;

pub async fn mark_post_comment_sent(
    pool: &PgPool,
    job_id: i64,
    bot_comment_message_id: i32,
) -> anyhow::Result<()>;

pub async fn insert_llm_generation(
    pool: &PgPool,
    generation: LlmGenerationInsert<'_>,
) -> anyhow::Result<()>;

pub async fn load_recent_bot_comments(pool: &PgPool) -> anyhow::Result<Vec<String>>;
```

Тип для insert:

```rust
pub struct LlmGenerationInsert<'a> {
    pub job_id: i64,
    pub provider: &'a str,
    pub model: &'a str,
    pub prompt: &'a str,
    pub image_used: bool,
    pub response: &'a str,
    pub final_html: &'a str,
}
```

После этого в pipeline не будет сырого SQL для `post_comment_jobs` и `llm_generations`.

## Шаг 7. Вынести first-comment pipeline

Файл:

```text
src/features/first_comment/pipeline.rs
```

Туда перенести из `main.rs`:

```rust
pub async fn maybe_comment_post(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
) -> anyhow::Result<()>;
```

И helpers:

```rust
fn owner_preview_chat(config: &Config) -> Option<i64>;
async fn send_owner_preview(...);
async fn download_largest_photo_base64(...);
async fn get_chat_member_count(...);
```

После этого `main.rs` должен стать примерно таким:

```rust
async fn handle_message(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    msg: Message,
    state: AppState,
) -> ResponseResult<()> {
    if let Err(err) = maybe_comment_post(&bot, &msg, &state).await {
        tracing::error!(%err, "failed to process message");
    }

    Ok(())
}
```

И `main.rs` больше не должен импортировать:

```text
base64
PgPool
MessageId
Download
RequesterExt
MemoryNote
build_comment_html
generate_text
strip_links
normalize_ai_markers
```

Если после выноса эти импорты исчезли — разрез получился хорошим.

## Шаг 8. Подготовить место под voice transcription

Не реализовывать voice в этом проходе. Только не мешать будущей фиче.

Будущая структура:

```text
src/features/voice/
  mod.rs
  pipeline.rs
  asr.rs
  render.rs
  types.rs
```

Renderer из `telegram/html.rs` должен быть достаточно общим, чтобы voice потом мог сделать:

```rust
RenderedTranscript::Message(Html)
RenderedTranscript::MessageAndFile { preview: Html, full_text: String }
```

Для Telegram limit сейчас достаточно `SAFE_TEXT_LIMIT`. Полный chunking можно отложить до voice PR.

## Шаг 9. Что не делать завтра

Не тащить сейчас:

- async actors;
- workspace;
- axum;
- maud/askama;
- pgvector;
- полноценный moderation engine;
- импорт Telegram Desktop export;
- voice transcription.

Задача завтрашнего прохода — закрыть архитектурные швы, а не добавить новую большую фичу.

## Финальная цель прохода

После завершения:

```text
src/main.rs
```

должен быть только про:

- init tracing;
- env/config;
- pool/migrations;
- refresh member snapshots;
- dispatcher wiring;
- маленькие handlers, которые делегируют в modules.

Желаемое состояние:

```text
src/
  main.rs
  config.rs
  state.rs
  text.rs
  db/
    mod.rs
    telegram.rs
  telegram/
    mod.rs
    commands.rs
    command_handler.rs
    custom_emoji.rs
    entities.rs
    html.rs
    render.rs
  llm/
    mod.rs
    service.rs
    types.rs
    ollama.rs
    openai_compat.rs
  features/
    first_comment/
      mod.rs
      candidate.rs
      clean.rs
      prompt.rs
      render.rs
      repo.rs
      pipeline.rs
    memory/
      mod.rs
      service.rs
      report.rs
    stats/
      mod.rs
      types.rs
      report.rs
```

Контрольный вопрос после прохода:

> Можно ли добавить `features/voice` без правок на 300 строк в `main.rs`?

Если да — рефактор достиг цели.

## Возможные тесты

Минимальные unit-тесты, которые стоит добавить после распила:

```text
telegram::html::escape
telegram::html::link
telegram::html::code
first_comment::clean::clean_post_for_llm
first_comment::render::build_comment_html with placeholder
first_comment::render::build_comment_html without placeholder
llm::service::normalize_provider
text::strip_links
```

Особенно важны тесты на HTML escaping:

```rust
assert_eq!(Html::text("<b>x</b>").into_string(), "&lt;b&gt;x&lt;/b&gt;");
```

И на placeholder:

```rust
let html = build_comment_html("залетайте в {CHAT_LINK}", &config);
assert!(html.contains("<a href="));
assert!(!html.contains("{CHAT_LINK}"));
```

## Порядок выполнения на завтра

Оптимальный порядок:

1. [x] `telegram/html.rs` + `pub mod html`.
2. [x] Guard от пустого текста в `telegram/render.rs`.
3. [x] Перевести `first_comment/render.rs` на `Html`.
4. [x] Перевести `memory/report.rs` на `Html`.
5. [x] Частично перевести `stats/types.rs` и самые простые места `stats/report.rs`.
6. [x] Вынести `first_comment/prompt.rs`.
7. [x] Вынести `first_comment/repo.rs`.
8. [x] Вынести `first_comment/pipeline.rs`.
9. [x] `cargo fmt && cargo check`.
10. Только после этого думать о voice/import.
