# Voice transcription follow-up

Актуальное состояние после реализации voice pipeline. Старый план внедрения выполнен: вертикальный срез `voice -> Groq ASR -> LLM cleanup -> Telegram reply/file -> DB audit` уже есть в коде.

## Уже реализовано

Коммиты после README cleanup добавили рабочий контур:

- `src/features/voice/pipeline.rs` - оркестрация job: сохранить сообщение, создать `voice_transcription_jobs`, скачать файл, вызвать ASR, cleanup, render, отправить reply/file, сохранить результат.
- `src/features/voice/types.rs` - `VoiceMedia`, `AsrTranscript`, `AsrSegment`, `CleanTranscript`, `TranscriptChapter`, render mode.
- `src/features/voice/download.rs` - `getFile`, temp file, проверка duration/file size, user-facing skip для слишком длинных/больших файлов.
- `src/features/voice/asr.rs` - Groq OpenAI-compatible `/audio/transcriptions`, `verbose_json`, segment timestamps.
- `src/features/voice/cleanup.rs` - LLM cleanup через JSON, fallback chain, plain fallback при parse/provider failure, нормализация техтерминов.
- `src/features/voice/render.rs` - short text, chapters, expandable blockquotes, safe Telegram limit, preview + file fallback.
- `src/features/voice/repo.rs` - запись job/status/raw ASR/cleaned result/final HTML/file id.
- `prompts/voice_cleanup.md` - prompt под русскоязычный техчат.
- `src/telegram/html.rs` - общий safe HTML builder, `expandable_blockquote`, `SAFE_TEXT_LIMIT`, truncation.
- `main.rs` - `maybe_transcribe_voice` вызывается до first-comment pipeline.

## Текущая политика поведения

Voice pipeline включается только при двух флагах:

```env
VOICE_TRANSCRIPTION_ENABLED=true
VOICE_AUTO_TRANSCRIBE=true
```

Фильтры в `maybe_transcribe_voice`:

- работает в private chat или в `DISCUSSION_CHAT_ID`;
- игнорирует ботов;
- игнорирует команды;
- игнорирует automatic forward;
- поддерживает `voice` и `audio`;
- `video_note` определяется, но сейчас явно отклоняется как unsupported.

Короткие расшифровки:

- если итоговый clean text `<= VOICE_SHORT_TEXT_MAX_CHARS`, renderer отправляет только текст;
- без заголовка;
- без глав;
- без timestamp;
- без blockquote.

Длинные расшифровки:

- если cleanup вернул главы и текст длиннее short limit, renderer собирает `Расшифровка голосового` + главы;
- тело главы идёт в `<blockquote expandable>`, если `VOICE_RENDER_EXPANDABLE_CHAPTERS=true`;
- если HTML влезает в `SAFE_TEXT_LIMIT`, отправляется одним сообщением;
- если не влезает, отправляется preview и полный `voice-transcript.txt`, если `VOICE_SEND_FULL_FILE=true`.

Fallback:

- если `VOICE_CLEANUP_PROVIDER` задан и падает, код пробует обычный `LLM_PROVIDER`;
- если все cleanup providers падают, используется raw ASR text;
- если cleanup JSON не парсится, используется plain LLM text;
- если после normalize нет глав, режим принудительно становится `short`.

## Что проверить руками

Минимальный smoke в живом чате:

1. `VOICE_TRANSCRIPTION_ENABLED=true`, `VOICE_AUTO_TRANSCRIBE=true`.
2. `VOICE_ASR_PROVIDER=groq`, `VOICE_ASR_MODEL=whisper-large-v3-turbo`.
3. `GROQ_API_KEY` заполнен.
4. Отправить короткое voice до 10 секунд.
5. Проверить, что ответ plain text без заголовка и timestamp.
6. Отправить длинное voice с 2-3 явными темами.
7. Проверить, что есть главы и раскрываемые цитаты.
8. Проверить записи в `voice_transcription_jobs`.

SQL для проверки:

```sql
select
    id,
    chat_id,
    message_id,
    media_kind,
    duration_sec,
    file_size,
    status,
    asr_provider,
    asr_model,
    render_mode,
    left(coalesce(error, ''), 120) as error,
    created_at,
    updated_at
from voice_transcription_jobs
order by id desc
limit 20;
```

## Ближайшие фиксы

### 1. Вернуть timestamp в заголовки глав

Запрошенный UX был таким:

```text
Обсуждение AMD 0:00
[свернутый текст до следующей главы]
```

Текущий `src/features/voice/render.rs` выводит только `chapter.title`, без `chapter.start_sec`.

Нужно:

- добавить `format_timestamp(seconds: f32) -> String` в `voice/render.rs` или общий helper;
- в `render_chapters`, `render_one_chapter` и `render_file_body` писать `title + timestamp`;
- timestamp рендерить через `Html::code(...)` или plain text рядом с title;
- добавить тест, что chapter title содержит `0:00`.

Критерий готовности:

```text
<b>Обсуждение AMD</b> <code>0:00</code>
<blockquote expandable>...</blockquote>
```

### 2. Сохранять cleanup provider/model в БД

Миграция уже содержит поля:

```text
cleanup_provider
cleanup_model
```

Но `save_voice_result` сейчас их не пишет. Для отладки fallback chain надо знать, какая модель реально чистила текст.

Вариант:

```rust
pub struct CleanupResult {
    pub provider: String,
    pub model: String,
    pub transcript: CleanTranscript,
}
```

Или минимально расширить `CleanTranscript`, если не хочется отдельный тип.

### 3. User-facing ошибки для ASR/cleanup/download

Сейчас validate errors отвечают пользователю, а ошибки download/ASR/cleanup в основном уходят в logs + `voice_transcription_jobs.error`.

Нужно решить policy:

- transient Groq/Telegram error: тихо логировать или отвечать `Не смог расшифровать, API отвалился`;
- empty ASR transcript: лучше коротко ответить пользователю;
- cleanup failed but ASR ok: можно отправлять raw ASR с пометкой не надо, если бот только для себя.

Практичный MVP: отвечать пользователю только на понятные recoverable ошибки, внутренние stack details не показывать.

### 4. Manual `/transcribe` reply command

Auto mode уже есть, но ручной режим полезен, если auto-transcribe начнёт шуметь.

Правило:

```text
/transcribe reply на voice/audio -> расшифровать reply message
```

Не нужно делать свободный аргумент с message id на первом проходе.

### 5. `video_note` через ffmpeg

Сейчас `video_note` определяется, но `download.rs` возвращает `UnsupportedVideoNote`.

Чтобы включить кружки:

- скачать `.mp4`;
- вытащить audio stream через ffmpeg;
- отправить audio в ASR;
- сохранить исходный `media_kind=video_note`.

Не тащить это до стабилизации обычных voice/audio.

### 6. Тесты, которых не хватает

Уже есть тесты для:

- HTML escaping;
- expandable blockquote escaping;
- short transcript render;
- cleanup fallback на отсутствие глав;
- normalize terms.

Добавить:

```text
voice::render chapter title includes timestamp
voice::render long chapters produce MessageAndFile when over SAFE_TEXT_LIMIT
voice::cleanup parses valid chapter JSON
voice::cleanup invalid JSON falls back to plain text
voice::download rejects video_note
voice::download rejects too long voice
voice::download rejects too large voice
voice::asr parses Groq verbose_json with segments
```

## Остаточные риски

- `VOICE_ASR_PROVIDER` сейчас фактически поддерживает только `groq`; unknown provider падает ошибкой.
- `VOICE_CLEANUP_PROVIDER` использует общий LLM provider router; если ошибиться в имени, будет `unknown LLM_PROVIDER`, хотя речь про cleanup provider.
- Для `audio` Telegram metadata обычно есть, но если duration/file_size внезапно отсутствуют, файл может дойти до API и упасть там.
- `render_mode=file` парсится как enum value, но renderer не имеет отдельной ветки для file-only режима; сейчас это не проблема, потому что prompt просит только `short | chapters`.
- В `render_preview` считается длина по уже HTML-escaped строкам плюс chunk; это достаточно для MVP, но не полноценный entity-aware splitter.
- CI/status checks на GitHub не настроены, поэтому сборку надо подтверждать локально через `cargo check`.

## Не делать сейчас

- diarization/speaker labels;
- VAD/chunking длинных аудио;
- local Whisper;
- embeddings по voice transcripts;
- summary всей истории голосовых;
- полноценный Telegram entities renderer;
- local Bot API server ради файлов больше 20 MB.

## Следующий порядок работы

1. Timestamp в chapter headings + тест.
2. Cleanup provider/model persistence.
3. User-facing error policy для ASR/download failures.
4. Manual `/transcribe` reply command.
5. Smoke в живом чате на коротком и длинном voice.
6. Только потом думать про `video_note`/ffmpeg.
