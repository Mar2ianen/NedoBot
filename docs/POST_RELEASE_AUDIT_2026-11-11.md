# Проверка после Cloudflare release, 11 ноября 2026

Основной срез: 11 ноября 08:02 UTC / 11:02 МСК. С audit rollout 10 ноября
15:56:43 UTC прошло около 16 часов, а не полные сутки. Оба бота и public MCP
active/running, NRestarts=0; executable SHA-256 совпали с release 4afef1a.
Embedding service также active. После correction release начинается новое
окно наблюдения; оно не заменяет этот исторический срез.

| Наблюдение с audit rollout | NedoNews | PVO |
|---|---:|---:|
| Новые успешные Cloudflare audits с применённым результатом | 19 | 3 |
| Новые успешные deterministic audits | 0 | 2 |
| Failed audits в этой новой когорте | 0 | 0 |
| Активный generation/materialization backlog | 0 | 0 |
| Новые сообщения в БД | 281 | 5 |
| Автоматические баны по локальному подтверждённому evidence | 0 | 2 |

Это счётчики jobs, не число HTTP-попыток и не измеренная точность/полнота
детектора. В этом окне нет новых ручных банов для независимой сверки recall.
Два PVO бана подтверждены member events, system labels и очисткой сообщений.
Автосанкции NedoNews выключены, как и прежде; review delivery выключена в
обоих инстансах. Эти флаги проверка не включает.

## Найденные ошибки и восстановление

- Два старых post-history HTTP 402 в 16:01 и 16:02 UTC 10 ноября относятся
  к Ollama memory до config-only follow-up в 16:27 UTC. Обе задачи уже
  восстановлены. На текущем срезе изменившиеся post-history rows: 14 ready и
  одна ignored от Cloudflare, без оставшегося error_kind; это включает recovery.
- Три NedoNews и два PVO update-listener ERROR — connection reset/timeout
  Telegram polling. Обработка сообщений продолжилась, рестартов не было.
  Это реальные ошибки периода, поэтому весь журнал не объявляется чистым.
- Девять search-extraction warnings HTTP 402 вызваны оставшимся Ollama route.
  В correction rollout search_extract NedoNews переключён на отдельный
  cloudflare_gemma_search, JSON Object, thinking=false, прежний бюджет 900.
  Два synthetic HTTP probe дали валидный research-plan contract и stop за
  224/255 output tokens. Это не live-проверка качества поиска по реальному посту.
  PVO profile побайтно сохранён; прочие NedoNews profile fields тоже сохранены.

Отдельно проверены старые terminal failures до Cloudflare rollout. Из 13
последних failed jobs пяти авторов разрешён однократный recovery: собственный
instance scope, совпадающие canonical snapshot/generation, нет ручной метки,
нет нового job и нет известного spam status. Остальные исключены как
устаревшие и/или вручную размеченные; старые исторические failures не удалены.

В 08:35:42 UTC 11 ноября эти пять jobs поставлены в retry без сброса attempts;
generation/materialization fences увеличены. К 08:36:25 UTC все пять получили
Cloudflare assessment и succeeded materialization, attempts=2, fences=3/2.
Три содержали аватар. Во время recovery enforcement, captcha и review delivery
NedoNews оставались выключены. Новых ручных spam labels не создавалось.

## Изменение антиспама

[PR #110](https://github.com/Mar2ianen/NedoBot/pull/110) исправляет доставку
капчи: только ephemeral reply на собственный комментарий, принятие и запись
карточки до удаления anchor; cleanup повторяется фоновым worker без второй
карточки или продления TTL. Non-member не исключается автоматически при наличии
свежего собственного комментария; отказ Telegram не запускает таймер и не
создаёт публичного fallback. Модель подозрения и пороги не изменены.

Live-доставка/уведомление на non-member клиенте не доказаны: Telegram не даёт
гарантии ephemeral delivery или read receipt. Контракт и ограничения описаны
в [runbook](EPHEMERAL_CAPTCHA.md). В Telegram не отправлялись тестовые сообщения.

Пороги локального корпуса не ослаблялись. Повторяющиеся короткие сообщения
размеченных spam-авторов включают обычные благодарности: авторская spam-метка
сама по себе не делает каждую его реплику спамом. На таком корпусе нельзя
обосновать новый short-text shortcut или автоматическое продвижение модели.

## Проверяемость

Дневные aggregate reports, recovery snapshot/action/evidence, profiles,
synthetic probes и checksums сохранены root-only в
 /var/backups/nedobot/2026-11-11-ephemeral-comment-captcha.
Сырые сообщения, аватары, DSN и credentials в репозиторий не добавлены.
Stdio MCP own-scope/foreign-scope/forged scope/private-memory probes и PostgreSQL
ACL smoke повторены для обеих БД до и после correction restart. Public HTTP
connector также прочитал только configured chat. Migration state не менялся:
latest=20261108130000, failed=0 в обеих БД.

Сведения о correction artifact, запуске, health window и rollback:
[DEPLOYMENT.md](DEPLOYMENT.md).
