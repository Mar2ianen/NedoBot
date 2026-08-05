# Следующая инженерная цель: post-migration verification

Unified audit, voice follow-up, миграция на fork `teloxide 0.18` и текущий model-driven `/ask` release закрыты и развёрнуты на `vps-153`. Самописные raw Bot API paths заменены typed methods fork-а; production-like Telegram smoke и release CI прошли. Deployment boundary зафиксирован тегом `deploy-2026-08-05-ask-author-evidence`.

## Закрытый voice follow-up

Старый план внедрения выполнен: вертикальный срез `voice/audio/video_note -> Groq ASR -> LLM cleanup -> Telegram reply/file -> DB audit` уже есть в коде.

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

```toml
[runtime]
voice_transcription_enabled = true
voice_auto_transcribe = true
```

Фильтры в `maybe_transcribe_voice`:

- работает в private chat или в `DISCUSSION_CHAT_ID`;
- игнорирует ботов;
- игнорирует команды;
- игнорирует automatic forward;
- поддерживает `voice`, `audio` и `video_note`;
- кружок отправляется в Groq как исходный MP4 с MIME `video/mp4`, без `ffmpeg`.

Короткие расшифровки:

- если итоговый clean text `<= runtime.voice_short_text_max_chars`, renderer отправляет только текст;
- без заголовка;
- без глав;
- без timestamp;
- без blockquote.

Длинные расшифровки:

- если cleanup вернул главы и текст длиннее short limit, renderer собирает `Расшифровка голосового` + главы;
- тело главы идёт в `<blockquote expandable>`, если `runtime.voice_render_expandable_chapters=true`;
- если HTML влезает в `SAFE_TEXT_LIMIT`, отправляется одним сообщением;
- если не влезает, отправляется preview и полный `voice-transcript.txt`, если `runtime.voice_send_full_file=true`.

Fallback:

- cleanup использует явный profile route `voice_cleanup` и его fallback chain;
- если все cleanup providers падают, используется raw ASR text;
- если cleanup JSON не парсится, используется plain LLM text;
- если после normalize нет глав, режим принудительно становится `short`.

## Что проверить руками

Минимальный smoke в живом чате:

1. `runtime.voice_transcription_enabled=true`, `runtime.voice_auto_transcribe=true`.
2. `runtime.voice_asr_provider=groq`, `runtime.voice_asr_model=whisper-large-v3-turbo`.
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

### Закрыто: provider/model, recoverable ошибки и ручной режим

`CleanupResult` сохраняет фактически использованные `cleanup_provider` и `cleanup_model` в `voice_transcription_jobs`, включая `raw_asr_fallback`.

Для recoverable download/ASR/cleanup failures job получает безопасный error kind (`download_failed`, `asr_failed`, `cleanup_failed`), а пользователь — короткий ответ без деталей Telegram, Groq, response body или внутреннего stack trace. Пустой ASR transcript получает отдельный понятный ответ.

`/transcribe` работает reply-командой для `voice`, `audio` и `video_note`. Она доступна при `runtime.voice_transcription_enabled=true`, даже когда `runtime.voice_auto_transcribe=false`; свободный аргумент с ID сообщения не поддерживается. Повторный вызов не создаёт второй job благодаря существующему dedup по `(chat_id, message_id)`.

### 4. `video_note` без `ffmpeg` — выполнено

Groq ASR принимает MP4 напрямую, поэтому кружок скачивается во временный файл и отправляется в существующий multipart ASR request с MIME `video/mp4`. `TempPath` удаляется после ASR, исходник на сервере не хранится, а в `voice_transcription_jobs` сохраняется только audit-результат с `media_kind=video_note`.

### Закрыто: ключевые unit-тесты voice

Покрыты HTML/expandable escaping, short transcript, fallback без глав и нормализация терминов, а также:

- JSON cleanup с главами и fallback при невалидном JSON;
- отсутствие timestamp в заголовках глав;
- длинные главы с `MessageAndFile` fallback;
- `video_note` с MIME `video/mp4` и лимитами duration/file size;
- Groq `verbose_json` и multipart request с segments.

## Остаточные риски

- `runtime.voice_asr_provider` сейчас фактически поддерживает только `groq`; unknown provider падает ошибкой.
- provider/model voice cleanup задаются в `voice_cleanup` profile route; отдельного env-router больше нет.
- Для `audio` Telegram metadata обычно есть, но если duration/file_size внезапно отсутствуют, файл может дойти до API и упасть там.
- `render_mode=file` парсится как enum value, но renderer не имеет отдельной ветки для file-only режима; сейчас это не проблема, потому что prompt просит только `short | chapters`.
- В `render_preview` считается длина по уже HTML-escaped строкам плюс chunk; это достаточно для MVP, но не полноценный entity-aware splitter.
- CI запускает форматирование, unit/integration tests и Clippy; живой Telegram/Groq smoke по-прежнему нужно подтверждать вручную.

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
