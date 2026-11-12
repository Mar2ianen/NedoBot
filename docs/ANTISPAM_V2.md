# Антиспам v2: локальные признаки и Gemini Decision shadow

`gemini-decision-1` — закрытый классификатор Decisions API, а не генеративная
Gemma. Native transport отправляет `POST /v1beta/decisions` с
`x-goog-api-key`. `/ask`, генерация и ASR продолжают пользоваться своими routes.
Спецификация протокола: https://aistudio.google.com/docs/decisions-api.

## Границы rollout

Decision работает только в shadow. Его очередь и `DecisionAssessment` отделены
от `new_user_audit_jobs`; результаты не конвертируются в legacy assessment,
не создают `auto_ban_evidence`, review, captcha, spam labels или Telegram actions.
Старый аудит и его generation/materialization fences сохраняются.
Новый `fresh_work_dm_funnel` — только review, включая разреженные/обфусцированные
предложения работы без суммы. Глобальный `min_text_chars=32` не снижен.
Для обфусцированного work/contact сочетания есть узкое исключение из shortcut.
Новое слабое правило и это исключение сохраняются как review observations,
не short-circuit старый генеративный аудит и не выключают его captcha/safety
policy. Старые deterministic fast paths остаются прежними.

Новый lexical view не меняет `unicode-spam-v2 model_text` обученного TF-IDF
и не расширяет старые правила дополнительной нормализацией. Структурные признаки
`spam-structure-v1` ограничены 4096 символами и сами не являются spam verdict.
Исключения для цитат, поиска работы и технической помощи задаются в versioned
`spam_features.toml`, а не новой коллекцией фраз в dispatcher.

## Покрытие и бюджет

После ingest выполняется SQL/local наблюдение без Telegram profile refresh,
embedding и внешних запросов. Только настроенные `ingest && moderation` чаты;
не боты/администраторы, последние ручные `not_spam` veto, до 10 сообщений и
возраст в чате не более 72 часов. Установившиеся участники не создают поток
shadow jobs. Первый текст новичка даёт контрольный пример; последующие сообщения
ставятся в очередь только при lexical/obfuscation сигнале.

Каждый instance имеет собственный PostgreSQL backlog (по умолчанию 100) и
атомарный hourly budget (30 попыток, ошибки и retries тоже расходуют бюджет).
Ни enqueue, ни worker не работают при disabled. Новые версии конфигурации,
вопросов, модели и нормализации не используют старые pending jobs.
Лишние кандидаты при полном backlog пропускаются явно, без неограниченного накопления.

Вход содержит только собственный bounded/redacted текст автора, явную policy
чата и структурные признаки. Reply-parent и чужие цитируемые сообщения отдельно
не подмешиваются. URL редактируются существующим redactor; полные Telegram JSON,
имена, IDs и биография не передаются. Аватар — отдельный opt-in, bounded cache
существующего Telegram profile photo; фотографии/голос/видео из сообщений не
отправляются. Признак наличия аватара и его версия входят в snapshot.

## Версии, состояния и отказ

Вопросы независимы: `unsolicited_promotion`, `work_solicitation`,
`private_contact_funnel`, `contextual_relevance`, `spam_campaign_type`,
`quoted_or_discussed_spam`. `noul` — P(yes), confidence не заменяет её.
Композиция в Rust не перемножает marginal probabilities. Вероятности, confidence,
usage, model, hash вопросов, normalization/policy versions и входа сохраняются.
Отсутствующий ответ, неизвестные категории, неверные distributions/types/score
закрывают запрос с `invalid_response`. Quote/неоднозначность даёт abstain;
`no_signal` не является ручной меткой ham.

Claim: `FOR UPDATE SKIP LOCKED`, общий external lease, монотонный fence.
При claim и завершении проверяется исходный текст, edit timestamp, последняя
ручная метка и версия аватара; устаревший ответ сохраняется как `stale`, не как
действующее решение. Потерянный lease не позволяет старому worker завершить
новый claim. Retry использует общий `ANALYSIS_RETRY`, HTTP400/401/403 и schema
errors terminal, 429/5xx/timeout retryable, максимум шесть claims. Source older
24h не отправляется обычным live worker. Ошибки не содержат provider body/ключ.
Внешняя ошибка не останавливает ingest и локальную классификацию.

Таблицы `moderation_text_observations`, `moderation_decision_jobs`,
`moderation_decision_budget` приватные. MCP views/manifest/grants не расширены.
Нельзя перепубликовывать эти строки через публичный SELECT.

## Конфигурация и эксплуатация

Секция `[moderation.decision]` описана в tracked profile example. Для instance:

1. Сохранить backup его profile, env, базы и бинарников; проверить обе community.
2. Подготовить migration-aware rollback binary со старым runtime и additive
   migration `20261112100000`. Старый binary без этой миграции запускать после
   применения нельзя: SQLx справедливо откажет из-за неизвестной applied migration.
3. Собрать один default-feature Ubuntu24.04 release и проверить SHA/checksums.
4. В private runtime profile добавить `enabled=true`,
   `external_processing_allowed=true`, `avatar_enabled=true` при разрешённой
   внешней обработке, `api_key_env="GEMINI_API_KEY"`, конкретную policy чата.
   Значение существующего ключа остаётся только в process environment.
5. Применить additive migration к обеим БД, установить один binary на оба
   instance и migration-aware MCP binaries, проверить активность/хеши/журнал.
6. Убедиться, что первые реальные jobs `succeeded`/объяснимый `stale`, budget
   соблюдён, provider failures не породили actions. Сравнивать queue и API latency
   отдельно, не выдавать время offline replay за live latency.

Для rollback достаточно `enabled=false` и restart соответствующего instance;
исторические assessments остаются. Для полного binary rollback использовать
только подготовленный migration-aware compatibility artifact. Ключи/биллинг
этим изменением не меняются. Оба чата подтверждены владельцем как публичные.

## Диагностика и воспроизводимость

`spam_decision trace OUTPUT CHAT MESSAGE` читает только настроенный scope в
read-only transaction, пишет private файл mode0600 (create-new). В нём исходное
Unicode/model представление, признаки, существующий job/assessment, policy,
captcha, удаления и Decision history. Неингестированное сообщение —
`not_ingested`, а не ошибка classifier. Текущее состояние не доказывает
исторический policy gate: для старых событий сверять журнал по времени/job.

`spam_decision probe --synthetic OUTPUT` делает максимум 12 запросов на
tracked regression fixtures, не читает чаты и не создаёт действий.
`spam_decision replay INPUT_JSONL OUTPUT` без сети пересчитывает lexical A/B
на замороженных текстах. Каждый row имеет hash; variants одного исходного
примера не являются независимыми наблюдениями. Replay трактует неизвестный
`label_level` как `author_weak`, а не message gold. Полный baseline включает
старый audit/policy, lexical A/B не выдаётся за его end-to-end recall.

Старые `manual_owner_confirmation` stamps **не message-level gold**: `record_spam`
штампует все сообщения автора. Для FPR/recall необходимы отдельные актуальные
ручные message labels, разбиение по автору/кампании/времени и отдельные instance
отчёты. Маленькие наборы normal (18/2 автора в предыдущем replay) не позволяют
включать новые санкции; при 0 FP верхняя 95% граница примерно `3/n`.
`spam_decision compare FROZEN_JSONL OUTPUT` сравнивает A/B/C/D на одинаковых
снимках: baseline берётся из замороженного решения, остальные варианты добавляют
local/Decision candidates; manual veto действует для всех. Проверяются instance,
source/question/model/policy/normalization versions, утечка групп между
partitions и дублирование источников. Выводятся recall/FPR/precision, FN по
campaign group, recall при review budget 5/10/25/50 в порядке поступления,
покрытие/abstentions, token usage, отдельные API/queue/action p50/p95/p99.
ECE/Brier считаются только для отдельно размеченного `noul` вопроса, а не из
confidence или author label. Стоимость без подтверждённого тарифа — unknown.
API error даёт abstain с fallback на baseline, не метку ham.
`enqueue-recent --limit N` (1..20) позволяет ограниченно проверить свежие
реальные сообщения своего instance, создавая только shadow jobs с теми же gates
и budget. Это не backfill старых санкций.

## Трасса реального пропуска 12 ноября 2026

В ПВО найдены три ingested сообщения с разреженным предложением подработки
(39 символов; обычные U+0020, без необычных пробелов). Возраст автора около
6–7 часов, итоговый count4. Генеративный аудит распознал work + DM; локальный
каскад дал `short_text`, старый lexical campaign отсутствовал.

Журнал UTC: 06:47:22 — score70, удаление подавлено `review_delivery_disabled`;
06:57:29 — score100, капча выдана, два сообщения удалены; 07:11:18 — score90,
`ineligible Telegram membership`. Третье сообщение не имеет bot-delete stamp.
Это подтверждённый локальный detection gap **и отдельные policy gates**,
а не три нераспознанных моделью сообщения. Новое правило не используется для
обхода этих защит или автоматического бана. Исторические данные не переразмечались.
