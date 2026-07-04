# Баги проекта tg-ai-bot-teloxide

## ✅ Исправлено 2026-07-03

- Critical 1–2: импорт Telegram Export теперь различает forwarded channel и automatic channel posts: `sender_chat_id` берётся из `from_id/actor_id`, `is_automatic_forward` не включается для обычных forwarded-сообщений.
- High 3: `refresh_known_member_snapshots` больше не кастует отрицательный `user_id` через `as u64`.
- High 4–5: статусы участников нормализованы явным `match`, banned везде сохраняется как `"banned"`.
- Medium 6: `save_message_reaction` получил `ON CONFLICT ... DO NOTHING` для повторных апдейтов.
- Medium 11: `telegram_message_reaction_counts.total_count` мигрирован на `bigint`, код сохраняет `i64` без `as i32`.
- Voice cleanup: prompt жёстко запрещает менять числа/версии/названия моделей и явно фиксирует контекст `НедоNews` + `Gemma 4 31B / gemma4:31b`.

## ✅ Исправлено 2026-07-04

- Gemini request parts serialization: `text` и `inlineData` теперь сериализуются в формате Gemini API; добавлен unit-test `request_parts_match_gemini_api_shape`.
- M1: для `groq`/`cerebras`/`openrouter` больше нет fallback на `VISION_MODEL`. Требуется `LLM_MODEL` или provider-specific `GROQ_MODEL`/`CEREBRAS_MODEL`/`OPENROUTER_MODEL`, иначе startup fail-fast.
- M2: `reqwest::Client` больше не создаётся заново на каждый LLM/ASR/profile request; добавлен общий кэш HTTP-клиентов с учётом timeout/proxy.
- M3: удалён unreachable `lower.contains("amd")` из вычисления `is_tech`.
- M4: voice cleanup больше не делает второй LLM-запрос с тем же prompt при падении кастомного cleanup provider; используется raw ASR fallback.
- L1: `first_text_chars` добавляет `…` при обрезке.
- L2: `strip_links` удаляет ссылки, обёрнутые пунктуацией/кавычками, например `(https://example.com)`.
- L5: удалён неиспользуемый параметр `_short_limit` из `plain_cleanup`.
- AP1: `normalize_provider` больше не дублируется; LLM router использует общий `normalize_llm_provider` из config.

---

## 🟡 Medium (отозвано / не подтвердилось)

### ~~H1. `user_profiles/service.rs:254` — getUserPersonalChatMessages не существует в Bot API~~
~~**Суть:** `fetch_personal_channel_messages` вызывает `getUserPersonalChatMessages` — это **не официальный метод Telegram Bot API**.~~
~~**Статус:** Отозван. Метод `getUserPersonalChatMessages` добавлен в **Bot API 10.1** (2026-06-11) и есть в официальной документации.~~

### ~~M5. `main.rs:112` — is_automatic_forward блокирует профиль для всех форвардов~~
~~**Суть:** `spawn_message_author_profile_refresh` проверяет `msg.is_automatic_forward()` и выходит.~~
~~**Статус:** Не подтвердилось. В `teloxide`/Bot API `is_automatic_forward` означает именно automatic channel post в connected discussion group, а не ручной user forward.~~

---

## 🟢 Low

### L3. `.env.example:45` — VOICE_ASR_TEMPERATURE=0 без дробной части
**Суть:** Везде `0.35`, `0.2`, а тут `0` без точки. Не баг (парсится ок), но неконсистентно.

### L4. `config.rs:80` — OLLAMA_BASE_URL=https://ollama.com (по умолчанию)
**Суть:** `https://ollama.com` — рабочий эндпоинт Ollama Cloud (см. docs.ollama.com/api/introduction). Не баг, но локальным пользователям нужно переопределять на `http://localhost:11434`. В `.env.example` тоже указан `https://ollama.com`.

### ~~L6. `first_comment/render.rs:77-95` — непарный `{CHAT_LINK` обрывает HTML~~
~~**Статус:** Уже исправлено ранее. При отсутствии `}` код добавляет весь остаток `after_start` как обычный текст и не теряет содержимое.~~

---

## 📋 Антипаттерны (code quality)

### AP2. Все тестовые fixtures дублируются
Каждый тестовый модуль определяет свою `fn config() -> Config` с 40+ полями. Добавление поля в `Config` требует правки в 10+ тестах. Нужен общий test-helper.

### AP3. Ошибки логируются, но Telegram не получает сигнал
`main.rs:94-102` — `maybe_transcribe_voice` / `maybe_comment_post` при ошибке возвращают `Ok(())`. Telegram не перешлёт update повторно при временных сбоях.

### AP4. Нет retry/backoff для внешних API
Нигде нет повторных попыток при временных ошибках Telegram Bot API или LLM провайдеров.

### AP5. `#[allow(dead_code)]` на неиспользуемых структурах
`Config`, `UserProfileDetails` (частично), `RefreshUserProfilesQuery`, `ProfileRefreshStats`, voice repo/types и другие заделы помечены `#[allow(dead_code)]`.

### AP6. Динамические имена колонок в ON CONFLICT UPDATE
`new_user_analysis.rs:1159-1168` — итерация по строкам с именами колонок. Хотя имена хардкожены в `audit_insert_columns()`, это хрупко.

---

## Статистика

| Уровень       | Было | Исправлено | Отозвано | Осталось |
|---------------|------|------------|----------|----------|
| 🔴 Critical   | 2    | 2          | 0        | 0        |
| 🔴 High       | 3    | 3          | 0        | 0        |
| 🟡 Medium     | 10   | 8          | 2        | 0        |
| 🟢 Low        | 6    | 3          | 1        | 2        |
| AP            | 6    | 1          | 0        | 5        |

(составлено 2026-07-03, обновлено 2026-07-04)
