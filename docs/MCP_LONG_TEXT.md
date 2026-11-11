# Длинные тексты MCP и YouTube

## Исправленная потеря данных

11 ноября 2026 запрос youtube.get_subtitles для
https://youtu.be/H37hGVoz3ko вернул ровно 12000 символов с обрывом слова,
note=null и без продолжения. Это ограничение текста адаптера, а не доказанный
13-минутный предел длительности YouTube. yt-dlp успешно получил субтитры.

## Субтитры

Старый вызов с urls поддерживается. Его результат теперь содержит явные
total_chars, offset, returned_chars, has_more, next_offset и transcript_id.
Дополнительно возвращаются язык и время последнего cue subtitle_end_seconds.
Это время покрытия субтитрами, а не обещание наличия субтитров на каждом
моменте ролика или точная длительность видео.

```json
{"urls":["https://youtu.be/H37hGVoz3ko"],"max_chars":6000}
```

Пока has_more=true, для каждого ролика отдельно передавать next_offset и
transcript_id из предыдущего ответа:

```json
{"urls":["https://youtu.be/H37hGVoz3ko"],"offset":6000,"max_chars":6000,"transcript_id":"<64 hex characters from response>"}
```

offset измеряется в символах Unicode. max_chars ограничивает страницу, не
длину ролика; при нескольких urls бюджет делится между роликами. По умолчанию
страница содержит до 6000 символов, не более настроенного server budget.
has_more=false означает, что возвращено окончание выбранной subtitle track;
первый фрагмент с has_more=true нельзя называть просмотром целого видео.

Полный текст используется из памяти конкретного процесса/инстанса до часа:
не более восьми записей, читаемый VTT не более 8 MiB. Просроченные записи
удаляются следующим запросом или рестартом. Временные файлы удаляются.
После eviction/рестарта текст загружается заново; несовпавший transcript_id
даёт ошибку с требованием начать сначала. Между инстансами нет общего кеша
разговоров или пользователей. Обрабатываются только публичные прямые HTTPS
YouTube video URL; cookies, произвольные CLI options и URL не принимаются.

Одновременно запускается один yt-dlp на экземпляр сервиса. При занятости
возвращается controlled retryable error без неограниченной очереди. Все urls
проверяются до первого запуска. Ошибка/таймаут provider не превращает частичный
файл в успешный transcript. Stdout отключён, stderr сохраняется только в
ограниченный внутренний buffer и не выдаётся клиенту.

Языки проверяются по приоритету до первой успешной исходной дорожки, в рамках
одного общего provider deadline на ролик. Автопереводы YouTube отключены через
youtube:skip=translated_subs: их отдельная ошибка 429 не должна лишать клиента
уже доступной оригинальной речи. При отсутствии первого языка пробуется
следующий; при настоящей ошибке/429 выбранной дорожки частичный файл не выдаётся.
В production-проверке H37hGVoz3ko запрос ru,en завершался ошибкой после
полного ru-файла, тогда как отдельный ru завершился с exit=0 и последним cue
00:52:47.725. Это основание для последовательного выбора, не обход блокировки.

MCP_YOUTUBE_SUBTITLES_TIMEOUT_SEC: default 60, диапазон 1..120; предел страницы
MCP_YOUTUBE_SUBTITLES_MAX_CHARS: 256..50000; видео за вызов: 1..5. Конкретная
конфигурация production может быть строже. Максимальный HTTP deadline — 630 с,
Nginx upstream — 650 с. Body timeout 15 с, body size 64 KiB, rate/connection
limits и PostgreSQL read-only ACL/statement timeout сохранены. Внешний MCP-клиент
может иметь собственный deadline, который сервер увеличить не может.

## Полные поля reviewed базы

Обычный db.select ограничивает text-cell и сообщает truncation. Новый
db.read_text читает только одну text-колонку уже разрешённой manifest view по
её полному primary key, с limit=1..12000 и offset. Для продолжения передавать
next_offset и revision. revision — MD5 выбранного текста для обнаружения
редактирования между страницами, не credential и не механизм авторизации.

```json
{"table":"voice_transcription_jobs","column":"raw_transcript","key":{"id":123},"offset":0,"limit":6000}
```

Primary key конкретной view уточнять через db.describe_table. Схемы/таблицы,
колонки и типы по-прежнему проверяются manifest; caller chat_id не задаёт scope.
Новых view, полей, grants или SQL endpoint нет. Закрытая conversational memory
не становится доступной. db.read_text не добавлен в allowlist /ask.

voice.list_transcripts и memory.list_notes принимают query, limit и cursor;
первый также message_id. Пустой объект остаётся допустимым. Это выборки
публичной ASR/RAG-истории, не приватной памяти диалога.

## /ask

В compiled allowlist добавлены youtube.get_subtitles, voice.list_transcripts,
memory.list_notes. Named AgentProfile по-прежнему должен разрешить инструмент
явно; existing custom allowlist автоматически не расширяется.
YouTube требует runtime.youtube_subtitles_enabled=true и валидную команду.
Эти runtime-параметры явно передаются stdio child после env_clear и имеют
приоритет над случайными MCP env. Выключенный feature gate скрывает инструмент
от модели, не создаёт jobs и не делает внешних запросов.

YouTube-вызов получает собственный ограниченный timeout по provider budget;
обычные DB tools сохраняют прежний timeout. Legacy NedoNews AgentProfile теперь
берёт общий бюджет из runtime.ask_total_timeout_sec, default 3600 с. Named
profiles используют собственный budgets.total_timeout_sec. Число шагов,
вызовов инструментов и max output tokens не увеличено.

Если preview не вмещает subtitle page, next_offset пересчитывается на число
реально показанных модели символов; нельзя перескочить через скрытый хвост.
Данные остаются внутри untrusted JSON envelope. В strict reply scope ASR rows
фильтруются по родительским message_id, общие RAG-карточки не подмешиваются.
Новые tools не записывают долговременную память и не меняют /forget.

Настройки yt-dlp сверены с [официальным README](https://github.com/yt-dlp/yt-dlp#subtitle-options).

## После обновления сервера

Клиент должен заново получить tools/list или обновить подключение MCP.
Старая кешированная schema знает только urls и может отбросить offset,
max_chars и transcript_id до отправки на сервер. Признак: запрос продолжения
с offset=6000 снова возвращает offset=0, а db.read_text отсутствует в каталоге
клиента. Свежий серверный tools/list содержит эти параметры и новый инструмент.
Это отдельно от серверной работоспособности: полный сквозной HTTPS probe
с актуальной схемой проверяет непрерывность всех страниц и checksum текста.
