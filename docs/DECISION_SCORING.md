# Decision в общей оценке новичка

`moderation.decision.mode = "shadow"` сохраняет прежнее наблюдение без санкций.
`score_captcha` разрешает durable handoff в существующую ephemeral captcha;
зависит от включённых moderation, Decision, external processing и captcha.
Идентификаторы чата берутся только из конфигурации community.

## Метрика

```
score = min(100, baseline + max(legacy_content, decision_content)
                + avatar + personal_channel + external_reputation + contact)
decision_content = round(score_weight * max(promo, min(work, private_funnel)))
```

Вероятности независимых вопросов не перемножаются. Для content-вклада нужны
валидные ответы, вероятность цитаты/обсуждения <0.2, relevance <=0.8,
суммарная вероятность рекламных категорий >=0.8 и promo >=0.8 либо
work/private_funnel >=0.8. Неопределённость между видами рекламы не исключает
рекламность. Shadow verdict сохраняет прежнюю более строгую классификацию.

Контактный supporting-вклад (по умолчанию 8, максимум 15) идёт отдельно.
Разбираются Telegram mention/text_mention entities с UTF-16 offsets;
code/pre/blockquote исключены. Собственные контакты и известные боты исключены.
Используются только история/снимки членства текущего чата. Неизвестный username
не считается доказанным non-member. Неоднозначный username не даёт вклад;
последний наблюдённый non-member отделён от неизвестного статуса.
Тег сам по себе не разрешает капчу: нужен рекламный Decision verdict и общий
`moderation.captcha_threshold`. Legacy content и Decision content объединяются
максимумом, чтобы не учитывать один текст дважды.

## Выборка и действие

`initial_texts` (1..10) позволяет проверить первые N авторских текстов новичка
без словарного сигнала; служебные сообщения не занимают слот. Остальные тексты
проверяются при локальном, внешнем контактном или обфускационном сигнале.
Сохраняются существующие пределы возраста/числа сообщений новичка,
`max_calls_per_hour`, `max_pending_jobs`, consent, normal/admin/bot veto.
API attempt расходует бюджет даже при ошибке; retries ограничены.

Validated response атомарно создаёт action outbox; отдельный fenced claim
повторно проверяет исходный текст, edit, label, scope и policy hash, обновляет
канонические метрики и сравнивает порог. `captcha_dry_run` не вызывает Telegram.
`captcha_max_message_age_sec` (по умолчанию 600) запрещает ретроспективные вызовы
на старые сообщения. Не требуется завершение legacy generative audit. Если baseline/profile audit
завершился позже и общий балл пересёк порог, подпороговый положительный результат
Decision повторно обрабатывается без нового API-вызова.

Капча отправляется персональным ephemeral reply на собственный flagged comment.
После подтверждённого API ack/сохранения карточки удаляются только собственные
подтверждённые рекламные тексты, без соседних обычных сообщений. Ошибка доставки
не запускает TTL/cleanup/ban и не даёт публичного fallback. Правильный ответ
восстанавливает только ограничение бота. При `ban_on_failure=true` неуспех
доставленной капчи блокирует чат/комментарии без автоматического unban, в том
числе для автора вне группы. Прямого бана по одному Decision ответу нет.
Ephemeral API ack не доказывает прочтение на телефоне; Telegram read receipts нет.

Правка/удаление source или ручной normal отзывают вклад и pending challenge;
setup worker снимает только собственный mute. После доставки рост счётчика
сообщений не позволяет сбросить капчу. Чужие ограничения не снимаются; отменённый администратором mute не накладывается повторно.
Уже пройденная captcha сохраняет прежнюю политику `AlreadyPassed`.

## Включение и rollback

Применить обе additive migrations `20261113130000`, `20261113131500` к каждой
собственной БД. Historical migrations и публичный MCP manifest не меняются;
raw answers/outbox не выдаются read-only MCP роли.

В копии существующего профиля добавить, сохранив остальные настройки:

```toml
[moderation.decision]
# Остальные provider/consent/budget настройки прежние.
mode = "score_captcha"
score_weight = 90
external_contact_weight = 8
initial_texts = 10
ban_on_failure = true
captcha_max_message_age_sec = 600
```

До restart выполнить `validate_instance_config` с реальным окружением unit,
проверить captcha enabled, threshold, TTL, dry-run и оба instance scope.
Бинарники брать только из Ubuntu 24.04 release artifact. Сначала подготовить
migration-compatible rollback artifact с обеими новыми миграциями и старым
runtime: старый production binary без новых embedded migrations не стартует
после обновления SQLx ledger. Откатить профиль (удалить новые ключи при откате
старого parser) и использовать совместимый artifact; applied migrations не
удалять. Консервативный старый runtime обслуживает существующие challenges по
прежней expiry policy без нового permanent ban-on-failure.

Операционный контроль: action_status/error_kind, private metrics_json,
общие risk_decision_score/risk_contact_score, source revocations,
доставленные/пройденные/истёкшие challenges, бюджет/429 и возраст очереди.
Публичные карточки не содержат raw вероятностей или lookup-контактов.

## Проверка провайдера 13 ноября 2026

Отправлены 22 text-only запроса через официальный google-genai SDK и текущие
ключи/модели двух instance, с учётом их общего hourly budget; Telegram actions
не выполнялись. 13 положительных примеров: по одному тексту от 12 ранее вручную
размеченных спамеров плюс новый recruitment/contact пример. 9 отрицательных:
пять обычных авторов и четыре синтетических контроля (помощь с внешним тегом,
цитата спама, поиск работы, приветствие). Все ответы прошли Rust schema validation.
Медиана 327 мс, максимум 1116 мс; input 20699 tokens, output 0.

Прежний shadow verdict выделил 11/13 положительных и 0/9 отрицательных.
Два завуалированных movie/book funnel получили promo 0.911/0.884, но максимальная
отдельная рекламная категория была 0.499/0.430. Для общей метрики рекламная
масса учитывается суммарно: 13/13 положительных проходят semantic candidate,
0/9 отрицательных. При weight=90 content score составляет 80..88; 11/13
пересекают threshold=85 одним content, ещё двум нужен baseline >=3/5 либо
подтверждённый дополнительный вклад. Weight=95 оставляет один пример подпороговым
без baseline, weight=100 ловит все13 отдельно; эти значения не включены:
сохранена модель общей оценки и независимого supporting-вклада.

Это небольшая выборка с авторской разметкой, не независимый holdout и не оценка
популяционного FP. Text-only replay не проверяет avatars/весь production payload.
Нельзя заключать, что production recall=100% или что API ack означает прочтение
капчи пользователем. Полные тексты, ID и ответы сохранены только в private
диагностических файлах; в репозитории их нет.
