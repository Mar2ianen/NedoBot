use teloxide::utils::command::BotCommands;

#[cfg(test)]
mod top_word_tests {
    use super::Command;
    use teloxide::utils::command::BotCommands;

    #[test]
    fn topword_command_parses_the_word_and_render_flag() {
        assert!(matches!(
            Command::parse("/topword амудятел -p", "nedobot"),
            Ok(Command::TopWord(args)) if args == "амудятел -p"
        ));
    }
}

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
    #[cfg(feature = "moderation")]
    #[command(description = "пометить сообщение reply как не спам (reviewer)")]
    Notspam(String),
    #[cfg(feature = "manual-moderation")]
    #[command(
        description = "ограничить участника на 1 день или указанный срок; причина необязательна"
    )]
    Mute(String),
    #[cfg(feature = "manual-moderation")]
    #[command(
        description = "забанить участника навсегда или на указанный срок; причина необязательна"
    )]
    Ban(String),
    #[cfg(feature = "manual-moderation")]
    #[command(
        description = "выдать предупреждение; три активных предупреждения дают мут на 5 дней"
    )]
    Warn(String),
    #[cfg(feature = "manual-moderation")]
    #[command(description = "снять выданный ботом mute")]
    Unmute(String),
    #[cfg(feature = "manual-moderation")]
    #[command(description = "снять выданный ботом ban")]
    Unban(String),
    #[cfg(feature = "manual-moderation")]
    #[command(description = "отозвать предупреждения: /unwarn [цель] [#id|all]")]
    Unwarn(String),
    #[cfg(feature = "manual-moderation")]
    #[command(description = "показать предупреждения участника: /warns <id|username> или reply")]
    Warns(String),
    #[cfg(feature = "manual-moderation")]
    #[command(description = "показать журнал модерации: /modlog [цель] [лимит]")]
    Modlog(String),
    #[cfg(feature = "manual-moderation")]
    #[command(description = "отменить последнюю свою команду модерации в этом чате")]
    Undo,
    #[command(description = "статистика за текущий день с 05:00 МСК; [-r|-p] [-e]")]
    StatsDay(String),
    #[command(description = "статистика за текущую неделю с понедельника 05:00 МСК; [-r|-p] [-e]")]
    StatsWeek(String),
    #[command(description = "статистика за текущий месяц с 1 числа 05:00 МСК; [-r|-p] [-e]")]
    StatsMonth(String),
    #[command(
        rename = "status",
        description = "статистика: /status day|week|month [-r|-p] [-e]"
    )]
    Status(String),
    #[command(
        rename = "topmsg",
        description = "топ 20 пользователей по сообщениям; [-r|-p] [-e]"
    )]
    TopMsg(String),
    #[command(
        rename = "topword",
        description = "топ 20 пользователей по употреблению слова; /topword <слово> [-r|-p] [-e]"
    )]
    TopWord(String),
    #[command(
        rename = "topreact",
        description = "топ 20 сообщений по реакциям со ссылками; [-r|-p] [-e]"
    )]
    TopReact(String),
    #[command(
        rename = "bottommsg",
        description = "20 самых молчаливых пользователей; [-r|-p] [-e]"
    )]
    BottomMsg(String),
    #[command(
        rename = "userstats",
        description = "статистика пользователя: /userstats <id|username> [-r|-p] [-e], или reply на сообщение"
    )]
    UserStats(String),
    #[command(
        rename = "userstatus",
        description = "alias /userstats: /userstatus <id|username> [-r|-p] [-e], или reply"
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

#[cfg(all(test, feature = "manual-moderation"))]
mod manual_moderation_tests {
    use super::Command;
    use teloxide::utils::command::BotCommands;

    #[test]
    fn moderation_commands_allow_missing_reason_and_parameters() {
        assert!(
            matches!(Command::parse("/mute", "bot"), Ok(Command::Mute(args)) if args.is_empty())
        );
        assert!(
            matches!(Command::parse("/ban @alice", "bot"), Ok(Command::Ban(args)) if args == "@alice")
        );
        assert!(
            matches!(Command::parse("/warn", "bot"), Ok(Command::Warn(args)) if args.is_empty())
        );
        assert!(matches!(Command::parse("/undo", "bot"), Ok(Command::Undo)));
    }
}
