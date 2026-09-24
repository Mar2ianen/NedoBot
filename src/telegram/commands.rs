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
    #[cfg(feature = "auto-comment")]
    #[command(description = "проверить формат первого комментария")]
    FormatTest(String),
    #[command(description = "показать последние заметки памяти")]
    Memory,
    #[cfg(feature = "voice")]
    #[command(description = "расшифровать voice, audio или кружок в reply")]
    Transcribe,
    #[cfg(feature = "ask")]
    #[command(description = "спросить помощника по истории чата; /ask <вопрос>")]
    Ask(String),
    #[cfg(feature = "ask")]
    #[command(description = "добавить общую заметку чата; /chat_note <текст>")]
    ChatNote(String),
    #[cfg(feature = "ask")]
    #[command(description = "добавить заметку о пользователе reply; /user_note <текст>")]
    UserNote(String),
    #[cfg(feature = "moderation")]
    #[command(description = "пожаловаться на сообщение reply; /report [причина]")]
    Report(String),
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

#[cfg(all(test, feature = "moderation"))]
mod tests {
    use super::Command;
    use teloxide::utils::command::BotCommands;

    #[test]
    fn report_command_parses_reply_reason() {
        assert!(matches!(
            Command::parse("/report рекламная ссылка", "nedobot"),
            Ok(Command::Report(reason)) if reason == "рекламная ссылка"
        ));
    }
}
