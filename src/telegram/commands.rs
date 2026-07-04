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
    #[command(description = "статистика за текущий день с 05:00 МСК; -r для rich")]
    StatsDay(String),
    #[command(description = "статистика за текущую неделю с понедельника 05:00 МСК; -r для rich")]
    StatsWeek(String),
    #[command(description = "статистика за текущий месяц с 1 числа 05:00 МСК; -r для rich")]
    StatsMonth(String),
    #[command(
        rename = "status",
        description = "статистика: /status day|week|month [-r]"
    )]
    Status(String),
    #[command(
        rename = "topmsg",
        description = "топ 20 пользователей по сообщениям; -r для rich"
    )]
    TopMsg(String),
    #[command(
        rename = "topreact",
        description = "топ 20 сообщений по реакциям со ссылками; -r для rich"
    )]
    TopReact(String),
    #[command(
        rename = "userstats",
        description = "статистика пользователя: /userstats <id|username> [-r], или reply на сообщение"
    )]
    UserStats(String),
    #[command(
        rename = "userstatus",
        description = "alias /userstats: /userstatus <id|username> [-r], или reply"
    )]
    UserStatus(String),
}
