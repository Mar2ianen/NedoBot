<div align="center">

# НедоBot

**AI-инфраструктура для `НедоNews Chat`: первый комментарий под постом, память контекста, статистика и немного магии вокруг живого Telegram-чата.**

<p>
  <a href="https://t.me/ned0news">
    <img alt="Telegram channel" src="https://img.shields.io/badge/Telegram-ned0news-26A5E4?style=for-the-badge&logo=telegram&logoColor=white">
  </a>
  <a href="docs/TECHNICAL.md">
    <img alt="Technical docs" src="https://img.shields.io/badge/docs-technical-8A2BE2?style=for-the-badge&logo=readthedocs&logoColor=white">
  </a>
  <img alt="Project status" src="https://img.shields.io/badge/status-live_mvp-00A67E?style=for-the-badge">
</p>

<p>
  <img alt="Rust" src="https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=white">
  <img alt="Telegram Bot API" src="https://img.shields.io/badge/Telegram_Bot_API-26A5E4?style=for-the-badge&logo=telegram&logoColor=white">
  <img alt="PostgreSQL" src="https://img.shields.io/badge/PostgreSQL-4169E1?style=for-the-badge&logo=postgresql&logoColor=white">
  <img alt="Ollama" src="https://img.shields.io/badge/Ollama-LLM-111111?style=for-the-badge">
  <img alt="Podman" src="https://img.shields.io/badge/Podman-892CA0?style=for-the-badge&logo=podman&logoColor=white">
</p>

<p>
  <a href="https://t.me/ned0news">Канал</a>
  ·
  <a href="docs/TECHNICAL.md">Техническая документация</a>
  ·
  <a href="prompts/first_comment.md">Prompt первого комментария</a>
  ·
  <a href="prompts/tech_rag.md">Tech RAG</a>
</p>

</div>

---

## Что это

**НедоBot** — внутренний AI-помощник для экосистемы [`НедоNews`](https://t.me/ned0news).

Он не пытается заменить людей, модерировать всё подряд или быть универсальным ботом «для любого чата». Его задача проще и полезнее: помогать живому обсуждению появляться быстрее, не терять контекст и понимать, что реально происходит в комьюнити.

Пост вышел → бот понял контекст → написал первый комментарий → чат подхватил тему → данные не потерялись.

## Зачем

В Telegram-чатах много жизни, но мало памяти. Хорошие мысли тонут, одинаковые заходы повторяются, статистика размазывается, а первый комментарий под постом часто решает, будет обсуждение или тишина.

**НедоBot** закрывает этот слой: он стоит рядом с чатом, не шумит громче людей и помогает новостям превращаться в разговор.

## Что умеет

<table>
  <tr>
    <td width="50%">
      <h3>Первый комментарий</h3>
      <p>Видит новый пост из канала, учитывает текст и картинку, убирает служебные хвосты и пишет нормальный первый комментарий в стиле чата.</p>
    </td>
    <td width="50%">
      <h3>Память контекста</h3>
      <p>Запоминает прошлые темы и подмешивает релевантные заметки, чтобы бот не делал вид, будто вчерашних новостей не существовало.</p>
    </td>
  </tr>
  <tr>
    <td width="50%">
      <h3>Tech RAG</h3>
      <p>Подстраховывает техно-новости от очевидной устаревшей дичи: релизы, платформы, железо, версии и повторяющиеся сюжеты.</p>
    </td>
    <td width="50%">
      <h3>Статистика чата</h3>
      <p>Считает активность, реплаи, медиа, реакции и динамику после комментариев, чтобы видеть не только шум, но и настоящую жизнь чата.</p>
    </td>
  </tr>
</table>

## Вайб

НедоBot не должен звучать как корпоративный SMM-отдел.

Он может позвать в чат, подкинуть вопрос, заметить странность новости, аккуратно подтолкнуть спор или дать повод для мемов. Главное — не ломать стиль `НедоNews` и не превращать комментарии в пластик.

> Не «присоединяйтесь к нашему сообществу», а «опять у AMD драйверы отвалились, залетайте обсудим».

## Сейчас в фокусе

- первый комментарий под постом;
- память новостей и анти-повтор CTA;
- статистика активности чата;
- аккуратная работа с LLM/Vision;
- подготовка к расшифровке голосовых с таймкодами и чисткой ASR.

## Roadmap

- [x] первый комментарий под постом канала;
- [x] память прошлых новостей;
- [x] статистика дня, недели и месяца;
- [x] пользовательская статистика;
- [ ] расшифровка голосовых сообщений;
- [ ] embeddings/pgvector для более умной памяти;
- [ ] админка без ручного ковыряния `.env`;
- [ ] более сильный модерационный слой без превращения чата в участок.

## Команды

<details>
<summary>Показать команды</summary>

```text
/ping
/db
/memory
/stats_day
/stats_week
/stats_month
/userstats <id|@username>
```

Есть и служебные команды для форматирования, emoji и отладки, но основная идея не в командах. Бот должен быть фоном, а не отдельной панелью управления космическим кораблём.

</details>

## Под капотом

Rust, teloxide, PostgreSQL, LLM/Vision, prompt-файлы, память, RAG и деплой на VPS.

README остаётся витриной проекта. Все эксплуатационные детали, SQL, конфиги, деплой, нюансы Telegram privacy mode и ограничения Bot API лежат в [`docs/TECHNICAL.md`](docs/TECHNICAL.md).

## Принцип

**НедоBot не должен быть громче людей.**

Он нужен, чтобы чату было проще начать разговор, проще не потерять контекст и проще понять, что реально происходит с сообществом.
