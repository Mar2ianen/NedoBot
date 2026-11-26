use teloxide::utils::command::BotCommands;

#[cfg(test)]
mod top_word_tests {
    use super::Command;
    use teloxide::utils::command::BotCommands;

    #[test]
    fn ping_command_accepts_optional_mode() {
        assert!(matches!(Command::parse("/ping", "nedobot"), Ok(Command::Ping(a)) if a.is_empty()));
        assert!(
            matches!(Command::parse("/ping full", "nedobot"), Ok(Command::Ping(a)) if a == "full")
        );
        assert!(
            matches!(Command::parse("/ping probe", "nedobot"), Ok(Command::Ping(a)) if a == "probe")
        );
    }

    #[test]
    fn topword_command_parses_the_word_and_render_flag() {
        assert!(matches!(
            Command::parse("/topword амудятел -p", "nedobot"),
            Ok(Command::TopWord(args)) if args == "амудятел -p"
        ));
    }

    #[cfg(feature = "ask")]
    #[test]
    fn forget_command_has_no_target_argument() {
        assert!(matches!(
            Command::parse("/forget", "nedobot"),
            Ok(Command::Forget(args)) if args.is_empty()
        ));
        assert!(matches!(
            Command::parse("/forget 123", "nedobot"),
            Ok(Command::Forget(args)) if !args.is_empty()
        ));
    }

    #[cfg(feature = "manual-moderation")]
    #[test]
    fn manual_moderation_command_is_registered_when_its_feature_is_compiled() {
        assert!(matches!(
            Command::parse("/ban 123", "nedobot"),
            Ok(Command::Ban(_))
        ));
        assert!(matches!(
            Command::parse("/channelops 42 applied", "nedobot"),
            Ok(Command::ChannelOps(args)) if args == "42 applied"
        ));
    }

    #[cfg(not(feature = "manual-moderation"))]
    #[test]
    fn manual_moderation_command_is_not_registered_without_its_feature() {
        assert!(Command::parse("/ban 123", "nedobot").is_err());
    }
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "snake_case")]
pub enum Command {
    #[command(description = "показать это меню")]
    Help,
    #[command(description = "диагностика бота; /ping [full|probe]")]
    Ping(String),
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
    #[command(description = "забыть твою память этого диалога")]
    Forget(String),
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
    #[cfg(feature = "manual-moderation")]
    #[command(
        rename = "channelops",
        description = "показать или подтвердить незавершённые операции с каналами: /channelops [id applied|not_applied]"
    )]
    ChannelOps(String),
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

#[cfg(test)]
mod command_inventory_tests {
    use super::Command;
    use teloxide::utils::command::BotCommands;

    #[test]
    fn ungated_commands_remain_parseable() {
        for (input, bot_name) in [
            ("/help", "nedobot"),
            ("/ping", "nedobot"),
            ("/db", "nedobot"),
            ("/emoji_ids", "nedobot"),
            ("/memory", "nedobot"),
            ("/stats_day", "nedobot"),
            ("/stats_week", "nedobot"),
            ("/stats_month", "nedobot"),
            ("/status", "nedobot"),
            ("/topmsg", "nedobot"),
            ("/topword rust", "nedobot"),
            ("/topreact", "nedobot"),
            ("/bottommsg", "nedobot"),
            ("/userstats 42", "nedobot"),
            ("/userstatus 42", "nedobot"),
        ] {
            assert!(
                Command::parse(input, bot_name).is_ok(),
                "{input} must parse"
            );
        }
    }

    #[cfg(feature = "ask")]
    #[test]
    fn ask_commands_are_present_only_with_the_ask_feature() {
        for input in [
            "/ask synthetic question",
            "/forget",
            "/chat_note synthetic",
            "/user_note synthetic",
        ] {
            assert!(
                Command::parse(input, "nedobot").is_ok(),
                "{input} must parse"
            );
        }
    }

    #[cfg(not(feature = "ask"))]
    #[test]
    fn ask_commands_are_not_registered_without_the_ask_feature() {
        assert!(Command::parse("/ask synthetic question", "nedobot").is_err());
    }

    #[cfg(feature = "moderation")]
    #[test]
    fn moderation_commands_are_present_with_the_moderation_feature() {
        assert!(Command::parse("/report synthetic", "nedobot").is_ok());
        assert!(Command::parse("/notspam synthetic", "nedobot").is_ok());
    }

    #[cfg(not(feature = "moderation"))]
    #[test]
    fn moderation_commands_are_not_registered_without_the_moderation_feature() {
        assert!(Command::parse("/report synthetic", "nedobot").is_err());
    }

    #[cfg(feature = "manual-moderation")]
    #[test]
    fn manual_moderation_commands_are_present_with_their_feature() {
        for input in [
            "/mute", "/ban", "/warn", "/unmute", "/unban", "/unwarn", "/warns", "/modlog", "/undo",
        ] {
            assert!(
                Command::parse(input, "nedobot").is_ok(),
                "{input} must parse"
            );
        }
    }

    #[cfg(not(feature = "manual-moderation"))]
    #[test]
    fn manual_moderation_commands_are_not_registered_without_their_feature() {
        assert!(Command::parse("/ban", "nedobot").is_err());
    }

    #[cfg(feature = "voice")]
    #[test]
    fn transcription_command_is_registered_with_voice_feature() {
        assert!(Command::parse("/transcribe", "nedobot").is_ok());
    }

    #[cfg(not(feature = "voice"))]
    #[test]
    fn transcription_command_is_not_registered_without_voice_feature() {
        assert!(Command::parse("/transcribe", "nedobot").is_err());
    }

    #[cfg(feature = "auto-comment")]
    #[test]
    fn format_test_command_is_registered_with_auto_comment_feature() {
        assert!(Command::parse("/format_test synthetic", "nedobot").is_ok());
    }

    #[cfg(not(feature = "auto-comment"))]
    #[test]
    fn format_test_command_is_not_registered_without_auto_comment_feature() {
        assert!(Command::parse("/format_test synthetic", "nedobot").is_err());
    }
}
