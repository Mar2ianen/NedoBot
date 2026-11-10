# Production deployment

Этот runbook описывает выкладку NedoBot на vps-153 (hostname сервера —
vps-13176). Deploy выполняется из слитого main, а не из рабочей feature
ветки. Фактически запущенный commit фиксируется immutable tag
deploy-YYYY-MM-DD-scope.

## Последняя фактическая выкладка

10 ноября 2026 в **16:27:35 UTC / 19:27:35 МСК** применён config-only follow-up
[deploy-2026-11-10-cloudflare-memory](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-11-10-cloudflare-memory)
на том же source 4afef1add94e56e9f4eba4eae5eb9491a5a4e2ab. Binaries,
SHA-256, manifests, scope и миграции соответствуют описанному ниже release.

В extended observation после первоначального audit rollout обнаружены две
Rust ERROR записи NedoNews в 16:01:16 и 16:02:06 UTC: post-history задачи
упали на старом memory route с HTTP 402 от Ollama. Поэтому первоначальное
33-секундное окно без ошибок не означает, что весь первый период был чистым.
Ошибок Cloudflare audit в этих двух записях нет.

Оба memory route теперь используют cloudflare_gemma_memory: та же Gemma,
JSON Object вместо JSON Schema, прежний typed output validator, images=false,
thinking=false, timeout=60s, runtime.memory_llm_max_tokens=512. Audit сохраняет
JSON Schema и бюджет 900; все прочие поля профилей сравнены с backup и сохранены.
Причина выбора режима и synthetic probes описаны в [runbook](CLOUDFLARE_GEMMA.md).
Оба candidate profiles прошли validate_instance_config; оба бота и public MCP
перезапущены, LoadCredential public MCP совпадает с текущим NedoNews profile.

После root-only snapshot двух failed history rows они однократно поставлены
на повтор с проверкой instance scope. Attempts не сбрасывался (10 → 11).
В 16:31:06 UTC обе задачи подтверждены ready от Cloudflare, с summary и
embedding_gemma2; error_kind=null. Это реальная проверка штатного Rust-worker,
не только synthetic HTTP probe. Telegram сообщения не отправлялись.
Старые failed new-user audits пакетно не переоткрывались.

Backup профилей перед memory change и private recovery evidence:
 /var/backups/nedobot/2026-11-10-cloudflare-gemma/memory-followup.
Для отката только follow-up восстановить nedonews-audit-profile.toml и
pvo-audit-profile.toml в соответствующие рабочие profile paths и перезапустить
оба бота и public MCP; текущие binaries поддерживают оба варианта.
Историю сводок и attempts не откатывать. Credentials и billing plan сохранены.


10 ноября 2026 в **15:56:43 UTC / 18:56:43 МСК** установлен
[deploy-2026-11-10-cloudflare-gemma](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-11-10-cloudflare-gemma)
из source 4afef1add94e56e9f4eba4eae5eb9491a5a4e2ab после
[PR #108](https://github.com/Mar2ianen/NedoBot/pull/108). До install проверены
ancestry и совпадение полного tree с merged main
218d75838264a920e6fcfdb44934ac5dee771ed4.

[Workflow 42847089914](https://github.com/Mar2ianen/NedoBot/actions/runs/42847089914)
успешно **собрал** artifact на Ubuntu 24.04 с glibc guard ≤ 2.39, deploy=false.
Установка выполнена по SSH installer-ом из artifact после SHA256SUMS,
preflight обоих candidate profiles и live Rust transport probe. В **15:55:53 UTC**
завершились все 12 synthetic probes: переставленные цвета двух JPEG и полный
audit schema во всех сочетаниях наличия картинки/первого сообщения, отдельно
с profile и EnvironmentFile каждого инстанса. Чужие fallback providers в probe
исключались; Telegram/БД и реальные сообщения не использовались.

SHA-256 обоих running bot executable:
ce454935a9794e6f959b8048d10ece05ced534aa9a6adad05a4745e9cd24f569.
Running public MCP:
26be9adc40da790e44753669a3c1749f4c612a6545e0058d5af86cb865a76d31.
Оба internal chat_db_mcp:
f6f5d55ee875fe08627ff103b6650df2cfd9875538d103a6bd70c16e9a029152.
Новых миграций нет: в обеих БД latest=20261108130000, failed=0.

В обоих private profiles добавлены Cloudflare provider/model и изменён только
routes.new_user_audit: primary — @cf/google/gemma-4-26b-a4b-it, images=true,
JSON Schema, timeout=60s, chat_template_kwargs.enable_thinking=false.
Прежние Ollama profiles удалены только из audit route из-за HTTP 402.
Текстовые Groq fallbacks PVO сохранены; при изображении resolver их пропускает,
сохраняя исходную modality. Другие routes, local cascade, enforcement,
captcha и delivery flags не менялись. Credentials и billing plan не менялись.
Подробности wire contract и общей account quota: [runbook](CLOUDFLARE_GEMMA.md).

**Initial SSH smoke — 15:57:15 UTC, окно 33 секунды:** все три unit
active/running, NRestarts=0, running hashes совпали с artifact; Rust ERROR и
journal priority err отсутствуют, embedding unit active. Этот smoke выполнен
отдельно от build-only CI. Stdio и ACL probes обеих БД подтвердили allowed SELECT,
отказ foreign scope/forged bootstrap/private memory и запрет raw tables/scope/
writes. Manifest файлы и actual ASK_DATABASE_URL role nedobot_mcp_ro сохранены;
public MCP использует обновлённый profile через прежний LoadCredential.

Backup совместимого предыдущего installer-набора, profiles/env/manifest/drop-in:
 /var/backups/nedobot/2026-11-10-cloudflare-gemma. Для rollback восстановить
предыдущие profiles вместе с binaries: старый parser не знает новое model поле.
DB rollback не нужен. Native preflight и health evidence сохраняются там же.
Telegram smoke-сообщения не отправлялись, старые failed audits не переоткрывались
пакетно. Проверка подключения не является измерением качества классификации
или точности новой модели на независимом spam holdout.

[CI 42847084034](https://github.com/Mar2ianen/NedoBot/actions/runs/42847084034)
для точного 4afef1a прошёл: workspace tests, strict Clippy, fresh migrations,
PostgreSQL/MCP/ACL. Локально те же проверки и installer tests прошли,
включая 527 unit tests приложения.


10 ноября 2026 в **14:27:11 UTC / 17:27:11 МСК** установлен
[deploy-2026-11-10-local-audit-cascade](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-11-10-local-audit-cascade):
source commit 8fafcbf190a98c7cbc143b2b02c2d2f5a1bad930 после
[PR #106](https://github.com/Mar2ianen/NedoBot/pull/106). Ancestry и полное
совпадение tree с merged main 791d380124f07ed22bb715c9a7890e1896f8076b
проверены до установки.

[Release workflow 42831798569](https://github.com/Mar2ianen/NedoBot/actions/runs/42831798569)
только собрал artifact на Ubuntu 24.04 и проверил glibc ≤ 2.39, deploy=false.
Установка выполнена через SSH тем же installer из artifact. Все SHA256SUMS
сверены локально и на VPS; preflight обоих candidate profiles, private export
и replay выполнены самим release binary на production БД до рестарта.
Export дал 233 NedoNews и 110 PVO сообщений. Это read-only проверка без LLM/API.

SHA-256 обоих running bot executable:
4fcdae922e4a77b67c15bb2d1aaf4f896427aa70ddfabaf4fbbdb6aa19c5aa08.
Running public MCP:
e897745b327992cb978d7ad0965131366fe4e176516b0e57cf682eb0b4ca9293.
Оба internal chat_db_mcp:
1881e6822e635c1f3c7ae01c5c1933d270faa1ff51ea72afa1e3071eeadd4cda.
Обе БД: latest migration 20261108130000, failed=0; новых миграций нет.

В каждом operator-owned profile добавлена только moderation.local_cascade:
review_only, similarity=0.85, два автора, normal-veto=0.80, linear=0.99,
окно до 10 сообщений/трёх дней. NedoNews сохраняет прежнюю word/char модель,
PVO использует template-only. Существующие enforcement/captcha флаги и
выключенная доставка review-карточек сохранены. Local candidate не разрешает
бан, удаление или капчу. Автоматически обновляется корпус ручных меток;
новый offline candidate не продвигался. См. [runbook](LOCAL_ANTISPAM_CASCADE.md)
и [ограничения replay](ANTISPAM_CASCADE_EVAL_2026-11-10.md).

**Initial SSH smoke: 14:28:24 UTC**, окно **73 секунды** с рестарта.
Все три unit active/running, NRestarts=0, process hashes совпали с artifact;
Rust ERROR и journal priority err отсутствуют, embedding unit active.
Это отдельная ручная проверка, а не утверждение о coverage build-only CI.
Повторены stdio initialize/tools/select, отказ на foreign chat/private memory
и forged bootstrap для обеих БД. ACL smoke подтвердил reviewed-view access
и запрет raw messages, private scope, conversation memory и writes.
Фактический ASK_DATABASE_URL обоих ботов использует nedobot_mcp_ro,
manifest файлы не менялись; public MCP загрузил текущий профиль через прежний
LoadCredential. Telegram smoke-сообщения не отправлялись.

Совместимые предыдущие binaries, installer, profiles/env/manifest/drop-in:
 /var/backups/nedobot/2026-11-10-local-cascade. Private validation exports/reports
лежат там же в validation/. Для rollback восстановить предыдущие profiles
вместе с предыдущим installer-набором: старый parser не понимает новую секцию.
DB rollback не нужен; credentials не менялись.

[CI 42831798359](https://github.com/Mar2ianen/NedoBot/actions/runs/42831798359)
и [CI 42831813946](https://github.com/Mar2ianen/NedoBot/actions/runs/42831813946)
для точного 8fafcbf прошли: workspace tests, strict Clippy, fresh PostgreSQL,
MCP/ACL и отрицательные local-review/fencing сценарии. Локально scripts/test.sh,
format и Clippy также прошли, включая 525 unit tests приложения.

Отдельное прежнее ограничение: до этого деплоя 12 NedoNews audits за сутки
завершились HTTP 402 от ollama_cloud / gemma4:31b (05:59–12:28 UTC).
Это не runtime-регрессия нового release. Эти terminal jobs не переоткрывались
вслепую с повторными платными вызовами; local shortcut помогает совпадениям,
остальной audit зависит от доступности configured provider.

9 ноября 2026 в **20:46:38 UTC / 23:46:38 МСК** установлен correction release
[`deploy-2026-11-09-private-note-scope`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-11-09-private-note-scope)
с source commit `d3b931deeff79cc3ef2714adb2875707c87f8f47` после
[PR #104](https://github.com/Mar2ianen/NedoBot/pull/104). Этот commit входит в
`main` через merge `0d8920fc12a10ebe59eadb11c59e9418a1d763cc`; перед установкой
проверены ancestry и полное совпадение trees source commit и merged main.

[Workflow 42694657594](https://github.com/Mar2ianen/NedoBot/actions/runs/42694657594)
**только собрал** artifact на Ubuntu 24.04 и проверил glibc ≤ 2.39
(`deploy=false`). Установка выполнена оператором через SSH с тем же
`install_release_binaries.sh`, а не deploy-job этого workflow. Проверены все
`SHA256SUMS`, оба runtime-профиля и совпадение DB scope; installer атомарно
обновил оба checkout, затем перезапущены оба бота и public MCP. Новых миграций,
scope changes или правок конфигурации в correction pass нет.

SHA-256 обоих running bot executable:
`659a9fccbd59034f6b94e8d59213705d2d18ffd71e04f9dececbbe522f010558`.
SHA-256 running public MCP:
`d0854d8f4b036a2c2a957537f756eb654e7d2e2170d5a06e79b3375e6060c7f8`.
В обеих БД по-прежнему последняя migration `20261108130000`, failed = 0.
Совместимый предыдущий набор бинарников, текущие профили/env/manifest и
systemd drop-in сохранены в
`/var/backups/nedobot/2026-11-09-private-note-scope`; `previous-release/`
содержит полный installer-набор и checksums для возврата к `5e7f435` без
изменения БД.

**Ручной initial smoke:** 9 ноября в **20:47:59 UTC**, около 82 секунд с
рестарта в 20:46:38. `systemctl show` подтвердил active/running,
`NRestarts=0` у всех трёх unit; `/proc/<MainPID>/exe` и оба `chat_db_mcp`
совпадают с artifact. В полном журнале с рестарта нет Rust `ERROR`,
`journalctl -p err` также пуст; embedding unit active. Это отдельная
SSH-проверка, не утверждение о coverage build-only workflow.

После restart повторены stdio initialize/tools/select для обеих БД, отказ
на чужой chat filter, forged bootstrap scope и private conversation memory.
PostgreSQL ACL smoke подтвердил reviewed-view access и запрет raw messages,
private scope, conversation memory и записи от `nedobot_mcp_ro` в обеих БД.
Подключённый public HTTP connector также успешно выполнил scoped SELECT.
Проверочные сообщения в Telegram чаты не отправлялись.

Исправление запрещает `notes.add_user` публиковать факт из private/foreign
conversation в shared retrieval scope — и на границе agent tool, и в обоих
persistence helpers. Разрешённые групповые notes и private conversational
memory сохраняют прежнюю семантику. На проверке после выкладки в обеих БД
нет исторических `notes.add_user` tool calls из private `ask_runs` и нет новых
user notes с первого restart в 20:25:48; пользовательские данные не удалялись.

[CI 42694490354](https://github.com/Mar2ianen/NedoBot/actions/runs/42694490354)
для точного `d3b931d` успешен: workspace tests, strict Clippy, fresh migrations,
PostgreSQL negative note-write regression и MCP/ACL integration. Те же проверки
прошли локально через `scripts/test.sh`.


9 ноября 2026 в **20:25:48 UTC / 23:25:48 МСК** выпущен release
[`deploy-2026-11-09-dependencies-agent-gateway`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-11-09-dependencies-agent-gateway)
на merge commit `5e7f43501a0c1ae1ac3cdb685dcf6a2ed3b8715e` после
[#101](https://github.com/Mar2ianen/NedoBot/pull/101),
[#102](https://github.com/Mar2ianen/NedoBot/pull/102) и
[#103](https://github.com/Mar2ianen/NedoBot/pull/103).
[Release workflow 42691285757](https://github.com/Mar2ianen/NedoBot/actions/runs/42691285757)
собрал default-features binary на Ubuntu 24.04, проверил glibc ≤ 2.39,
валидировал оба operator-owned профиля, установил artifact и перезапустил
основной бот, PVO и public MCP.

SHA-256 обоих работающих bot executable:
`97d6d35c9376e3e05a3b935d35cf9c5690975de1d223deb853f6c5d6eb201544`.
SHA-256 public MCP:
`704bac908a1c9094c2821e7c7ce6459081f57d302ddb8eaeaa9ad883c407eca4`.
Running hashes сверены с artifact; internal `chat_db_mcp` в обоих checkout
также соответствует artifact. В обеих БД применена последняя миграция
`20261108130000`, failed migrations = 0. Historical migrations сохранены.

Перед restart оператор отдельно применил миграции и provision-ил каждый MCP
scope из собственного профиля; новые manifest проверены по точным именам,
типам и nullable-статусу колонок reviewed repository manifest. NedoNews
использует `/etc/tg-ai-bot/mcp_db_manifest.toml`, PVO —
`/etc/tg-ai-bot/pvo-mcp_db_manifest.toml`. Для существующего NedoNews HTTP
endpoint явно включён `public_mcp.enabled`; в PVO он выключен. Профиль и manifest
для `nedonews-mcp` передаются через systemd `LoadCredential`, без расширения
прав на `/etc/tg-ai-bot`. Gateway и conversation memory доставлены как opt-in
возможности; их runtime включение не менялось, PVO `/ask` остаётся выключенным.

**Источник ручного initial smoke:** SSH-проверка 9 ноября в **20:26:59 UTC**,
окно журнала **20:25:48–20:27:00 UTC** (72 секунды), отдельно от workflow.
`systemctl show` подтвердил active/running и `NRestarts=0` у всех трёх unit;
SHA-256 `/proc/<MainPID>/exe` совпали с `SHA256SUMS` artifact. Проверены как
`journalctl -p err`, так и Rust `ERROR` в полном журнале трёх сервисов — 0.
`nedobot-rag-embedding` active. Это initial smoke, не заявление о десятиминутном
окне в CI.

После restart выполнены реальные stdio initialize/tools/select для обеих БД;
чужой chat filter, поддельный bootstrap scope и доступ к conversation memory
отклонены. Public HTTP через подключённый NedoNews v2 connector вернул только
свой chat ID и отклонил foreign scope/private memory. Отдельный SQL smoke через
`SET ROLE nedobot_mcp_ro` в обеих БД подтвердил доступ к reviewed views и отказ
на raw messages, private scope, conversation memory и write-запросы. Native
vision/tool preflight на CI artifact с тем же runtime tree вернул ожидаемый
`inspect(query=test)` через фактический `ask` route. Сообщения-smoke в Telegram
чаты не отправлялись.

Сохранены согласованные PostgreSQL dump обоих инстансов (каталоги прочитаны
`pg_restore --list`, checksums записаны вне Git), а также копии профилей,
manifest, env и прежних бинарников в
`/var/backups/nedobot/2026-11-09-dependency-refresh` на VPS.
Ограничения отката после новых миграций описаны в
[инструкции обновления зависимостей](DEPENDENCY_REFRESH_2026-11-09.md).

Проверки #103: [CI 42689237802](https://github.com/Mar2ianen/NedoBot/actions/runs/42689237802)
и [CI 42689276022](https://github.com/Mar2ianen/NedoBot/actions/runs/42689276022)
успешны, включая workspace tests, Clippy, fresh migrations двух БД и MCP/ACL
integration. План сокращения ручных lexical rules и LLM calls:
[ANTISPAM_LEARNING_PLAN.md](ANTISPAM_LEARNING_PLAN.md).


7 ноября 2026 в **10:41 UTC / 13:41 МСК** выпущен release
[`deploy-2026-11-07-casual-paid-task-signal`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-11-07-casual-paid-task-signal)
на merge commit `cbf3de07216a0ef9e76fbf5395be28a8758631ee` после PR
[#98](https://github.com/Mar2ianen/NedoBot/pull/98). [Release workflow
42364376332](https://github.com/Mar2ianen/NedoBot/actions/runs/42364376332)
успешно установил artifact в основной и PVO инстансы и перезапустил public MCP.
SHA-256 обоих работающих bot executable —
`721d8b094930b6ee3c9ee8b019fcab67203fdebfa6c840da81fa6b57e97d0c8f` — совпадает
с artifact. Все три unit-а active; workflow подтвердил отсутствие прироста
`NRestarts`, пустой error-level journal после выкладки и migration
`20261105190000` в обеих БД. Добавлена review-only кампания для короткого
неформального задания с конкретной суммой, сроком около часа, оплатой сразу
после выполнения и приглашением написать в ответ. Совпадение создаёт audit
candidate без LLM-вызова, но само по себе не ставит spam-label, не банит и не
удаляет сообщения.

6 ноября 2026 в **22:44 UTC / 7 ноября 01:44 МСК** выпущен release
[`deploy-2026-11-07-review-campaign-safety`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-11-07-review-campaign-safety)
на merge commit `43d94871a1bc1a78e14b06b3b5714d28e71d8edd` после PR
[#96](https://github.com/Mar2ianen/NedoBot/pull/96). [Release workflow
42305860555](https://github.com/Mar2ianen/NedoBot/actions/runs/42305860555)
успешно установил artifact в оба community-инстанса и public MCP. SHA-256
обоих работающих bot executable —
`29886991d026d45b4e810252ad78d39327a62f39a025649d720d1e87edbf93b6`.
Миграция `20261105190000` подтверждена в обеих БД; все три unit-а active.
После release workflow PVO профиль был возвращён из временного dry-run в
`enforce_enabled=true`, `enforce_dry_run=false`, threshold `90`. PVO перезапущен
7 ноября в 09:07 UTC: оба bot hash совпадают с artifact, все три unit-а active,
`NRestarts=0`, error-level journal пуст. Временный профиль сохранён в
`/etc/tg-ai-bot/pvo-llm_profiles.toml.bak-20261107-deterministic-campaign-safety`;
backup состояния dry-run перед восстановлением —
`/etc/tg-ai-bot/pvo-llm_profiles.toml.bak-20261107-postdeploy-dryrun`.

Исправление #96 переводит все текущие детерминированные кампании в review-only:
нейтральный сигнал сохраняется для аудита, но не создаёт System spam-label,
не вызывает автобан или удаление и не является ban evidence. Независимые
подтверждённые identity/template-сигналы сохраняют прежнюю политику. Перед
любой автомодерацией проверяется последняя ручная метка `not_spam`, в том числе
ещё раз перед вызовом Telegram ban API. После релиза #94 до временного dry-run
не было новых spam-label или auto-enforcement событий в обеих БД.

6 ноября 2026 в **19:21 UTC / 22:21 МСК** выпущен release
[`deploy-2026-11-06-configurable-spam-features`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-11-06-configurable-spam-features)
на merge commit `78e5a4f1d12a6e25614293d17976eff279586b5f` после PR
[#94](https://github.com/Mar2ianen/NedoBot/pull/94). [Release workflow
42277501394](https://github.com/Mar2ianen/NedoBot/actions/runs/42277501394)
успешно установил artifact в основной и PVO инстансы и перезапустил public MCP.
SHA-256 работающих bot binaries на `tg-ai-bot-teloxide` и `nedobot-pvo`
совпадает: `f55d0a42b3c1d7621af0c40c7d8b62c58eb43fc442f06efe694ce91aa73df070`.
Все три unit-а active; workflow подтвердил отсутствие новых рестартов, пустой
error-level journal с момента выкладки и миграцию `20261105190000` в обеих БД.
Выпуск выносит детерминированные spam-сигналы в общий конфигурируемый набор
признаков и правил кампаний: просьбы о деньгах в личку, платные задания и найм,
финансовые предложения, раздача контента, ограниченные предложения и adult
каналы. При совпадении правила fast path срабатывает до LLM; существующие
moderation policy gates сохранены.

6 ноября 2026 в **15:24 UTC / 18:24 МСК** выпущен release
[`deploy-2026-11-06-money-dm-campaigns`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-11-06-money-dm-campaigns)
на merge commit `7d0e9c7cf234f69c700d223e975b0d2d1b4a108f` после PR
[#92](https://github.com/Mar2ianen/NedoBot/pull/92). [Release workflow
42241754588](https://github.com/Mar2ianen/NedoBot/actions/runs/42241754588)
успешно собрал и установил binary в оба community-инстанса и public MCP.
SHA-256 обоих работающих bot executable —
`3c27e0665c1692f4d10630ac9df96a4486bb82a3fe1d9fa9692f89e5d8de9527`.
После релиза в PVO включён `enforce_dry_run=false`; перед правкой профиля
сохранён backup `pvo-llm_profiles.toml.bak-20261106-money-dm-campaign`.
Финальная проверка после рестарта PVO: три unit-а active, `NRestarts=0`, хеши
запущенных ботов совпадают с artifact, error-level journal пуст. Миграция
`20261105190000` подтверждена в обеих production БД. На этот выпуск также
размечены три подтверждённых вручную заблокированных денежно-DM спамера.
Детерминированный фильтр теперь ловит прямую просьбу о деньгах с CTA в личку,
учитывает homoglyph-варианты, действует в пределах 10 сообщений и трёх дней и
может банить совпавшую кампанию без LLM-вызова.

5 ноября 2026 в **19:41 UTC / 22:41 МСК** выпущен release
[`deploy-2026-11-05-audit-review-fencing`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-11-05-audit-review-fencing)
на merge commit `8695830e8d78d4ac68c465e6e1883ef347934570` после PR
[#89](https://github.com/Mar2ianen/NedoBot/pull/89), устраняющего гонки generation/materialization,
stale deterministic labels и потерю score-компонентов. [Release workflow
42095198276](https://github.com/Mar2ianen/NedoBot/actions/runs/42095198276)
успешно установил binaries и перезапустил НедоNews, PVO и public MCP.
При отдельной ручной проверке по SSH и SQL в 19:42 UTC подтверждены SHA-256
обоих bot binaries (`e74038868a93bd7a15cbcdae51946c82dc767cb6179e099c811c0066b27c28da`),
наличие migration `20261105190000` в БД `tg_ai_bot` и `tg_ai_bot_pvo`, активность
всех трёх unit-ов, `NRestarts=0` и пустой error-level journal за последние 10 минут.
В run `42095198276` явная проверка `_sqlx_migrations` ещё не включала новую
версию, поэтому применение подтверждено отдельным SQL-запросом. Этот follow-up
добавляет release workflow проверку последней migration собираемого ref и
проверку активности, прироста `NRestarts` и error-level journal для всех трёх
unit-ов.

5 ноября 2026 в **18:27 UTC / 21:27 МСК** выпущен release
[`deploy-2026-11-05-audit-trigger-fingerprints`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-11-05-audit-trigger-fingerprints)
на merge commit `50fd2de8d54be6384e9424ee99169f8b79a7cef6` после PR
[#87](https://github.com/Mar2ianen/NedoBot/pull/87). [Release workflow
42082883617](https://github.com/Mar2ianen/NedoBot/actions/runs/42082883617)
собрал и установил artifact в основной и PVO инстансы, применил проверки миграций
в обеих production БД и перезапустил public MCP. SHA-256 обоих bot binaries —
`2f42bdb82e8654becd23e762e46799d88ea69bb0990b5df26944f02090b8fd3b`; хеши
запущенных `tg-ai-bot-teloxide` и `nedobot-pvo` совпали с artifact. Все три
unit-а active, `NRestarts=0`, error-level journal после рестарта пуст; миграция
`20261018120000` есть в обеих БД. CI прошёл format, все targets, Clippy и
PostgreSQL integration suites.

Предыдущий deploy: 5 ноября 2026 в **17:10 UTC / 20:10 МСК**, tag
[`deploy-2026-11-05-audit-cost-gate`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-11-05-audit-cost-gate)
на commit `3c84c639db6858ff0bc5b0074a07cc8d35ef9174` после PR
[#85](https://github.com/Mar2ianen/NedoBot/pull/85). Release workflow
[42070378287](https://github.com/Mar2ianen/NedoBot/actions/runs/42070378287)
обновил оба бота, MCP и миграции; SHA-256 обоих bot binaries был
`30c9fc53fd45be5a95f79a89cadb052f1576679a74e24457dbfa74e7a07aae63`.

20 октября 2026 в **18:06 UTC / 21:06 МСК** выпущен release
[`deploy-2026-10-20-tls-security`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-20-tls-security)
на merge commit `5be14fcc14c6d4dbc9b984b5a2c40d1a1283d2fe` после PR
[#83](https://github.com/Mar2ianen/NedoBot/pull/83). [Release workflow
39470540838](https://github.com/Mar2ianen/NedoBot/actions/runs/39470540838)
установил бинарники в оба community-инстанса и public MCP. SHA-256 обоих bot
binaries — `43c7521b46a9c4679c3c7c9b1ea3b023af61a7034d7ba23921e86808fcc72486`,
MCP — `56f67e85f3594f081e664adaaa85085238f867af1fcdac052632ea6f1a55c16b`;
хеши запущенных процессов совпали с artifact.

В `Cargo.lock` обновлены `rustls` до 0.23.46, `rustls-webpki` до 0.103.16,
`reqwest` до 0.13.5, `rustls-platform-verifier` до 0.7.1 и совместимые TLS
зависимости. На `vps-153` исключён корень `emSign ECC Root CA - G3` в
`/etc/ca-certificates.conf`; `update-ca-certificates` удалил один сертификат
из системного bundle, а OpenSSL подтвердил, что корень больше не доверенный.
Резервная копия конфигурации:
`/etc/ca-certificates.conf.bak-emSign-20261020T175330Z`. После изменения
перезапущены `tg-ai-bot-teloxide`, `nedobot-pvo` и `nedonews-mcp`; все три
active, error-level journal пуст. TLS-проверки до `api.telegram.org` и
`generativelanguage.googleapis.com` прошли успешно. В CI
[39469538568](https://github.com/Mar2ianen/NedoBot/actions/runs/39469538568)
прошли форматирование, все targets, Clippy и PostgreSQL integration suites.

18 октября 2026 в **14:06 UTC / 17:06 МСК** выпущен release
[`deploy-2026-10-18-risk-captcha-ephemeral`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-18-risk-captcha-ephemeral)
на merge commit `89427ff54e6a18b44c2ed95e10f6c3bf95e2de1a` после review
[PR #81](https://github.com/Mar2ianen/NedoBot/pull/81). [Release workflow 39133862251](https://github.com/Mar2ianen/NedoBot/actions/runs/39133862251)
установил artifact в оба community-инстанса и перезапустил public MCP.
SHA-256 обоих bot binaries —
`eaf09b61010657bfbdd2c553fa564eb3197075352903ddb4d04c8b4e3cdafaf8`; оба
работающих процесса совпали с artifact.

Оба бота и `nedonews-mcp` active; error-level journal после рестарта пуст.
Новая миграция `20261018120000` применена в обе БД. CI
[39133347843](https://github.com/Mar2ianen/NedoBot/actions/runs/39133347843)
прошёл format, tests, Clippy и PostgreSQL integration suites. Капча теперь
отправляется эфемерно только проверяемому участнику; успешный ответ обновляет
его приватную карточку, неверные ответы не дают уведомлений, после трёх ошибок
кнопки убираются, а истёкшая карточка удаляется без текста о неудаче.

15 октября 2026 в **20:06 UTC / 23:06 МСК** выпущен release
[`deploy-2026-10-15-spam-patterns`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-15-spam-patterns)
на merge commit `155c47bd3da5718def0fae601bc64844f7c175f3` после review
[PR #79](https://github.com/Mar2ianen/NedoBot/pull/79). [Release workflow 38798335159](https://github.com/Mar2ianen/NedoBot/actions/runs/38798335159)
собрал и установил artifact на оба community-инстанса и перезапустил public MCP.
SHA-256 обоих bot binaries —
`24b8d268a0a19c0cf0d654b1a10dd40cacc6ee277c075d67ffc150f19b073c08`; оба
работающих процесса совпали с artifact.

Оба бота и `nedonews-mcp` active; error-level journal после выкладки пуст,
сообщений о пропущенных миграциях нет. CI
[38797292117](https://github.com/Mar2ianen/NedoBot/actions/runs/38797292117)
прошёл форматирование, тесты, Clippy и PostgreSQL integration suites. К
детерминированному аудиту свежих аккаунтов добавлены два подтверждённых PVO
шаблона: оплата «на руки» с личным CTA и короткая задача с суммой в рублях
после выполнения. Правило короткой задачи требует одновременно все признаки и
не более трёх сообщений за первые сутки; автоматическое действие по-прежнему
определяет общая moderation policy.

14 октября 2026 в **12:55 UTC / 15:55 МСК** выпущен release
[`deploy-2026-10-14-deterministic-spam`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-14-deterministic-spam)
на merge commit `a2dcc618bba80f19f699b589551a4d0b90119479` после review
[PR #77](https://github.com/Mar2ianen/NedoBot/pull/77). [Release workflow 38566401993](https://github.com/Mar2ianen/NedoBot/actions/runs/38566401993)
собрал и установил artifact на оба community-инстанса и перезапустил public MCP.
SHA-256 обоих bot binaries —
`2dec6fe5e691a6f44ab089b907177d619df6035036a748a488a54eba4367454b`; оба
запущенных процесса совпали с artifact.

Оба бота и `nedonews-mcp` active; error-level journal обоих ботов за 15 минут
после выкладки пуст, сообщений о пропущенных миграциях нет. CI
[38565369859](https://github.com/Mar2ianen/NedoBot/actions/runs/38565369859)
прошёл formatting, tests, Clippy и PostgreSQL integration suites.
Новые высокоуверенные кампании спама среди свежих пользователей теперь получают
system spam label без LLM-вызова; дальнейшее действие задаёт обычная политика
модерации инстанса. Это правило не выдаёт безусловный ban.

13 октября 2026 в **21:04 UTC / 14 октября 00:04 МСК** выпущен release
[`deploy-2026-10-14-stats-semantics`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-14-stats-semantics)
на merge commit `28bad6606531155454e860ab21344ff78d783b20` после успешных проверок
[PR #75](https://github.com/Mar2ianen/NedoBot/pull/75). [Release workflow 38466458639](https://github.com/Mar2ianen/NedoBot/actions/runs/38466458639)
установил artifact в оба community-инстанса и public MCP. SHA-256 обоих bot binaries —
`466e48b2a88dbe85b2329b3add137a02900c3fcf78216ff2beb9dc0f2bd0e0e2`; MCP —
`c75fa302535cc2771fe4828bbf6c2b9d5355e35b996b24515f4df34b64148a98`.

Все три unit-а active с `NRestarts=0`; хеши обоих bot executable и MCP совпали с artifact,
error-level journal после рестарта пуст. Статистические метрики приведены к общим определениям
для HTML и Rich отчётов: отдельно подписаны добавления реакций и обновления снимков, переходы
участия учитывают `restricted.is_member`, а 30-минутная активность показывается только для
полностью завершённых окон. PostgreSQL integration suite прошёл на тестовой БД.

12 октября 2026 в **09:21 UTC / 12:21 МСК** выпущен release
[`deploy-2026-10-12-spam-audit-embedding-fallback`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-12-spam-audit-embedding-fallback)
на merge commit `8eafb8d7019e49f424530a0090a1693c96b7784f` после review
[PR #73](https://github.com/Mar2ianen/NedoBot/pull/73). [Release workflow 38228410730](https://github.com/Mar2ianen/NedoBot/actions/runs/38228410730)
установил artifact в оба community-инстанса и public MCP. SHA-256 обоих bot binaries —
`3b8394451336738481de72f1933f31af388a34dffb70395af769136eb216ce3c`; MCP —
`bb000a8ea6c93987ef9c000a12f708d17ebbe72c2d25ca35e97a8df89baafbc0`.

Все три unit-а active с `NRestarts=0`; хеши обоих bot executable совпали с artifact,
error-level journal после рестарта пуст. Ошибка/невалидный ответ EmbeddingGemma теперь
пропускает только similarity и embedding-head: текстовая first-message оценка сохраняется
и передаётся автомодерации. После выкладки один из двух зависших PVO-аудитов получил
капчу с риском 100 и TTL 600 секунд; второй к моменту повторной проверки уже был banned,
поэтому автоматическое действие для него было пропущено.

`nedobot-rag-embedding` остановлен после 1 869 перезапусков и `SIGKILL` (exit status 137):
на VPS исчерпалась RAM и swap. Text-based аудит спама продолжает работать без embedding,
но retrieval/similarity функции, которым нужен этот сервис, остаются недоступны до
устранения нехватки памяти.

11 октября 2026 в **09:55 UTC / 12:55 МСК** выпущен release
[`deploy-2026-10-11-known-spammer-risk`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-11-known-spammer-risk)
на merge commit `9a5f54b839acdc94acef58ebeec075c81bb25f99` после review
[PR #70](https://github.com/Mar2ianen/NedoBot/pull/70). [Release workflow 38129601469](https://github.com/Mar2ianen/NedoBot/actions/runs/38129601469)
установил artifact в оба community-инстанса и public MCP. SHA-256 обоих bot binaries —
`0817aa8075e84e3b65b5cd4d6cd6edc2896cf00f6d5b4363678a4beb0293de3a`; MCP —
`6d5d3940698fb59e02455bf3a9738319a24ff19da17becd1ad20c817eeb76446`.

Все три unit-а active с `NRestarts=0`; хеши обоих работающих bot executable совпали
с artifact, error-level journal после рестарта пуст. Текущая локальная spam-метка
усиливает baseline на 70 баллов, сохраняет полный аудит и меняет ревизию снимка.
Ранее размеченный участник с новым сообщением может получить капчу после первых
суток в чате; старый replay без свежей активности действий не выполняет.
Текущий статус проверяется через typed Telegram API: banned/left и администраторы
пропускаются. PVO-порог 85 и TTL 600 секунд сохранены.
Проверки: `scripts/test.sh`, strict Clippy, зелёный [CI 38129117611](https://github.com/Mar2ianen/NedoBot/actions/runs/38129117611),
PostgreSQL + Telegram stub для +70, chat scope, свежести исходной Telegram date,
повторной капчи и снятия метки. Для зависимости прошла полная матрица features
([teloxide-antispam PR #1](https://github.com/Mar2ianen/teloxide-antispam/pull/1)).

11 октября 2026 в **08:27 UTC / 11:27 МСК** выпущен release
[`deploy-2026-10-11-captcha-message-cleanup`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-11-captcha-message-cleanup)
на merge commit `a20fc82b0569eea6209e6bc60196c17de138ae44` после review
[PR #68](https://github.com/Mar2ianen/NedoBot/pull/68). [Release workflow 38124356577](https://github.com/Mar2ianen/NedoBot/actions/runs/38124356577)
установил artifact в оба community-инстанса и public MCP. SHA-256 обоих bot binaries —
`c2018639febab83c95f0d660c1b0e63d031050430c842874a4bd6943ead77a94`; MCP —
`2a04055fe0351ddc27c1d6f72037b2fd2b87eca654a98568fb4a6cfea4872c03`.

Все три unit-а active с `NRestarts=0`; хеши обоих bot executable совпали с artifact,
error-level journal после рестарта пуст. Капча очищает свежую пачку сообщений автора
в границах аудита; повторная обработка ожидающей капчи сохраняет карточку и TTL.
В PVO подтверждены `captcha_threshold=85`, `captcha_ttl_sec=600`, реальные действия
без dry-run и оба флага удаления сообщений ручных команд модерации.
Проверки перед выкладкой: `scripts/test.sh` с PostgreSQL и Telegram stub,
`cargo clippy --all-targets --locked -- -D warnings`, зелёный CI PR #68.

10 октября 2026 в **18:23 UTC / 21:23 МСК** выпущен release
[`deploy-2026-10-10-moderation-message-delete`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-10-moderation-message-delete)
на merge commit `7ea41fe8dab67dd1ffc43213f0f86e9611909425` после review
[PR #66](https://github.com/Mar2ianen/NedoBot/pull/66). [Release workflow 38074803057](https://github.com/Mar2ianen/NedoBot/actions/runs/38074803057)
успешно установил artifact в оба community-инстанса и MCP. SHA-256 обоих bot binaries —
`53aef7c8bccb6a6b8972389ef8faa2b84aa864676a4b1fab63b001ab5cf84a62`; SHA-256 MCP —
`2357ceb9d9b7460aced6b35516e4ad1221abe8591466e81fa4bea69c6451cfab`.

Все три unit-а active, `NRestarts=0`; хеши обоих работающих bot binaries совпали с artifact,
свежих error-level записей после рестарта нет. В PVO-профиле для `chats.review` включены
`moderation_delete_command_message=true` и `moderation_delete_target_message=true`;
в Nedonews эти опции не включены. Удаление выполняется только после успешного `/ban`, `/mute`
или `/warn`. Перед правкой PVO-профиля сохранена копия
`/etc/tg-ai-bot/pvo-llm_profiles.toml.bak-20261010T181144Z`; startup preflight подтвердил
право PVO-бота удалять сообщения.

10 октября 2026 в **11:55 UTC / 14:55 МСК** выпущен release
[`deploy-2026-10-10-pvo-risk-captcha`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-10-pvo-risk-captcha)
на merge commit `f33cfd4b9695b2205f5f06173f47f2c87a24d22d` после review [PR #63](https://github.com/Mar2ianen/NedoBot/pull/63). [Release workflow 38049697137](https://github.com/Mar2ianen/NedoBot/actions/runs/38049697137)
собрал artifact на Ubuntu 24.04, проверил glibc ≤ 2.39 и установил его в оба bot instance и MCP.
SHA-256 обоих bot binaries —
`248965b10992ad10dc9ebab5c2484eddce54a21cc19afc05364da847aff3b7f7`, MCP —
`0a6c0a8f008fc2878a59d094f4801a3af09447d1eedf002acd5b2a259dc5b186`.

Оба бота и `nedonews-mcp` active с `NRestarts=0`; running binary hashes совпали с artifact,
error-level journal после выкладки пуст. Обе production БД применили migrations
`20261010120000` и `20261010130000`; таблица `telegram_risk_captcha_challenges` есть в PVO.
При релизе в PVO-профиле включены `captcha_enabled=true`, `captcha_dry_run=false`, порог `70`, TTL `600` секунд;
предрелизная копия: `/etc/tg-ai-bot/pvo-llm_profiles.toml.bak-20261010T1148Z`. Startup preflight подтвердил права бота ПВО
на удаление сообщений и ограничения участников.
10 октября в **12:09 UTC / 15:09 МСК** runtime-порог капчи поднят с `70` до `85`; TTL остался 600 секунд.
Перед правкой сохранена копия `/etc/tg-ai-bot/pvo-llm_profiles.toml.bak-20261010T120842Z`, перезапущен только `nedobot-pvo`;
после рестарта все три unit-а active, `NRestarts=0`, свежих error-level записей нет.

9 октября 2026 в **20:21 UTC / 23:21 МСК** выпущен release
[`deploy-2026-10-09-userstatus-parallel`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-09-userstatus-parallel)
на merge commit `f6d1c00cfad8a081a58344a037a3974cf960cac4` после review
[PR #57](https://github.com/Mar2ianen/NedoBot/pull/57). Release workflow
[37985398485](https://github.com/Mar2ianen/NedoBot/actions/runs/37985398485)
успешно собрал artifact, проверил glibc ≤ 2.39 и установил его в оба bot
instance и MCP. SHA-256 обоих bot binaries —
`2ca45b0161e9b98b78e2832d71e2f73dc48312a5255c27d940e06d6cffe1cdec`, MCP —
`939fc9160824752b6000254432df717abf9f37831b1ed5f789bdc1ee0a378305`.

Оба бота и `nedonews-mcp` активны с `NRestarts=0`; хеши работающих процессов
совпали с release artifact, error-level journal после выкладки пуст. В
`/userstatus` независимое обновление member/profile и SQL-чтения выполняются
параллельно; до четырёх DB-запросов одновременно. На момент проверки в
20:24 UTC новых полных командных trace-ов после выкладки ещё не было, команду
smoke в чаты не отправляли.

8 октября 2026 в **19:20 UTC / 22:20 МСК** выпущен release
[`deploy-2026-10-08-telegram-send-fallback`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-08-telegram-send-fallback)
на merge commit `3b427de3e3caa3cc7a996b12ce8132b603c2d6c9` после review
[PR #47](https://github.com/Mar2ianen/NedoBot/pull/47). Release workflow
[37829976468](https://github.com/Mar2ianen/NedoBot/actions/runs/37829976468)
успешно собрал artifact, проверил glibc ≤ 2.39 и установил его в оба bot
instance и MCP. SHA-256 обоих bot binaries —
`48f05e673ebb67af0655804ce193644f2e172052468d8b6cb9649b480f3c65a2`, MCP —
`9bf767116cf9e3edf4b0e8d6229882ebf5861e57426d36967bd9ea8deac92bf2`.

В обеих production БД успешно применены все 94 миграции, failed migrations
нет; последняя — `20261008183924_preserve_spammer_avatar_dataset_refs`, колонка
`dataset_avatar_file_id` присутствует. Оба бота и `nedonews-mcp` активны с
`NRestarts=0`; hashes работающих bot processes и MCP совпали с artifact.
Error-level journal обоих ботов после выкладки пуст. Telegram API подтверждает
`can_read_all_group_messages=true` для Недострая и ПВО: обычные сообщения групп
читаются. Недострай не администратор группы (`restricted`), поэтому Telegram не
передаёт ему реакции и `chat_member`; его доступ к source channel при этом
подтверждён. ПВО — администратор. Команду-smoke в чаты не отправляли.

В релиз также вошёл `provider-access` как самостоятельный workspace crate;
приложение пока не использует его как runtime route.

8 октября 2026 в **16:26 UTC / 19:26 МСК** выпущен release
[`deploy-2026-10-08-ask-python-files`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-08-ask-python-files)
на merge commit `e1a6ab47b88f3eccfa2100c8857a8cbd544ce0f3` после review
[PR #45](https://github.com/Mar2ianen/NedoBot/pull/45). Сборка и deploy обоих
ботов и public MCP прошли в [release workflow 37807666497](https://github.com/Mar2ianen/NedoBot/actions/runs/37807666497).
Для обоих ботов SHA-256 исполняемого файла —
`5eef9f12104a6c0be1653effe42a039ece06fb3ef21d80c4c55eb9ef746a44cd`, для MCP —
`a9e4a1c16769431c5569b885dc201a13f45bc8426e981bd9db1d17beb9e4261c`.

В production включён `sandbox.python` только в профиле НедоNews; профиль ПВО
оставлен выключенным. Образ Python закреплён digest
`sha256:2a890751d3ac217ba36aab6235e15fa29d0f0e28f72ab040afac0d8180a0fdbd`.
Команда запускает одноразовый rootless Podman-контейнер от `nedobot-sandbox`:
без сети, host mounts, секретов и capabilities, с read-only root, лимитами CPU,
памяти, процессов и времени. Модель может передать ему один UTF-8 текстовый
документ из reply размером до 2 MiB; работают только стандартная библиотека
Python и временный `/workspace`. Файлы результата и состояние между вызовами
не сохраняются, постоянного Jupyter kernel нет. Startup preflight выполнил
`pass` в контейнере. До рестарта сделан закрытый backup основного профиля в
`/opt/tg-ai-bot-releases/deploy-2026-10-08-ask-python-files-before-20261008T162503Z/`.

После включения профиля оба бота и MCP перезапущены явным systemd system
manager; все три unit-а активны, `NRestarts=0`, application journal без ошибок.
Telegram smoke-команду не отправляли.

### Предыдущие выпуски 8 октября

PR [#44](https://github.com/Mar2ianen/NedoBot/pull/44), tag
[`deploy-2026-10-08-userstatus-rich-photo`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-08-userstatus-rich-photo),
устранил отказ `/userstatus`: renderer отдавал ссылку на кэшированный avatar
без передачи media, из-за чего Telegram отклонял всю rich-карточку. Workflow
[37800916143](https://github.com/Mar2ianen/NedoBot/actions/runs/37800916143)
успешно установил artifact `4266e6c8f8a3c6ded1f95800e3f6b98156e2fdcfda303b2bd3dc170c2e7a2997`.
Неполученные во время сбоя старые ответы Telegram не переигрывает; команду нужно
повторить.

PR [#43](https://github.com/Mar2ianen/NedoBot/pull/43), tag
[`deploy-2026-10-08-ask-context-grounding`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-08-ask-context-grounding),
обновил изоляцию контекста `/ask`, границы недоверенных данных и проверку
grounding цитат. Workflow
[37799237640](https://github.com/Mar2ianen/NedoBot/actions/runs/37799237640)
успешно завершился. Public MCP остаётся намеренно публичным read-only allowlist;
добавлять к нему Bearer-auth не требуется.

### Предыдущая выкладка: EmbeddingGemma 2, 7 октября

7 октября 2026 в **19:53 UTC / 22:53 МСК** выпущен release
[`deploy-2026-10-07-embeddinggemma2`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-07-embeddinggemma2)
на source `7883f750e86f333fba9f6da2e994dfc6bce73f48` после review
[PR #40](https://github.com/Mar2ianen/NedoBot/pull/40). Release workflow
[37676399623](https://github.com/Mar2ianen/NedoBot/actions/runs/37676399623)
собрал artifact и успешно установил его в оба checkout. В первой попытке
deploy шаг применил MCP `GRANT` раньше, чем startup-миграции обоих процессов
завершились; повторный запуск failed job прошёл после создания схемы. В этом
изменении workflow ждёт таблицы в обеих БД перед выдачей прав.

Оба бота и MCP запущены с artifact hashes:
bot `9401a65d3467da5cd513689fa74c3fa814091c873d9c19c3984abe223ed8aacf`,
MCP `00bfebb787904d1a3fcf68d6f10e8e0db82b556bfd7335e5c370a76ff721c317`.
В каждой БД 93 успешные миграции, failed migrations нет; все три unit-а имеют
`NRestarts=0`. Local MCP ответил `403`, public MCP — `405` на unauthenticated
GET; это ожидаемый ответ для этих probes. Оба production profiles используют
pinned `onnx-community/embeddinggemma-2-ONNX` Q4 revision
`daa72c51243991dfcaf9f9137d2c573d8f7790c0`, 512d text/image vectors и timeout
60 секунд. Main использует новый 512d spam-head; общая модерация ПВО осталась
выключенной. Новый `/embed` и `/embed-image` прошли smoke на тексте и
синтетическом PNG: по 512 конечных компонентов, L2 norm `1.0`.

`nedobot-rag-embedding` обслуживает новый encoder на `127.0.0.1:8788`.
Старый 300M chat unit остановлен и отключён, его unit и volume сохранены;
предыдущий RuBERT unit сохранён в snapshot. Для каждого инстанса запущены
chat/history backfill и avatar worker. Avatar dataset принимает только
векторы после явной spam-метки, удаляет строку после снятия метки и не хранит
image bytes. В момент старта workers нашли 177 ранее помеченных spammer-аватаров
с доступными Telegram file IDs; обработка и повтор ограниченных download errors
продолжаются.

До миграции сохранены оба PostgreSQL dump-а, consistent SQLite snapshot,
profiles, MCP env, units и работающие binaries в приватном
`/opt/tg-ai-bot-releases/deploy-2026-10-07-embeddinggemma2-before-20261007T192500Z/`.
Для обоих dump-ов проверены `pg_restore --list` и SHA-256; SQLite прошла
`PRAGMA integrity_check`.

### Предыдущая выкладка: changelog 6 октября

6 октября 2026 в **16:09 UTC / 19:09 МСК** выпущен release
[`deploy-2026-10-06-bot-changelog-details`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-06-bot-changelog-details)
на source `6949a151fc8e9ffd67419a8bb477ebd2b2077fc2`, после review
[PR #38](https://github.com/Mar2ianen/NedoBot/pull/38). Бинарники собраны
[release workflow](https://github.com/Mar2ianen/NedoBot/actions/runs/37492109948)
с `deploy=false`; `BUILD_INFO.txt` и все записи `SHA256SUMS` проверены перед
установкой. Обновлены и перезапущены оба bot-инстанса и `nedonews-mcp`.

В changelog добавлены антиспам-факты из коммитов 5 октября, вошедших в релиз
6 октября: сохранённые spam/not-spam метки учитываются при повторе имён,
заголовок личного канала служит сигналом, а доставка review-карточек (если
включена) ограничена первыми пятью минутами. Сейчас review delivery выключена
в обоих production profiles. Runtime profiles, env и схема БД не менялись.

Оба бота и MCP активны; `/proc/<pid>/exe` hashes совпали с артефактом:
bot `d3d910bc5a356fae9771fba0eabd28d20003d901f901bd6fac0ff868d0d636fe`,
MCP `26968db01032cf3f46508e2c3ece07906f0a0cbc2f232d02bf7199762a6140df`.
В каждой production БД 92 успешные миграции, failed migrations и ошибки
приложения после рестарта отсутствуют. Local MCP ответил `403`, public MCP —
`405` на unauthenticated GET; Telegram smoke-команды не отправлялись.

До выкладки сохранены и проверены dumps обеих БД (`pg_restore --list`),
consistent SQLite backup (`PRAGMA integrity_check`), работающие binaries,
profiles и env в приватном `/opt/tg-ai-bot-releases/deploy-2026-10-06-bot-changelog-details-before-20261006T155329Z/`.
Проверенный artifact и `DEPLOYMENT_RESULT.json` находятся в
`/opt/tg-ai-bot-releases/deploy-2026-10-06-bot-changelog-details-6949a15/`.

## Перед выкладкой

### Предыдущая выкладка changelog 6 октября, 18:36 МСК

Первая версия `/ask` changelog была выпущена tag
[`deploy-2026-10-06-bot-changelog`](https://github.com/Mar2ianen/NedoBot/tree/deploy-2026-10-06-bot-changelog)
на source `6c11dc761872ae6ee7b650fb59fac2e2f7072f46` после review
[PR #36](https://github.com/Mar2ianen/NedoBot/pull/36). Её заменил текущий
release после повторной сверки заметных изменений из коммитов 5 октября.
Предыдущие stage и backup сохранены рядом с текущими в `/opt/tg-ai-bot-releases/`.

### Расширение лимитов /ask от 6 октября, 10:18 МСК

Фактическая выкладка: tag `deploy-2026-10-06-ask-limits`, source
`19f9ec21d091d59d935b38d727373911b5ea28ca`, review [PR #34](https://github.com/Mar2ianen/NedoBot/pull/34).
Точные artifact hashes и backup paths записаны в `TECHNICAL.md`.

В profile НедоNews после бэкапа вручную расширены шесть runtime-параметров:
16 384 токена, 64 tools / до 68 LLM turns, LLM-попытка 180 секунд,
общий deadline 1800 секунд, concurrency 4, MCP request 30 секунд.
Основной `groq_qwen_ask` использует `level_high` и timeout 180 секунд.
Добавлены отдельные `openrouter_qwen_ask` и `gemini_ask` с теми же effort/timeout;
`routes.ask.models` ссылается на них. Profile целиком из example не копировать.

`probe_ask_route [--image]` теперь валидирует startup secrets и проверяет
фактический `ask_llm_max_tokens`, печатая также tool budget и общий deadline.
До рестарта артефактом проверены staged Groq text/image, OpenRouter text
и Gemini image; после рестарта повторены Groq text/image на live profile.
Замена profile выполнена атомарно с проверкой исходного hash и сохранением
owner/mode. Новых миграций нет. Второй community profile сохранён без
изменений, `/ask` в ПВО выключен. Перезапуск и health check охватили
оба бота и MCP. Public RMCP проверен без отправки сообщений в Telegram.

### Релиз исправлений /ask от 6 октября, 09:30 МСК

Ниже сохранён порядок предыдущей выкладки исправлений и backfill.

Аудит и границы проверки: [ASK_AUDIT_2026-10-06.md](ASK_AUDIT_2026-10-06.md).
Сначала review `dev → main`, затем сборка release с `deploy=false` на точном
SHA и проверка `BUILD_INFO.txt`/`SHA256SUMS`. Деплой обоих ботов и MCP остаётся
отдельным действием.

В production profile нужна одна ручная правка после бэкапа: добавить
существующий `gemini_flash_comment` в конец `routes.ask.models`. Это
независимый vision fallback: текстовые OpenRouter/Ollama profiles при
приложенной фотографии исключаются capability-фильтром. Секрет
`GEMINI_API_KEY` уже используется текущими Gemini routes; profile целиком
из example не копировать. Второй community profile проверяется отдельно.

До рестарта можно проверить staged release CLI из каталога инстанса:

```bash
./probe_ask_route
./probe_ask_route --image
```

CLI загружает выбранный через `LLM_PROFILES_PATH` profile и secrets env;
вызывается только диагностический LLM tool, без Telegram, БД и заметок.
Для проверки ещё не изменённой production topology использовать отдельную
копию profile с добавленным fallback. Возвращаются identity выбранного
profile и `native_tool_verified`, содержимое ответа не печатается.

Проверка SDK артефактом с VPS подтвердила native tool calls Groq для текста
и изображения. Scoped dry run НедоNews обнаружил 52 восстанавливаемых
rich-сообщения. В ПВО найдены два rich-сообщения из Desktop export: их формат
тоже поддерживают backfill и обновлённый `import_telegram_export`.

При старте встроенная миграция `20261006000000` переочередит materialization
успешных v7 assessments в v8. LLM-аудит не повторяется. Контролировать
очередь и CAS/retry метрики обоих инстансов.

После миграций восстановить старые rich-тексты отдельно в каждой БД.
Выполнять из каталога инстанса с его env; CLI не требует Telegram token:

```bash
./target/release/backfill_rich_messages -1001932061163
./target/release/backfill_rich_messages -1001932061163 --apply
```

Для второго инстанса подставить его chat ID и DSN. Сначала проверить dry
run, затем `--apply`; повторный запуск идемпотентен. Payload и старый audit
не меняются, user activity не увеличивается, новые Telegram сообщения
и LLM jobs не создаются. Бэкап БД перед выкладкой обязателен.

Smoke после выкладки: reply на старый rich-ответ, вопрос о родительском
посте, точный word count с повтором слова в одной строке, групповой scope
при включённом private default chat, оба service PID/hash и journal.
Deployment tag ставить только после фактического успешного deploy.

1. Проверить, что PR слит в main, worktree чистый, а remote head известен:

   ```bash
   git fetch origin main
   git status --short --branch
   git rev-parse origin/main
   ```

2. Прогнать локальные проверки:

   ```bash
   cargo fmt --all -- --check
   cargo test --workspace --all-targets --locked
   cargo clippy --workspace --all-targets --locked -- -D warnings
   ./scripts/test.sh
   ```

3. Проверить production profile. В репозитории хранится только
   config/llm_profiles.toml.production.example: он не содержит секретов, но
   содержит реальные enabled flags, routes и команды MCP. На сервере он
   устанавливается в /etc/tg-ai-bot/llm_profiles.toml. Секреты остаются в
   /opt/tg-ai-bot-teloxide/.env и в systemd drop-ins.

   ```bash
   ssh vps-153 'test -f /etc/tg-ai-bot/llm_profiles.toml'
   ssh vps-153 'systemctl show tg-ai-bot-teloxide -p EnvironmentFiles'
   ssh vps-153 'systemctl cat tg-ai-bot-teloxide | sed -n "/llm-profiles.conf/,+3p"'
   ```

   Значение LLM_PROFILES_PATH должно быть абсолютным:
   /etc/tg-ai-bot/llm_profiles.toml.

## Инстансы

На vps-153 два community-инстанса одного бинаря, у каждого своя БД, env и
профиль. Общий только SQLite спам-репутации (один путь, разные `instance.id`).

| Инстанс | Unit | Checkout / binary | Profile | Database |
|---|---|---|---|---|
| НедоNews (`nedonews`) | `tg-ai-bot-teloxide.service` | `/opt/tg-ai-bot-teloxide/target/release/` | `/etc/tg-ai-bot/llm_profiles.toml` | `tg_ai_bot` |
| ПВО (`pvo`) | `nedobot-pvo.service` | `/opt/nedobot-pvo/target/release/` | `/etc/tg-ai-bot/pvo-llm_profiles.toml` | `tg_ai_bot_pvo` |

Оба бинаря ставятся из одного артефакта (`scripts/install_release_binaries.sh`
пишет в оба checkout). Рестарт и проверка всегда охватывают оба юнита:
`systemctl restart tg-ai-bot-teloxide nedobot-pvo`. Профили правятся вручную с
бэкапом и автоматикой не перезаписываются; шаблона pvo-профиля в репозитории
нет. Миграции встроены в бинарь (`sqlx::migrate!`), поэтому рассинхрон чекаута
и БД (как остановка `nedobot-pvo` 2026-10-01 из-за отсутствующей в его сборке
миграции) лечится выкладкой свежего бинаря, а не правкой `_sqlx_migrations`.

## Выкладка

Основной путь — GitHub Actions (`release`, раннер `ubuntu-24.04`, glibc 2.39
совпадает с production): сборка `--locked --release` с default features
(единый бинарь на все инстансы), проверка glibc-совместимости, публикация
артефакта и опциональный deploy на vps-153 с атомарной заменой бинарей,
restart и сверкой хеша `/proc/<pid>/exe`. Сборка на сервере больше не
используется: на 4 ГБ RAM `rustc` убивает OOM-killer. Для deploy из CI нужны
секреты репозитория: `VPS_HOST`, `VPS_USER`, `VPS_SSH_KEY`, `VPS_KNOWN_HOSTS`.
Конфиг `/etc/tg-ai-bot/llm_profiles.toml` автоматика не трогает: он правится
вручную с бэкапом, шаблон — `config/llm_profiles.toml.production.example`.

Запасной путь при недоступности CI — сборка на сервере (нужен свободный swap,
иначе OOM), затем restart и те же проверки. Сначала сделать dry-run. Не
использовать `--delete`: production checkout может
содержать SQLx migration-файлы, уже применённые к БД, но отсутствующие в текущем
source snapshot. Удаление такого файла приведёт к `VersionMissing` при следующем
старте. Устаревшие исходники удалять только отдельной проверенной процедурой
после сверки `_sqlx_migrations` обеих production-БД. Секреты, persistent
static-файлы, backups, дампы и локальный build cache исключаются явно:

В production есть один известный legacy gap: применённая миграция
`20260927120000` недоступна с исходным checksum. `db::migrate` разрешает
пропустить только эту отсутствующую версию после сверки applied versions;
любая другая отсутствующая версия остаётся startup error. Не добавляй файл с
тем же номером и другим SQL и не меняй checksum в `_sqlx_migrations`.

```bash
rsync -azn \
  --exclude target \
  --exclude .git \
  --exclude '.env*' \
  --exclude static/ \
  --exclude backups/ \
  --exclude models/ \
  --exclude '*.dump' \
  --exclude docs/LOCAL_WORKFLOW.md \
  ./ vps-153:/opt/tg-ai-bot-teloxide/
```

После проверки списка изменений повторить команду без -n, сохранив
machine-specific `docs/LOCAL_WORKFLOW.md`:

```bash
rsync -az \
  --exclude target \
  --exclude .git \
  --exclude '.env*' \
  --exclude static/ \
  --exclude backups/ \
  --exclude models/ \
  --exclude '*.dump' \
  --exclude docs/LOCAL_WORKFLOW.md \
  ./ vps-153:/opt/tg-ai-bot-teloxide/
```

`models/` исключён намеренно: весовые JSON лежат только на сервере (плюс
снапшоты в `/opt/tg-ai-bot-releases/`), а `--delete` без этого exclude уже
удалял `alt_word_char_v2_2026-10-03.json` 2026-10-05 с crash-loop НедоNews до
восстановления из снапшота (хеш сверен с задокументированным в релизе).

При использовании запасного пути повторить source sync для
`/opt/nedobot-pvo/`, сохранив его собственные `.env`, profile и persistent
файлы. Затем отдельно проверить доступ сервисного пользователя к обоим
checkout:

```bash
ssh vps-153 'chmod 755 /opt/tg-ai-bot-teloxide && runuser -u tg-ai-bot -- test -x /opt/tg-ai-bot-teloxide'
ssh vps-153 'chmod 755 /opt/nedobot-pvo && runuser -u tg-ai-bot -- test -x /opt/nedobot-pvo'
```

Не выполнять рекурсивный `chmod`: MCP нужен только проход по каталогу и
доступ к release binary. Изменения non-secret profiles внести вручную после
бэкапа, сохранив настройки каждого инстанса. Example целиком поверх
существующего profile не копировать. До рестарта проверить оба файла:

```bash
ssh vps-153 'test -s /etc/tg-ai-bot/llm_profiles.toml && test -s /etc/tg-ai-bot/pvo-llm_profiles.toml'
```

В запасном пути собрать один release на сервере с production toolchain и
локальным cargo cache. Подготовить stage из бинарей и installer, как в
workflow `release`, затем установить его через `install_release_binaries.sh`
сразу в оба checkout. Простого rebuild первого инстанса недостаточно:

```bash
ssh vps-153 'cd /opt/tg-ai-bot-teloxide && /root/.cargo/bin/cargo build --locked --release'
# После установки проверенного stage через installer:
ssh vps-153 'systemctl restart tg-ai-bot-teloxide nedobot-pvo'
ssh vps-153 'systemctl is-active tg-ai-bot-teloxide nedobot-pvo'
ssh vps-153 'systemctl restart nedonews-mcp'
ssh vps-153 'systemctl is-active nedonews-mcp'
```

### Опциональная Python-песочница `/ask`

Перед включением создай отдельного `nedobot-sandbox` с домашним каталогом
`/var/lib/nedobot-sandbox`, выделенным диапазоном subordinate UID/GID и включённым
lingering через `loginctl enable-linger nedobot-sandbox`. Бот запускается от
root, но вызывает Podman через `runuser` с очищенным окружением от имени этого
пользователя. Проверь его `podman info --format
'{{.Host.Security.Rootless}}'` и cgroup v2 лимиты. Подготовь
`deploy/ask-sandbox/Containerfile` от проверенного OCI digest; после сборки
получи immutable image ID через `podman image inspect --format '{{.Id}}'
<tag>` и укажи его в `ask_python_sandbox_image`. Включай
`ask_python_sandbox_enabled=true` только после успешного smoke запуска от
`nedobot-sandbox`. Образ во время запроса не скачивается (`--pull=never`).

Песочница принимает один UTF-8 текстовый файл до 2 MiB из reply для анализа
табличных данных, обычного текста и исходного кода; документ копируется через
stdin во временный `/workspace`. Каждый tool call получает новый контейнер, поэтому
состояние Python не хранится между вызовами и это не постоянное Jupyter-ядро.
Файлы-результаты отдельно не доставляются. Вложение, сеть и файлы хоста не
монтируются; сеть контейнера отключена, rootfs read-only, RAM ограничена 256 MiB
без swap, CPU и PID имеют лимиты, временный диск ограничен. На втором
community-инстансе оставь функцию выключенной, пока отдельно не проверишь
rootless Podman и лимиты ресурсов.

Для pinned EmbeddingGemma 2 text+image encoder создать volume и установить unit
из checkout; unit собирает pinned Transformers.js Q4 image из
`deploy/rag-embedding` при старте и скачивает model snapshot в постоянный volume:

```bash
ssh vps-153 'podman volume create nedobot_rag_embedding'
ssh vps-153 'install -m 0644 /opt/tg-ai-bot-teloxide/deploy/rag-embedding/nedobot-rag-embedding.service /etc/systemd/system/nedobot-rag-embedding.service'
ssh vps-153 'systemctl daemon-reload && systemctl enable --now nedobot-rag-embedding && curl -fsS http://127.0.0.1:8788/healthz'
```

Миграции запускаются startup-кодом бота. После проверки `/healthz` выставить в
обоих profile-файлах один pinned model id и URL `http://127.0.0.1:8788` для
RAG и chat retrieval; конфиги менять отдельно с бэкапом. Старые RuBERT 312d и
EmbeddingGemma 300M 768d колонки остаются нетронутыми для rollback. Новые
колонки заполняются бинарями `backfill_post_history_embeddings`,
`backfill_audit_embeddings` и `backfill_chat_embeddings` для каждой базы.
Старый cosine порог не переносится. Новая Gemma 2 spam-голова — отдельный
512d артефакт; устанавливать только этот файл рядом с сохранёнными моделями,
не синхронизировать каталог `models/` целиком.

`avatar_embeddings_enabled=true` включает отдельный bounded worker. Он
добавляет аватар только после явного spam-label; штатный аудит остальных
пользователей не сохраняет их image vectors. Снятие метки удаляет записи этого
пользователя. До векторизации Postgres держит file id только для повторной
загрузки; после готового embedding он очищается, остаётся только
non-downloadable file-unique id. Байты изображения не сохраняются.

После переключения проверить `nedobot-rag-embedding` и остановить старые
`nedobot-chat-embedding`/RuBERT-serving units. При rollback восстановить оба
profile-файла и старые unit-ы; legacy-векторы остаются отдельными и не
перезаписываются.

Для синхронизации репутации спамеров все локальные инстансы должны работать на
одном сервере и использовать одинаковый абсолютный путь к SQLite-файлу, например
`/var/lib/nedobot/spam-reputation.sqlite`. Это локальная SQLite-база в WAL-режиме,
не сетевой диск. Создать каталог с правами только для сервисного пользователя
перед запуском обоих инстансов; в каждом профиле установить
`[spam_reputation] enabled = true` и тот же `sqlite_path`. У каждого инстанса
должен быть собственный стабильный `instance.id`. Текущие локальные подтверждённые
метки загружаются в общий журнал при старте, затем статусы синхронизируются раз в
30 секунд; снятие локальной метки создаёт отзыв решения. Общие метки используются
как отдельный сильный сигнал для ревью и для поиска повторно используемых имён,
но сами по себе не вызывают бан.

## Проверка после restart

```bash
ssh vps-153 'systemctl is-active tg-ai-bot-teloxide nedobot-pvo nedonews-mcp container-tg-ai-bot-postgres nedobot-rag-embedding'
ssh vps-153 'journalctl -u tg-ai-bot-teloxide -u nedobot-pvo -n 120 --no-pager'
ssh vps-153 'journalctl -u nedonews-mcp -n 80 --no-pager'
ssh vps-153 'podman ps'
ssh vps-153 'curl -sS -o /dev/null -w "local=%{http_code} %{time_total}\n" http://127.0.0.1:8787/mcp/nedonews/v2'
curl -sS -o /dev/null -w 'public=%{http_code} %{time_total}\n' https://nedobot.chickenkiller.com/mcp/nedonews/v2
```

Для unauthenticated probe `403` на локальном endpoint и `405` на публичном
GET могут быть нормальным результатом: health-check подтверждает, что route
доступен, а не что MCP-клиент уже выполнил POST discovery. Реальный smoke
должен использовать MCP client с текущим RMCP v2 контрактом и допустимым Host/Origin; application auth намеренно отсутствует у публичного read-only endpoint-а.

Для Telegram runtime smoke используется отдельный тестовый чат и команда
/ping, затем /ask с коротким вопросом. Для /ask проверить:

- private chat получает native draft и один final answer;
- group получает один progress message, который редактируется до final;
- при rich delivery failure fallback не создаёт второе сообщение при
  Unknown;
- новые записи ask_runs содержат captured_now, timezone, renderer revision,
  compiled Markdown и delivery outcome.

## Фиксация release и rollback

Только после успешного restart и smoke создать и запушить annotated tag:

```bash
git tag -a deploy-YYYY-MM-DD-scope <merged-main-sha> \
  -m "Deploy NedoBot <merged-main-sha>"
git push origin deploy-YYYY-MM-DD-scope
```

Rollback — это повторная выкладка предыдущего release tag тем же способом:
checkout/archive exact tag, dry-run rsync, profile check, build, restart и
тот же post-deploy smoke. Не использовать git reset --hard на рабочем сервере
и не удалять .env, /etc/tg-ai-bot/llm_profiles.toml, static/, backups или
database dumps.
