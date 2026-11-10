# Локальный каскад до LLM

Секция moderation.local_cascade добавляет ранний текстовый путь в существующий
fenced new-user audit. Используются имеющаяся word/char модель и корпус
последних ручных решений своей БД/чата. Добавление подтверждённого примера
не требует правки Rust или перезапуска.

## Режимы и границы

- disabled — default; нет дополнительных запросов корпуса или inference.
- shadow — сохраняет локальную оценку в _local_cascade, продолжает обычный
  LLM audit. Экономии LLM в этом режиме нет.
- review_only — при совпадении с независимыми шаблонами либо высоком linear
  score сохраняет локальный assessment и пропускает LLM, загрузку аватара,
  embedding и CAS в этой audit job. Итог — кандидат ручной проверки,
  без права на бан, удаление или капчу. Normal-shortcut пока нет.

Прежний deterministic fast path остаётся первым. Известный спамер,
актуальный not_spam, положительные avatar/personal-channel компоненты или
уже имеющееся auto-ban evidence исключают новый shortcut. Профиль/аватар
текстовым решением не признаются чистыми; сохранённые независимые компоненты
не обнуляются. Бюджет LLM для остальных audit jobs не меняется.

Рабочее окно ограничено числом сообщений и возрастом в чате. Проверяются
first preview и до десяти recent previews, включая спам после приветствия.
Live, replay и offline training используют один bounded/redacted input:
280 символов плюс возможное многоточие, URL заменены на [link]. Это тот же
канонический snapshot, что уже используется unified audit; за пределами
preview могут остаться неувиденные признаки.

Корпус выбирается только по настроенному managed moderation chat. Личка и
чужие чаты не читаются. Последняя метка должна быть spam/not_spam от
owner_manual, owner_review или legacy_backfill; более новое system решение
не подтверждает старую ручную метку. Включаются максимум десять сообщений
автора до метки; spam требует manual stamp. Правки после метки исключаются.
Это **разметка автора**, а не независимая проверка каждого его сообщения.
Собственные spam-примеры кандидата не подтверждают шаблон. Собственный
not_spam и похожий проверенный normal запрещают локальный review.
Дубликаты одного автора не дают независимых голосов.

Переполнение max_corpus_examples означает abstain, а не отбрасывание
отрицательных примеров. Нормализация выполняется один раз на job; поиск
пока линейный. При росте корпуса нужен индекс с сохранением normal-veto.

Перед materialization источники и policy проверяются заново. Отозванное
решение возвращается в generation под новыми монотонными fences; прежний
worker не может его восстановить. Отзыв после завершённого review сам по
себе не запускает массовую переоценку старых кандидатов. Следующие audits
используют актуальный корпус. Гонка между последней проверкой источника и
записью может оставить устаревший **review**, но не enforcement.

## Включение и rollback

Требуются compiled feature moderation, включённые moderation/new-user audit
и обычный валидный audit route для fallback. Линейная модель необязательна:
без linear_spam_enabled/model работает template-only путь.

Секция из config/llm_profiles.toml.example задаёт все пороги и ограничения.
Начальные значения: similarity 0.85, два независимых автора, normal-veto 0.80,
linear probability 0.99, минимум 32 символа/5 tokens, до 10 сообщений и трёх
дней в чате. Это не оценка вероятности правильной модерации. Новых порогов
enforcement нет.

Сохранить private backup instance TOML, добавить только новую секцию,
проверить validate_instance_config с прежними secrets/env и перезапустить
оба community instance по deployment runbook. Остальные флаги, модели,
routes и MCP scope не переписывать.

Для возврата к обычному LLM достаточно mode=disabled и рестарта:
неприменённые local assessments вернутся в generation. Завершённые review
scores этим не удаляются. При rollback на старый binary убрать новую секцию
или восстановить старый TOML: старый parser может её не понимать.

## Private export, replay и обучение

Пути ниже private и исключены из git. Выбирать профиль и DATABASE_URL
конкретного инстанса; DSN должен принадлежать той же БД. MCP read-only DSN
не подходит. Не объединять текстовые корпуса инстансов автоматически.

~~~sh
umask 077
mkdir -p work/cascade
# В окружении уже заданы DATABASE_URL и LLM_PROFILES_PATH своего инстанса.
cargo run --locked --bin spam_cascade -- export work/cascade/reviewed.jsonl
cargo run --locked --bin spam_cascade -- replay \
  work/cascade/reviewed.jsonl /private/current-model.json work/cascade/replay.json
# Для template-only replay вместо пути модели передать "-"
cargo run --locked --bin spam_cascade -- tokens \
  work/cascade/reviewed.jsonl work/cascade/tokens.jsonl
python3 -m venv work/cascade/venv
work/cascade/venv/bin/pip install -r eval/reviewed-spam-requirements.txt
work/cascade/venv/bin/python eval/train_reviewed_spam.py \
  work/cascade/tokens.jsonl work/cascade/candidate
cargo run --locked --bin spam_cascade -- verify \
  work/cascade/reviewed.jsonl work/cascade/candidate/model.json \
  work/cascade/candidate/parity.json
~~~

Файлы создаются только новыми с 0600, candidate directory — 0700. Тексты,
token dump, словарь модели и идентификаторы авторов не коммитить.
Export/replay/training не пишут в production БД и не вызывают LLM/API.
Trainer не устанавливается фоновой production job.

Replay считает 30 комбинаций порогов, holdout по автору и вариант только с
метками до времени сообщения. Export содержит последние метки, без полной
отозванной истории; существующая модель могла видеть старых авторов.
Review coverage не равна end-to-end recall или измеренной экономии API.

Trainer использует Rust tokens для parity, TF-IDF fit только на train,
шесть вариантов C и раздельные validation/test. Автор и точная кампания
объединяются в связные компоненты до split. Дополнительно проверяется
временной cutoff без будущих меток и пересекающихся компонентов.
Семантически близкие кампании могут оставаться в разных частях: это
отдельно требует кластеризации. Суммарный вес одного автора равен единице.

При нехватке классов/holdout trainer пишет insufficient_split; иначе —
неутверждённый candidate с нулевыми supporting scores и test/temporal
метриками. Он **никогда автоматически не заменяет** рабочую модель.
Ноль false positives на нескольких normal авторах не обосновывает
enforcement: нужен большой независимый normal holdout и новые кампании.
Сначала сравнить candidate через replay, проверить Python/Rust parity,
затем shadow на свежем потоке.

Автоматически без правки правил обновляются шаблоны из журнала решений.
Плановое candidate training и shadow rollout — следующий этап.

## Наблюдение

Jobs сохраняют provider local_cascade, model/version, reason,
text/policy/corpus hash, probability, число авторов и ссылки
(label_event_id, user_id, message_id) первых двадцати независимых источников.
Тексты в новое evidence не копируются. В materialization сохраняется
local_cascade_review_candidate с auto_ban_eligible=false.

~~~sql
select provider, status, materialization_status, count(*)
from new_user_audit_jobs
where created_at >= now() - interval '1 day'
group by provider, status, materialization_status;
~~~

Считать долю local jobs среди сравнимых завершённых audits отдельно по
инстансам и периодам. Это не экономия на всех сообщениях: часть audits
уже избегала LLM через прежние deterministic rules.
local_evidence_revoked — ожидаемый fallback после отзыва/смены policy.

Механизмы TF-IDF и группового split описаны в
[документации scikit-learn](https://scikit-learn.org/stable/modules/generated/sklearn.feature_extraction.text.TfidfTransformer.html)
и [cross-validation](https://scikit-learn.org/stable/modules/cross_validation.html).
