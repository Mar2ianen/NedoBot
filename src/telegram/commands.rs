use teloxide::utils::command::BotCommands;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "snake_case")]
pub enum Command {
    #[command(description = "показать это меню")]
    Help,
    #[command(description = "проверить, что бот жив")]
    Ping,
    #[command(description = "проверить подключение к базе")]
    Db,
    #[command(description = "показать custom_emoji_id из сообщения")]
    EmojiIds,
    #[command(description = "проверить формат первого комментария")]
    FormatTest(String),
    #[command(description = "показать последние заметки памяти")]
    Memory,
    #[command(description = "спросить помощника по истории чата; /ask <вопрос>")]
    Ask(String),
    #[command(description = "добавить общую заметку чата; /chat_note <текст>")]
    ChatNote(String),
    #[command(description = "добавить заметку о пользователе reply; /user_note <текст>")]
    UserNote(String),
    #[command(description = "статистика за текущий день с 05:00 МСК; [-r|-p]")]
    StatsDay(String),
    #[command(description = "статистика за текущую неделю с понедельника 05:00 МСК; [-r|-p]")]
    StatsWeek(String),
    #[command(description = "статистика за текущий месяц с 1 числа 05:00 МСК; [-r|-p]")]
    StatsMonth(String),
    #[command(
        rename = "status",
        description = "статистика: /status day|week|month [-r|-p]"
    )]
    Status(String),
    #[command(
        rename = "topmsg",
        description = "топ 20 пользователей по сообщениям; [-r|-p]"
    )]
    TopMsg(String),
    #[command(
        rename = "topreact",
        description = "топ 20 сообщений по реакциям со ссылками; [-r|-p]"
    )]
    TopReact(String),
    #[command(
        rename = "userstats",
        description = "статистика пользователя: /userstats <id|username> [-r|-p], или reply на сообщение"
    )]
    UserStats(String),
    #[command(
        rename = "userstatus",
        description = "alias /userstats: /userstatus <id|username> [-r|-p], или reply"
    )]
    UserStatus(String),
}
