# Баги проекта tg-ai-bot-teloxide

## ✅ Исправлено 2026-07-03

- Critical 1–2: импорт Telegram Export теперь различает forwarded channel и automatic channel posts: `sender_chat_id` берётся из `from_id/actor_id`, `is_automatic_forward` не включается для обычных forwarded-сообщений.
- High 3: `refresh_known_member_snapshots` больше не кастует отрицательный `user_id` через `as u64`.
- High 4–5: статусы участников нормализованы явным `match`, banned везде сохраняется как `"banned"`.
- Medium 6: `save_message_reaction` получил `ON CONFLICT ... DO NOTHING` для повторных апдейтов.
- Medium 11: `telegram_message_reaction_counts.total_count` мигрирован на `bigint`, код сохраняет `i64` без `as i32`.
- Voice cleanup: prompt жёстко запрещает менять числа/версии/названия моделей и явно фиксирует контекст `НедоNews` + `Gemma 4 31B / gemma4:31b`.

## 🔴 Critical

### 1. `import_telegram_export.rs:389` — sender_chat_id = source_channel_id (неверно для forwarded)
**Суть:** В SQL `$10` — `sender_chat_id`, но на этой позиции биндится `source_channel_id`. Когда юзер пересылает сообщение из канала: `source_channel_id = Some(channel_id)`, а `sender_chat_id` должен быть `None`.
**Фикс:** Парсить `from_id` отдельно для `sender_chat_id`, а не использовать `source_channel_id`.

### 2. `import_telegram_export.rs:384` — is_automatic_forward = source_channel_id.is_some() (неверно для forwarded)
**Суть:** `$5` — `is_automatic_forward`, биндится `source_channel_id.is_some()`. Если юзер переслал из канала (`forwarded_from_id`), `is_automatic_forward` должно быть `false`, но будет `true`.
**Фикс:** Учитывать, что сообщение forwarded, а не auto-forwarded.

---

## 🔴 High

### 3. `db/telegram.rs:735` — `user_id as u64` без проверки отрицательного
**Суть:** Если в БД `user_id` (i64) отрицательный, `as u64` даст огромное число → битый API-запрос к Telegram.
**Фикс:** `UserId(u64::try_from(user_id)?)`.

### 4. `refresh_chat_members.rs:235` — статус `"kicked"` вместо единого `"banned"`
**Суть:** `ChatMemberKind::Banned(_)` возвращает `"kicked"`, а `chat_member_status` в `db/telegram.rs:903-904` через Debug-формат даёт `"banned"`. В БД два разных значения для одного статуса — ломается фильтрация и отчёты.
**Фикс:** `("banned", false, false)`.

### 5. `db/telegram.rs:903-904` — chat_member_status зависит от Debug-формата
**Суть:** `format!("{:?}", kind.status()).to_lowercase()` — если teloxide изменит Debug-формат, все статусы в БД перестанут совпадать.
**Фикс:** Явный match, как в `refresh_chat_members.rs`.

---

## 🟡 Medium

### 6. `db/telegram.rs:581-597` — save_message_reaction без ON CONFLICT
**Суть:** INSERT в `telegram_message_reactions` без `on conflict`. При повторной доставке апдейта (реконнект) — `unique violation`. Рядом `save_message_reaction_count` (строка 619) имеет `on conflict`, значит просто забыли.
**Фикс:** `on conflict (chat_id, message_id, user_id, event_at) do nothing`.

### 7. `llm/service.rs:96-97` — fallback на vision_model для groq/cerebras/openrouter
**Суть:** Если `LLM_MODEL` не указан, для groq/cerebras/openrouter используется `config.vision_model` (напр. `gemma4:31b`). 31B модель может быть недоступна или неоптимальна для этих провайдеров.
**Фикс:** Хранить дефолтную модель для каждого провайдера отдельно.

### 8. 4× `reqwest::Client` создаётся на каждый запрос
**Файлы:** `llm/gemini.rs:41`, `llm/openai_compat.rs:44`, `llm/ollama.rs:36`, `features/voice/asr.rs:40`
**Суть:** На каждый вызов LLM/ASR новый HTTP-клиент → TLS handshake оверхед, нет keep-alive.
**Фикс:** Вынести `Client` в поле структуры или в `AppState`.

### 9. `render.rs:55` — мёртвый код (unreachable `lower.contains("amd")`)
**Суть:** Строка 48-53: `if lower.contains("amd") { return ... }`. Строка 55: `lower.contains("amd")` в вычислении `is_tech` — никогда не выполнится.
**Фикс:** Убрать дублирующую проверку.

### 10. `cleanup.rs:43-60` — двойная генерация при падении cleanup-провайдера
**Суть:** При ошибке кастомного провайдера и `should_try_default_cleanup_provider = true` делается второй запрос к LLM с тем же промптом. Удвоение времени и токенов.
**Фикс:** Логировать первую ошибку, не пересоздавая запрос.

### 11. `db/telegram.rs:630` — total_count as i32 может переполниться
**Суть:** Сумма реакций считается как `i64` (строка 611-612), но кастуется в `i32`. При >2³¹ реакций — переполнение.
**Фикс:** Убрать `as i32`, оставить `i64`.

---

## 🟢 Low

### 12. `text.rs:1-8` — first_text_chars не добавляет многоточие при обрезке
**Суть:** Текст просто обрезается без `…`.
**Фикс:** Добавить `…` при обрезке.

### 13. `text.rs:19-24` — strip_links не ловит ссылки с пунктуацией
**Суть:** `split_whitespace()` + `starts_with("http://"|"https://")` — ссылка `https://example.com.` (с точкой) не удалится.
**Фикс:** Трим пунктуации перед проверкой.

### 14. `.env.example:45` — VOICE_ASR_TEMPERATURE=0 без дробной части
**Суть:** Везде `0.35`, `0.2`, а тут `0` без точки. Не баг (парсится ок), но неконсистентно.

### 15. `config.rs:78` — OLLAMA_BASE_URL=https://ollama.com (по умолчанию)
**Суть:** `https://ollama.com` — рабочий эндпоинт Ollama Cloud (см. docs.ollama.com/api/introduction). Не баг, но локальным пользователям нужно переопределять на `http://localhost:11434`. В `.env.example` тоже указан `https://ollama.com`.

---

## Статистика

| Уровень    | Было | Исправлено | Осталось |
|------------|------|------------|----------|
| 🔴 Critical | 2    | 2          | 0        |
| 🔴 High     | 3    | 3          | 0        |
| 🟡 Medium   | 6    | 2          | 4        |
| 🟢 Low      | 4    | 0          | 4        |
| **Итого**   | **15** | **7**    | **8**    |

(составлено 2026-07-03, обновлено 2026-07-03)
