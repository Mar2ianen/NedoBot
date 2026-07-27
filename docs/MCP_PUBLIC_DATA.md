# Публичный data surface MCP

Этот документ — human-readable inventory текущего `mcp_public` read-model. Он сгенерирован по [`config/mcp_db_manifest.toml`](../config/mcp_db_manifest.toml) (manifest version 1, `generated_at = 2026-07-17T18:14:36.178064522+00:00`); manifest остаётся исчерпывающим машиночитаемым источником имён колонок, PostgreSQL-типов и nullable-статуса.

## Границы публикации

- Scope сообщений: `discussion_chat_id = -1001932061163`; одного совпадения `source_channel_id` недостаточно.
- MCP принимает только структурированные read-only tools и только view из manifest: произвольного SQL, write-операций и доступа к `public.*` нет.
- Новая колонка базовой таблицы не становится публичной автоматически: её нужно явно добавить в view, regenerated manifest и этот inventory отдельным review.
- Поля с текстом, заметками, ASR-расшифровками, результатами `/ask` и антиспам-анализом являются частью опубликованного contract. Потребитель обязан считать их данными публичного чата и не использовать для идентификации или профилирования вне назначения endpoint.
- Raw Telegram JSON не опубликован. Полный список фактически опубликованных идентификаторов и полей находится в manifest; он важнее обобщающих описаний ниже.

## Views

| View | Primary key | Колонок | Назначение |
|---|---|---:|---|
| `admin_events` | `id` | 6 | Аудит административных событий. |
| `ask_runs` | `id` | 15 | Запуски `/ask`: вопрос, ответ, status, модель и метрики. |
| `ask_tool_calls` | `id` | 10 | Аудит вызовов инструментов `/ask`. |
| `avatar_analysis_jobs` | `id` | 16 | Статус и метаданные задач анализа аватаров. |
| `avatar_image_analyses` | `profile_photo_file_unique_id`, `prompt_version` | 7 | Результаты анализа изображений аватаров. |
| `avatar_profile_assessments` | `telegram_user_id`, `profile_photo_file_unique_id`, `features_snapshot_hash`, `prompt_version` | 9 | Оценки профилей по снимку признаков. |
| `llm_generations` | `id` | 12 | Метаданные, prompt, response и HTML генераций комментариев. |
| `post_comment_jobs` | `id` | 11 | Очередь и результаты первых комментариев. |
| `post_history_entries` | `id` | 16 | RAG-карточки постов: summary, entities, angle и внешний факт. |
| `search_runs` | `id` | 8 | Запуски поиска для комментариев. |
| `telegram_chat_member_events` | `id` | 9 | События вступления, выхода и изменения статуса участника. |
| `telegram_chat_member_snapshots` | `chat_id`, `telegram_user_id` | 6 | Текущий снимок статуса участника. |
| `telegram_chat_notes` | `id` | 9 | Общие заметки публичного чата. |
| `telegram_chat_users` | `chat_id`, `telegram_user_id` | 31 | Участники, activity-метрики и spam-разметка. |
| `telegram_message_edits` | `id` | 8 | Наблюдаемые изменения сообщений. |
| `telegram_message_reaction_counts` | `chat_id`, `message_id` | 6 | Агрегированные реакции сообщений. |
| `telegram_message_reactions` | `id` | 9 | История изменений реакций. |
| `telegram_messages` | `chat_id`, `message_id` | 32 | Текст, автор, reply- и media-метаданные сообщений публичного чата. |
| `telegram_new_user_profile_audits` | `chat_id`, `telegram_user_id` | 91 | Признаки и результат антиспам-анализа нового участника. |
| `telegram_profile_identity_observations` | `telegram_user_id`, `snapshot_key` | 7 | Наблюдения нормализованных имени, username и аватара. |
| `telegram_user_notes` | `id` | 11 | Заметки об участнике, привязанные к публичному чату. |
| `telegram_user_profiles` | `telegram_user_id` | 28 | Безопасная typed-проекция профиля участника и metadata refresh. |
| `voice_transcription_jobs` | `id` | 23 | Расшифровки голосовых и метаданные их обработки. |

## Поля повышенного внимания

Следующие группы не скрыты от generic `db.*` tools, если они перечислены в manifest. Перед расширением потребителей или публикацией новой колонки требуется отдельная privacy/security-проверка:

- пользовательский текст: `telegram_messages.text`, old/new text edit-истории, заметки, ASR transcript и cleanup-результаты;
- результаты генерации: `llm_generations.prompt`, `response`, `final_html`, а также `/ask` question/answer;
- profile и anti-spam metadata, включая username/display name, bio, статусы участников, risk labels и причины;
- stable Telegram/file-derived identifiers, которые явно перечислены в manifest.

## Как обновлять

```bash
cargo run --release --bin generate_mcp_db_manifest -- config/mcp_db_manifest.toml
git diff -- config/mcp_db_manifest.toml
```

После review manifest обновить эту таблицу, README и раздел [«Публичный Read-only MCP»](TECHNICAL.md#публичный-read-only-mcp). Не публиковать новый view или колонку вместе с несвязанным рефакторингом transport-а.
