# provider-access

Общие средства подключения к провайдерам для локальных и self-hosted Rust-агентов. Крейт
изолирован от NedoBot, чтобы позже его можно было перенести в отдельный репозиторий.

## Возможности

- OpenAI Sign in with ChatGPT (SIWC): loopback callback, OAuth state и nonce, PKCE, динамическая
  регистрация клиента, проверка подписи/issuer/audience/nonce ID token, проверка выданных scope,
  обновление токенов с ротацией refresh token, межпоточная и межпроцессная блокировка обновления,
  отзыв сессии и каталог моделей для выбранного аккаунта.
- Файловое хранилище профилей: отдельный файл на регистрацию, атомарная замена, права 0600 на
  токен-файлы и 0700 на каталоги Unix. Приложение может реализовать CredentialStore поверх
  системного keychain или другого зашифрованного хранилища.
- Выбор модели по возможностям, размеру контекста, цене, провайдеру и списку приоритетных моделей.
- Фильтр бесплатных моделей OpenRouter и его динамический маршрут openrouter/free.
- Необязательный genai-адаптер для OpenRouter Chat Completions.

## Подключение ChatGPT

Приложение поднимает LoopbackListener и создаёт OpenAiSiwc с CredentialStore. Затем вызывает
start_sign_in с адресом перенаправления listener-а, открывает полученный URL авторизации в
системном браузере, ждёт callback и передаёт попытку с результатом в complete_sign_in. Открытие
браузера остаётся в приложении, чтобы библиотека не зависела от конкретной ОС.

Для повторной авторизации в start_sign_in передаётся сохранённый профиль. Используются его выданный
client ID, постоянный host ID и сохранённая подсказка ID token. Нельзя сохранять
dynamic_agent_client вместо client ID, выданного при регистрации.

Короткий сценарий подключения выглядит так:

```rust,ignore
let store = Arc::new(FileCredentialStore::new(config_dir));
let siwc = OpenAiSiwc::new(OpenAiSiwcConfig::new("My Local Agent"), store.clone());
let callback = LoopbackListener::bind().await?;
let start = siwc.start_sign_in(callback.redirect_uri(), None).await?;
open_system_browser(start.authorization_url.as_str())?;
let result = callback.wait(Duration::from_secs(180)).await?;
let profile = siwc.complete_sign_in(start, result).await?;
```

access_token(profile_id) при необходимости обновляет токены выбранного аккаунта. Метод
list_models(profile_id) загружает актуальный модельный каталог этого аккаунта. Неизвестные
возможности модели считаются неподдерживаемыми: одного факта видимости в списке недостаточно,
чтобы считать доступными инструменты или изображения.

stream_responses(profile_id, ResponsesRequest) отправляет публичный Responses запрос с
`store=false` и `stream=true`; тип запроса не позволяет задавать `previous_response_id`,
`temperature` или `max_output_tokens`, а inline сообщение system отклоняется. Поток разбирает SSE,
возвращает машинный quota-код события `response.failed` и завершает успешно только после
`response.completed`. Для SIWC здесь используется прямой transport: текущий Responses adapter
`genai` теряет машинный код некоторых stream-ошибок.

Ответ читается без дополнительного stream-trait импорта:

```rust,ignore
let mut stream = siwc
    .stream_responses(
        &profile.id,
        ResponsesRequest::new("gpt-5-codex", serde_json::json!([
            {"role": "user", "content": "Суммируй изменения"}
        ]))
        .with_instructions("Будь краток"),
    )
    .await?;
while let Some(event) = stream.next_event().await {
    let event = event?;
    if event.is_completed() {
        // В event.data находится terminal response вместе с usage.
    }
}
```

Для выбора конкретной модели `OpenAiModel::descriptor()` и `OpenRouterModel::descriptor()`
передаются в общий `ModelSelector`. Например, `ModelQuery` может одновременно потребовать
инструменты, изображения и минимальный размер контекста, а также задать предпочитаемый provider
или список model ID.

classify_responses_failure различает исчерпанный лимит, временную недоступность, неподходящий
аккаунт, неподдерживаемую возможность и прочие 429. Ошибка
`subscription_sharing_usage_limit_exceeded` может означать как общий лимит плана, так и
установленный пользователем лимит именно для этого приложения; API не сообщает, какой из них
сработал. Поэтому статус `UsageLimitReached` не приписывает причину и ведёт в
[настройки использования ChatGPT](https://chatgpt.com/settings/usage), где можно посмотреть
использование приложения и управлять его лимитом. Публичный SIWC API не документирует endpoint
для чтения числового остатка или точной причины срабатывания лимита. Пятчасовой лимит Plus при этом
остаётся общим для приложений: app-specific limit — дополнительное ограничение на одно приложение,
а не отдельная квота сверх общего лимита.

## Бесплатные модели OpenRouter

OpenRouterFreeProvider принимает API key. Вызов select_free_model с ModelQuery, например с
requires_tools=true и minimum_context=32000, выбирает конкретную модель из каталога. В выборку
попадают только модели с нулевой ценой и входных, и выходных токенов.

Динамический маршрут openrouter/free выбирает модель на стороне OpenRouter для каждого запроса и
может учитывать поддержку изображений и вызова инструментов. Адаптер
genai_adapter::chat_openrouter_free использует именно этот маршрут и сам не переключается на
платную модель.

## Ограничения интеграции

OpenAI документирует использование SIWC-плана для open-source инструментов, личных локальных
проектов и некоторых частных приложений. Для платного или удалённо размещённого приложения OpenAI
просит получить доступ до того, как предлагать пользователям расходовать их план.
Этот крейт не подключает SIWC к публичному Telegram-боту; прежде чем делать такое подключение,
нужно подтвердить право такого deployment-а на использование функции. Self-hosted runtime должен
использовать собственный постоянный host ID и защищённое хранилище. Файловый backend — удобный
вариант, но не замена системному хранилищу ключей, если оно доступно.

OpenCode использован как референс для выбора профиля и обновления токенов. Его внутренний маршрут
chatgpt.com/backend-api здесь не используется. Реализация следует публичным OAuth endpoint-ам
auth.openai.com, /v1/models и POST /v1/responses.

## Источники

- [OpenAI SIWC: регистрация и вход](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
- [OpenAI SIWC: профили, обновление токенов и использование](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)
- [OpenAI SIWC: UX лимитов и ссылка на управление использованием](https://developers.openai.com/siwc/ui-ux-guidelines)
- [OpenAI SIWC: модели и запросы](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)
- [OpenAI SIWC: ошибки и восстановление](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery)
- [OpenRouter: каталог моделей](https://openrouter.ai/docs/api/api-reference/models/get-models)
- [OpenRouter: бесплатный роутер](https://openrouter.ai/openrouter/free/apps)
- [OpenCode: реализация Codex провайдера](https://github.com/anomalyco/opencode/blob/dev/packages/opencode/src/plugin/openai/codex.ts)
