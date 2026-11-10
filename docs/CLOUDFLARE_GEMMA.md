# Cloudflare Gemma для unified audit

Модель Workers AI: @cf/google/gemma-4-26b-a4b-it. Подключается профилем
существующего genai OpenAI adapter, без отдельного HTTP-клиента или
provider-specific ветки router. Локальный antispam cascade остаётся перед LLM.

## Проверенный wire contract

- Base URL заканчивается на /accounts/ACCOUNT_ID/ai/v1. Используется
  /chat/completions; native /ai/run имеет другой envelope и сюда не подходит.
- Авторизация — Bearer из CLOUDFLARE_API_TOKEN. ACCOUNT_ID берётся оператором
  из существующего env при подготовке private profile; runtime не подставляет
  переменные в URL. Реальный account ID в репозиторий не добавлять.
- Изображение передаётся OpenAI image_url с JPEG data URI. Probe проверяет
  две картинки с переставленными цветами, чтобы подтвердить чтение пикселей.
- JSON Schema передаётся стандартным response_format.json_schema.schema.
  Полный audit response дополнительно проходит прежний typed validator.
- Для короткого audit обязательно выключить thinking через model option
  chat_template_kwargs = { enable_thinking = false }. Одного thinking="none"
  недостаточно: этот режим лишь не отправляет reasoning_effort.
  В проверке с default template 128 output tokens ушли на reasoning, content
  был пустым. С explicit template option тот же JSON завершился за 11 tokens.

Новое поле model.chat_template_kwargs типизировано, принимает только
enable_thinking. Оно не позволяет подменять messages, auth, tools или
бюджеты произвольным extra_body. Без поля запросы не меняются.
Startup validation отклоняет native/несовместимый adapter и одновременное
использование thinking effort и chat-template control.

Пример provider/model есть в config/llm_profiles.toml.example и не включён
в default routes автоматически. Нужный private profile выбирает модель
в routes.new_user_audit. Для production замена ограничена этим маршрутом:
Cloudflare становится primary для текста и изображений, прежний Ollama
удаляется из audit route из-за HTTP 402; другие task routes не меняются.
Существующие текстовые fallback-модели можно сохранить после Cloudflare:
route resolver пропускает их при наличии изображения, не отбрасывая картинку.

## Live probe без пользовательских данных

~~~sh
# Те же env и LLM_PROFILES_PATH, что у выбранного instance.
probe_new_user_audit --provider cloudflare
~~~

Probe сужает только свою копию route до указанного provider, чтобы другой
fallback не скрывал отказ Cloudflare. Telegram и БД не используются.
Внешние вызовы ограничены двумя синтетическими image-color запросами и
четырьмя запросами полного audit schema — все сочетания наличия картинки
и первого сообщения. Обычная
validation retry может повторить каждый невалидный ответ один раз.
Prompt/ответы модели и credentials не печатаются; выводятся только
provider/model, modality и прошедшая проверка.

Для staging запускать probe и validate_instance_config из нового CI artifact
с candidate TOML до изменения рабочих профилей. Старый binary может не
понимать новое model поле. При rollback восстанавливать предыдущие binary
и оба профиля совместно, сохраняя прочие изменения оператора.

## Цена и общая квота

Оба production instance используют один Cloudflare account. Бесплатная
аллокация Workers AI — 10 000 neurons в сутки на аккаунт, а не на бота.
На Workers Paid превышение оплачивается. Подключение не меняет billing
plan, не добавляет credits и не создаёт AI Gateway.

Профиль audit ограничивает output 900 tokens из runtime, timeout 60 секунд;
thinking выключен. Это ограничение одного запроса, не дневной денежный cap.
Нужно наблюдать фактическое потребление общей квоты, особенно если тот же
аккаунт используется другими Workers. Error/invalid JSON следует обычной
fenced job policy, scope/ban evidence и review-only ограничения сохраняются.

Официальные источники:
[Gemma 4: vision, context и параметры](https://developers.cloudflare.com/workers-ai/models/gemma-4-26b-a4b-it/),
[OpenAI-compatible endpoint](https://developers.cloudflare.com/workers-ai/configuration/open-ai-compatibility/),
[pricing и free allocation](https://developers.cloudflare.com/workers-ai/platform/pricing/).
