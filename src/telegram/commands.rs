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
    #[command(description = "статистика за текущий день с 05:00 МСК")]
    StatsDay,
    #[command(description = "статистика за текущую неделю с понедельника 05:00 МСК")]
    StatsWeek,
    #[command(description = "статистика за текущий месяц с 1 числа 05:00 МСК")]
    StatsMonth,
    #[command(rename = "topmsg", description = "топ 20 пользователей по сообщениям")]
    TopMsg,
    #[command(
        rename = "topreact",
        description = "топ 20 сообщений по реакциям со ссылками"
    )]
    TopReact,
    #[command(
        rename = "userstats",
        description = "статистика пользователя: /userstats <id|username>, или reply на сообщение"
    )]
    UserStats(String),
}
