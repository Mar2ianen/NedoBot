# Voice transcription follow-up

Актуальное состояние после реализации voice pipeline. Старый план внедрения выполнен: вертикальный срез `voice/audio/video_note -> Groq ASR -> LLM cleanup -> Telegram reply/file -> DB audit` уже есть в коде.

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
- поддерживает `voice`, `audio` и `video_note`;
- кружок отправляется в Groq как исходный MP4 с MIME `video/mp4`, без `ffmpeg`.

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
9. Отправить кружок и проверить `media_kind=video_note`, `status=sent`.

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

### 1. Сохранять cleanup provider/model в БД

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

### 2. User-facing ошибки для ASR/cleanup/download

Сейчас validate errors отвечают пользователю, а ошибки download/ASR/cleanup в основном уходят в logs + `voice_transcription_jobs.error`.

Нужно решить policy:

- transient Groq/Telegram error: тихо логировать или отвечать `Не смог расшифровать, API отвалился`;
- empty ASR transcript: лучше коротко ответить пользователю;
- cleanup failed but ASR ok: можно отправлять raw ASR с пометкой не надо, если бот только для себя.

Практичный MVP: отвечать пользователю только на понятные recoverable ошибки, внутренние stack details не показывать.

### 3. Manual `/transcribe` reply command

Auto mode уже есть, но ручной режим полезен, если auto-transcribe начнёт шуметь.

Правило:

```text
/transcribe reply на voice/audio -> расшифровать reply message
```

Не нужно делать свободный аргумент с message id на первом проходе.

### 4. `video_note` без `ffmpeg` — выполнено

Groq ASR принимает MP4 напрямую, поэтому кружок скачивается во временный файл и отправляется в существующий multipart ASR request с MIME `video/mp4`. `TempPath` удаляется после ASR, исходник на сервере не хранится, а в `voice_transcription_jobs` сохраняется только audit-результат с `media_kind=video_note`.

### 5. Тесты, которых не хватает

Уже есть тесты для:

- HTML escaping;
- expandable blockquote escaping;
- short transcript render;
- cleanup fallback на отсутствие глав;
- normalize terms.

Добавить:

```text
voice::render chapter title has no timestamp
voice::render long chapters produce MessageAndFile when over SAFE_TEXT_LIMIT
voice::cleanup parses valid chapter JSON
voice::cleanup invalid JSON falls back to plain text
voice::types video_note has mp4 MIME type for ASR upload
voice::download accepts video_note as mp4 media
voice::download keeps duration and size limits for video_note
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

1. Cleanup provider/model persistence.
2. User-facing error policy для ASR/download failures.
3. Manual `/transcribe` reply command.
4. Smoke в живом чате на коротком и длинном voice, а также на кружке.
