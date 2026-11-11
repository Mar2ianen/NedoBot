# Капча в комментариях без публичной карточки

Капча включается только существующей policy для подозрительного автора.
Пороги и модель подозрения не меняются. Для остальных сообщений дополнительных
проверок, уведомлений или вызовов LLM нет.

## Доставка

1. Находим собственное свежее сообщение проверяемого автора в пределах
   текущего аудита. Channel posts, другой чат/автор, служебные события и
   уже удалённые сообщения исключены.
2. Отправляем только ephemeral sendMessage с receiver_user_id автора и
   reply_parameters.message_id его сообщения. allow_sending_without_reply
   не включён: исчезнувший reply target не превращается в отдельную карточку.
3. Требуем ephemeral_message_id в ответе API и сохраняем карточку/TTL в БД.
4. Только затем удаляем сообщения в прежних границах captcha cleanup.

Ошибка удаления после принятого reply не переотправляет карточку и не
продлевает TTL. Следующий high-risk audit может повторить только cleanup.
При отклонённой отправке комментарий не удаляется, TTL не запускается.
Прежний bounded setup retry остаётся; после исчерпания попыток бот снимает
только собственное ограничение и уважает параллельные решения администратора.
Публичного fallback, общей справки, DM-рассылки и принудительного вступления нет.

## Автор не состоит в discussion group

Статус left не исключает наличие комментариев. При наличии своего свежего
reply target разрешена попытка ephemeral reply. Бот не мутит non-member,
не вступает за него в чат и не kick/ban-ит его за истечение такой капчи.
Callback по-прежнему привязан к challenge, Telegram chat и владельцу.
Правильный ответ фиксирует passed без изменения членства или чужих прав.

Поддержка такой доставки конкретным Telegram server/client не доказана
mock-тестом. Если API её отклоняет, результат — setup_failed без активного
TTL и без публичной замены. Нельзя записывать этот случай как успешную
проверку или блокировку спамера.

## Ограничение Telegram

Ephemeral reply с message_id поддерживается API. Но Telegram отдельно
предупреждает, что ephemeral-сообщение может не дойти, особенно offline;
произвольная отправка администратором документирована для members.
Обычные ответы на комментарии могут приходить в чат «Ответы», но переносить
эту гарантию на ephemeral нельзя. API ack/ephemeral_message_id не доказывает
уведомление, прочтение или сохранность карточки после рестарта клиента.
Прежняя timeout policy для ограниченного участника остаётся; патч исправляет
reply-контекст и порядок действий, но не добавляет Telegram read receipts.

Источники: [ephemeral delivery](https://core.telegram.org/bots/api#ephemeral-messages-and-commands),
[reply parameters](https://core.telegram.org/bots/api#replyparameters),
[discussion replies](https://core.telegram.org/api/discussion).

## Проверки

PostgreSQL/typed Telegram stub проверяет sender/chat/audit scope reply target,
ephemeral recipient, порядок send → persist → delete, отказ доставки и
отсутствие TTL/cleanup/public fallback, отсутствие ephemeral ID, повтор cleanup
без второй карточки, non-member callback и отсутствие ban/restrict при expiry.
Это контрактные проверки, а не live-проверка уведомления на телефоне.
