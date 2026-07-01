# Voice transcription integration plan

Активный следующий проход: добавить расшифровку голосовых сообщений в бота без возврата к огромному `main.rs`.

Уже закрытый рефактор вынесен в [`REFACTOR_DONE.md`](REFACTOR_DONE.md). В этом файле остаётся только план новой фичи.

## Цель

Бот должен уметь принимать `voice`, `audio` и позже `video_note`, получать расшифровку через Groq Whisper, прогонять результат через LLM-cleanup и отвечать в чат аккуратным Telegram HTML.

Желаемый результат для пользователя:

```text
Расшифровка голосового

0:00 Начали обсуждать драйверы AMD и почему опять всё отвалилось.
0:28 Перешли к Linux/Proton и спору, насколько это массовая проблема.
1:10 Итог: надо проверить версию Mesa и свежие багрепорты.
```

Если текст не влезает в безопасный лимит Telegram, бот отправляет короткий preview в чат и полный transcript файлом.

## Внешние ограничения

Telegram `sendMessage` принимает 0-4096 символов после entities parsing. В коде уже есть hard-limit `TELEGRAM_TEXT_LIMIT = 4096` и safe warning около `3900`. Для voice нельзя просто надеяться, что LLM уложится в лимит: renderer обязан сам выбирать single message, chunks или file fallback.

Обычный cloud Bot API через `getFile` скачивает файлы до 20 MB. Для больших аудио нужен local Bot API server, где файл доступен локальным путём. На MVP держать консервативный лимит и явно писать пользователю, что файл слишком большой.

Groq Speech-to-Text использует OpenAI-compatible endpoint:

```text
POST https://api.groq.com/openai/v1/audio/transcriptions
```

Для MVP:

```text
model = whisper-large-v3-turbo
response_format = verbose_json
timestamp_granularities[] = segment
language = ru
temperature = 0
```

`whisper-large-v3` оставить как более точный, но более дорогой/медленный fallback. Groq сейчас поддерживает `flac`, `mp3`, `mp4`, `mpeg`, `mpga`, `m4a`, `ogg`, `wav`, `webm`; Telegram voice обычно `ogg/opus`, поэтому первый MVP может обойтись без ffmpeg для обычных ГС.

Ссылки для проверки перед деплоем:

- Groq Speech-to-Text: <https://console.groq.com/docs/speech-to-text>
- Groq API reference: <https://console.groq.com/docs/api-reference>
- Telegram Bot API `getFile`: <https://core.telegram.org/bots/api#getfile>
- Telegram Bot API `sendMessage`: <https://core.telegram.org/bots/api#sendmessage>

## UX policy

Рекомендуемый MVP:

- Автоматически расшифровывать `voice` в `DISCUSSION_CHAT_ID`, если `VOICE_AUTO_TRANSCRIBE=true`.
- Не трогать сообщения бота, команды и auto-forward посты канала.
- Не расшифровывать файлы больше `VOICE_MAX_FILE_MB` и дольше `VOICE_MAX_DURATION_SEC`.
- На слишком длинное или большое аудио отвечать коротким HTML-сообщением с причиной отказа.
- Для `audio` включить поддержку после `voice`: там чаще бывают длинные файлы, музыка и мусор.
- Для `video_note` включить третьим шагом: нужно решить, хотим ли выдирать аудио через ffmpeg.

Опциональная ручная команда позже:

```text
/transcribe
```

Она полезна как безопасный режим, если auto-transcribe начнёт шуметь. Команду можно сделать reply-only: пользователь отвечает `/transcribe` на voice/audio, бот расшифровывает именно reply message.

## Архитектура

Новая структура:

```text
src/features/voice/
  mod.rs
  pipeline.rs
  types.rs
  download.rs
  asr.rs
  cleanup.rs
  render.rs
  repo.rs

prompts/voice_cleanup.md
```

Ответственность модулей:

| Модуль | Ответственность |
| --- | --- |
| `pipeline.rs` | Оркестрация: определить media, создать job, скачать, ASR, cleanup, render, send, mark status. |
| `types.rs` | `VoiceMedia`, `AsrSegment`, `AsrTranscript`, `CleanTranscript`, `RenderedTranscript`. |
| `download.rs` | `getFile`, проверка размера, скачивание в temp path, cleanup temp files. |
| `asr.rs` | Groq multipart request, парсинг `verbose_json`, нормализация timestamps. |
| `cleanup.rs` | LLM prompt для исправления ASR и разбивки на смысловые фрагменты. |
| `render.rs` | Telegram HTML preview, file body, fallback для длинного текста. |
| `repo.rs` | SQL для `voice_transcription_jobs`. |

`main.rs` должен получить только один новый делегат в `handle_message`:

```rust
if maybe_transcribe_voice(&bot, &msg, &state).await? {
    return Ok(());
}
```

Порядок в `handle_message`:

1. reply-only command hacks, которые уже есть;
2. voice transcription;
3. first-comment pipeline.

Voice не должен мешать first-comment pipeline: auto-forward посты из канала не являются `voice`, а обычные voice/audio не являются first-comment candidates.

## Config

Добавить в `Config` и `.env.example`:

```env
VOICE_TRANSCRIPTION_ENABLED=true
VOICE_AUTO_TRANSCRIBE=true
VOICE_MAX_DURATION_SEC=600
VOICE_MAX_FILE_MB=20
VOICE_LANGUAGE=ru
VOICE_ASR_PROVIDER=groq
VOICE_ASR_MODEL=whisper-large-v3-turbo
VOICE_ASR_TEMPERATURE=0
VOICE_CLEANUP_PROVIDER=
VOICE_CLEANUP_MODEL=
VOICE_CLEANUP_TEMPERATURE=0.2
VOICE_CLEANUP_MAX_TOKENS=1800
VOICE_SEND_FULL_FILE=true
```

Правила:

- `VOICE_CLEANUP_PROVIDER` пустой значит использовать обычный `LLM_PROVIDER`.
- `VOICE_CLEANUP_MODEL` пустой значит использовать обычную модель provider-а.
- `VOICE_MAX_FILE_MB=20` выбран из-за cloud Bot API `getFile`; если поднимешь local Bot API server, можно увеличивать отдельно.
- `VOICE_SEND_FULL_FILE=true` значит длинная расшифровка уходит preview + `.md`/`.txt` файлом.

## Database

Добавить миграцию без Postgres enum, чтобы проще менять статусы:

```sql
create table voice_transcription_jobs (
    id bigserial primary key,
    chat_id bigint not null,
    message_id integer not null,
    user_id bigint,
    file_id text not null,
    file_unique_id text,
    media_kind text not null,
    duration_sec integer,
    file_size bigint,
    mime_type text,
    status text not null default 'pending',
    error text,
    asr_provider text,
    asr_model text,
    asr_request_id text,
    cleanup_provider text,
    cleanup_model text,
    raw_transcript text,
    cleaned_text text,
    segments_json jsonb,
    raw_asr_json jsonb,
    final_html text,
    full_text_file_id text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (chat_id, message_id),
    check (status in ('pending', 'downloading', 'transcribing', 'cleaning', 'sent', 'failed', 'skipped'))
);

create index voice_transcription_jobs_status_idx on voice_transcription_jobs(status);
create index voice_transcription_jobs_created_at_idx on voice_transcription_jobs(created_at desc);
```

`repo.rs` API:

```rust
pub async fn create_voice_job(
    pool: &PgPool,
    media: &VoiceMedia,
) -> anyhow::Result<Option<i64>>;

pub async fn mark_voice_job_status(
    pool: &PgPool,
    job_id: i64,
    status: &str,
) -> anyhow::Result<()>;

pub async fn mark_voice_job_failed(
    pool: &PgPool,
    job_id: i64,
    error: &str,
) -> anyhow::Result<()>;

pub async fn save_asr_result(
    pool: &PgPool,
    job_id: i64,
    transcript: &AsrTranscript,
) -> anyhow::Result<()>;

pub async fn save_voice_result(
    pool: &PgPool,
    job_id: i64,
    result: &CleanTranscript,
    final_html: &str,
    full_text_file_id: Option<&str>,
) -> anyhow::Result<()>;
```

## Types

Минимальные типы:

```rust
pub enum VoiceMediaKind {
    Voice,
    Audio,
    VideoNote,
}

pub struct VoiceMedia {
    pub chat_id: i64,
    pub message_id: i32,
    pub user_id: Option<i64>,
    pub kind: VoiceMediaKind,
    pub file_id: String,
    pub file_unique_id: Option<String>,
    pub duration_sec: Option<u32>,
    pub file_size: Option<u64>,
    pub mime_type: Option<String>,
}

pub struct AsrSegment {
    pub start_sec: f32,
    pub end_sec: f32,
    pub text: String,
}

pub struct AsrTranscript {
    pub provider: String,
    pub model: String,
    pub request_id: Option<String>,
    pub text: String,
    pub segments: Vec<AsrSegment>,
    pub raw_json: serde_json::Value,
}

pub struct CleanTranscript {
    pub text: String,
    pub topics: Vec<String>,
    pub short_summary: Option<String>,
}
```

Не хранить Telegram HTML как единственный источник истины. `cleaned_text` должен быть plain text/Markdown-like, а HTML собирать отдельно.

## Download layer

`download.rs` должен:

1. выбрать `file_id` из `msg.voice()`, `msg.audio()`, позже `msg.video_note()`;
2. проверить duration/file_size из Telegram metadata до скачивания;
3. вызвать `bot.get_file(file_id.clone()).await?`;
4. скачать файл во временную директорию;
5. вернуть `DownloadedVoice { path, original_ext, mime_type, size }`;
6. удалить temp-файл после ASR.

Зависимости, которые могут понадобиться:

```toml
reqwest = { version = "0.12", features = ["json", "multipart"] }
tempfile = "3"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "fs", "io-util"] }
```

Если включишь ffmpeg preprocessing позже, добавить `tokio/process` и отдельный helper:

```rust
async fn normalize_audio_for_asr(input: &Path) -> anyhow::Result<PathBuf>;
```

Для MVP не надо ffmpeg для обычного Telegram voice: `ogg` поддерживается Groq.

## Groq ASR client

Не смешивать ASR с `llm/service.rs`: это другой тип API и другой контракт. Сделать отдельный `features/voice/asr.rs`.

Примерная функция:

```rust
pub async fn transcribe_with_groq(
    config: &Config,
    audio_path: &Path,
    prompt: Option<&str>,
) -> anyhow::Result<AsrTranscript>;
```

Запрос:

```rust
let form = reqwest::multipart::Form::new()
    .text("model", config.voice_asr_model.clone())
    .text("response_format", "verbose_json")
    .text("language", config.voice_language.clone())
    .text("temperature", config.voice_asr_temperature.to_string())
    .text("timestamp_granularities[]", "segment")
    .part("file", file_part);
```

Headers:

```text
Authorization: Bearer $GROQ_API_KEY
Content-Type: multipart/form-data
```

Парсить минимум:

```json
{
  "text": "...",
  "segments": [
    { "start": 0.0, "end": 12.4, "text": "..." }
  ],
  "x_groq": { "id": "..." }
}
```

`x_groq.id` сохранить в `asr_request_id`, если есть.

## Cleanup prompt

Создать `prompts/voice_cleanup.md`.

Задача cleanup LLM:

- исправить явные ошибки ASR;
- восстановить пунктуацию;
- убрать слова-паразиты и бессмысленные повторы;
- не выдумывать факты;
- сохранить смысл, стиль и важные формулировки;
- сохранить/поправить технические термины;
- заменить длинные числительные цифрами, где это улучшает читаемость;
- разбить на смысловые фрагменты;
- поставить timestamps в формате `0:30` в начале фрагментов;
- если кусок неразборчив, писать `[неразборчиво]`, а не фантазировать.

Пример prompt:

```text
Ты чистишь расшифровку голосового сообщения из Telegram-чата.

На входе ASR segments с таймкодами. Исправь ошибки распознавания, восстанови пунктуацию, убери слова-паразиты и бессмысленные повторы. Не добавляй новых фактов и не меняй позицию говорящего.

Формат ответа:
0:00 Первый смысловой фрагмент.
0:30 Второй смысловой фрагмент.
1:10 Третий смысловой фрагмент.

Правила:
- таймкод бери из начала соответствующего ASR segment;
- объединяй соседние segments, если это одна мысль;
- не делай фрагменты длиннее 3-5 предложений;
- технические термины сохраняй точно;
- если не уверен в слове, выбери наиболее вероятный вариант по контексту;
- если фрагмент реально неразборчив, напиши [неразборчиво];
- не используй Telegram HTML;
- верни только готовую расшифровку.
```

Контекст для модели лучше передавать как compact list segments:

```text
[0:00-0:12] ну короче амд опять что то с драйверами
[0:12-0:24] на линуксе вроде норм но в протоне...
```

## Rendering

`voice/render.rs` должен работать от plain cleaned text.

MVP-правило:

1. собрать HTML title + cleaned transcript;
2. если `chars <= SAFE_TEXT_LIMIT`, отправить одним reply;
3. если длиннее, отправить preview reply и полный `.md`/`.txt` через `send_document`;
4. если даже preview внезапно > 4096, обрезать preview через `truncate_text` до `SAFE_TEXT_LIMIT`.

Пример типа:

```rust
pub enum RenderedTranscript {
    Single { html: String },
    PreviewAndFile { html: String, filename: String, body: String },
}
```

Для single message можно подсветить timestamp:

```text
0:30 -> <code>0:30</code>
```

Но только если renderer сам экранирует весь остальной текст. Не отдавать LLM право писать Telegram HTML.

Для file body использовать plain Markdown-like text:

```text
# Расшифровка голосового

Чат: ...
Сообщение: ...

0:00 ...
0:30 ...
```

## Pipeline skeleton

```rust
pub async fn maybe_transcribe_voice(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
) -> anyhow::Result<bool> {
    if !state.config.voice_transcription_enabled {
        return Ok(false);
    }

    let Some(media) = VoiceMedia::from_message(msg) else {
        return Ok(false);
    };

    if !state.config.voice_auto_transcribe {
        return Ok(false);
    }

    if should_skip_voice(&media, &state.config) {
        return Ok(true);
    }

    let Some(job_id) = repo::create_voice_job(&state.pool, &media).await? else {
        return Ok(true);
    };

    let result = run_voice_job(bot, &state.config, &state.pool, job_id, media).await;
    if let Err(err) = result {
        repo::mark_voice_job_failed(&state.pool, job_id, &err.to_string()).await?;
        return Err(err);
    }

    Ok(true)
}
```

`run_voice_job`:

1. `mark status = downloading`;
2. download;
3. `mark status = transcribing`;
4. Groq ASR;
5. save raw ASR;
6. `mark status = cleaning`;
7. cleanup via LLM;
8. render;
9. send reply or preview + document;
10. save final result and `status = sent`.

## Sending document fallback

Для длинной расшифровки:

```rust
bot.send_document(chat_id, InputFile::memory(body.into_bytes()).file_name(filename))
    .caption("Полная расшифровка")
    .reply_parameters(ReplyParameters::new(original_message_id).allow_sending_without_reply())
    .await?;
```

Хранить `full_text_file_id`, если Telegram вернул document with file_id. Это позволит потом не генерировать файл повторно при retry/report.

## Error policy

Не спамить чат внутренними ошибками.

Пользовательские ответы:

- файл слишком большой;
- голосовое слишком длинное;
- Telegram не дал скачать файл;
- ASR временно недоступен;
- расшифровка получилась пустой.

Внутренние детали писать в `voice_transcription_jobs.error` и logs.

Для transient ошибок Groq/Telegram можно оставить job `failed` и позже добавить retry tool:

```bash
cargo run --release --bin retry_voice_transcriptions -- --limit 10
```

Но retry tool не тащить в первый commit, если MVP ещё не стабилен.

## Security and privacy

- Не логировать полный transcript на info level.
- Не логировать download URL: он содержит bot token.
- Temp files удалять после ASR даже при ошибке.
- Не сохранять audio bytes в Postgres.
- Сохранять только `file_id`, `file_unique_id`, ASR JSON и cleaned text.
- Для owner preview voice не нужен.

## Порядок коммитов

Оптимальный порядок:

1. `config: add voice transcription settings`.
2. `db: add voice transcription jobs table`.
3. `voice: add media detection and types`.
4. `voice: add telegram file download layer`.
5. `voice: add groq transcription client`.
6. `voice: add cleanup prompt and renderer`.
7. `voice: wire pipeline into message handler`.
8. `voice: add document fallback for long transcripts`.
9. `docs: document voice transcription`.

После каждого шага:

```bash
cargo fmt
cargo check
```

После pipeline wiring проверить в живом чате на коротком voice до 10 секунд.

## Tests

Минимальные unit tests:

```text
voice::types::VoiceMedia::from_message ignores non-voice
voice::render single short transcript
voice::render preview + file for long transcript
voice::render escapes raw HTML from cleanup model
voice::cleanup prompt contains segment timestamps
voice::asr parses verbose_json segments
config parses voice env defaults
```

Integration smoke без реального Groq:

- fake `AsrClient` возвращает segments;
- fake cleanup возвращает cleaned text;
- renderer отправляет one-message path;
- long text уходит в `PreviewAndFile`.

## Что не делать в первом voice PR

- Не делать VAD/chunking длинных аудио.
- Не делать diarization.
- Не делать speaker labels.
- Не делать embeddings по transcript.
- Не делать summary по всей истории голосовых.
- Не делать local Whisper/Ollama audio.
- Не переписывать весь Telegram renderer под entities.

Первый PR должен дать рабочий вертикальный срез: voice -> Groq ASR -> LLM cleanup -> Telegram reply/file -> DB audit.
