use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const DEFAULT_MUTE_DURATION: Duration = Duration::from_secs(24 * 60 * 60);
pub const WARNING_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
pub const WARNING_MUTE_DURATION: Duration = Duration::from_secs(5 * 24 * 60 * 60);
pub const WARNING_MUTE_THRESHOLD: i64 = 3;
pub const MAX_TARGETS_PER_BATCH: usize = 20;
pub const MAX_DURATION: Duration = Duration::from_secs(365 * 24 * 60 * 60);
pub const MIN_DURATION: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    Mute,
    Ban,
    Warn,
    Unmute,
    Unban,
    Unwarn,
    Warns,
    Modlog,
    Undo,
}

impl CommandKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mute => "mute",
            Self::Ban => "ban",
            Self::Warn => "warn",
            Self::Unmute => "unmute",
            Self::Unban => "unban",
            Self::Unwarn => "unwarn",
            Self::Warns => "warns",
            Self::Modlog => "modlog",
            Self::Undo => "undo",
        }
    }

    pub fn mutates(self) -> bool {
        !matches!(self, Self::Warns | Self::Modlog)
    }

    pub fn requires_restrict_right(self) -> bool {
        matches!(
            self,
            Self::Mute | Self::Ban | Self::Warn | Self::Unmute | Self::Unban | Self::Undo
        )
    }
}

pub fn should_escalate_warning(active_count: i64, has_active_restriction: bool) -> bool {
    active_count >= WARNING_MUTE_THRESHOLD && !has_active_restriction
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    pub kind: CommandKind,
    pub targets: Vec<String>,
    pub duration: Option<Duration>,
    pub reason: Option<String>,
    pub warn_id: Option<i64>,
    pub warn_all: bool,
    pub limit: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BatchRequestSnapshot {
    pub kind: CommandKind,
    pub target_user_ids: Vec<i64>,
    pub duration_seconds: Option<u64>,
    pub reason: Option<String>,
    pub warn_id: Option<i64>,
    pub warn_all: bool,
    pub limit: i64,
}

impl BatchRequestSnapshot {
    pub fn from_parsed(parsed: &ParsedCommand, target_user_ids: Vec<i64>) -> Self {
        Self {
            kind: parsed.kind,
            target_user_ids,
            duration_seconds: parsed.duration.map(|duration| duration.as_secs()),
            reason: parsed.reason.clone(),
            warn_id: parsed.warn_id,
            warn_all: parsed.warn_all,
            limit: parsed.limit,
        }
    }

    pub fn duration(&self) -> Option<Duration> {
        self.duration_seconds.map(Duration::from_secs)
    }

    pub fn parsed_command(&self) -> ParsedCommand {
        ParsedCommand {
            kind: self.kind,
            targets: Vec::new(),
            duration: self.duration(),
            reason: self.reason.clone(),
            warn_id: self.warn_id,
            warn_all: self.warn_all,
            limit: self.limit,
        }
    }
}

pub fn parse_command(kind: CommandKind, args: &str) -> Result<ParsedCommand, String> {
    let (head, reason) = split_reason(args)?;
    let tokens = head.split_whitespace().collect::<Vec<_>>();
    let mut parsed = ParsedCommand {
        kind,
        targets: Vec::new(),
        duration: None,
        reason,
        warn_id: None,
        warn_all: false,
        limit: 20,
    };

    if kind == CommandKind::Undo && !tokens.is_empty() {
        return Err("/undo не принимает аргументы".to_string());
    }
    if matches!(kind, CommandKind::Warns | CommandKind::Modlog) && parsed.reason.is_some() {
        return Err("для этой команды разделитель -- не используется".to_string());
    }

    match kind {
        CommandKind::Mute | CommandKind::Ban | CommandKind::Warn => {
            let mut remaining = tokens.as_slice();
            if let Some(first) = remaining.first().copied()
                && let Some(duration) = parse_duration_token(first)?
            {
                validate_finite_duration(duration)?;
                parsed.duration = Some(duration);
                remaining = &remaining[1..];
            }
            parsed.targets = parse_targets(remaining)?;
            if parsed.targets.len() > MAX_TARGETS_PER_BATCH {
                return Err(format!(
                    "за один раз можно указать не больше {MAX_TARGETS_PER_BATCH} пользователей"
                ));
            }
            if parsed.duration.is_none() {
                parsed.duration = match kind {
                    CommandKind::Mute => Some(DEFAULT_MUTE_DURATION),
                    CommandKind::Warn => Some(WARNING_TTL),
                    CommandKind::Ban => None,
                    _ => unreachable!(),
                };
            }
        }
        CommandKind::Unmute | CommandKind::Unban | CommandKind::Warns => {
            parsed.targets = parse_targets(&tokens)?;
            if parsed.targets.len() > MAX_TARGETS_PER_BATCH {
                return Err(format!(
                    "за один раз можно указать не больше {MAX_TARGETS_PER_BATCH} пользователей"
                ));
            }
        }
        CommandKind::Unwarn => {
            let mut target_tokens = Vec::new();
            for token in tokens {
                if token.eq_ignore_ascii_case("all") {
                    if parsed.warn_id.is_some() || parsed.warn_all {
                        return Err("укажи только один warn id или all".to_string());
                    }
                    parsed.warn_all = true;
                } else if let Some(id) = token.strip_prefix('#') {
                    let id = id.parse::<i64>().ok().filter(|id| *id > 0).ok_or_else(|| {
                        "warn id должен быть положительным числом после #".to_string()
                    })?;
                    if parsed.warn_id.replace(id).is_some() || parsed.warn_all {
                        return Err("укажи только один warn id или all".to_string());
                    }
                } else {
                    target_tokens.push(token);
                }
            }
            parsed.targets = parse_targets(&target_tokens)?;
            if parsed.targets.len() > MAX_TARGETS_PER_BATCH {
                return Err(format!(
                    "за один раз можно указать не больше {MAX_TARGETS_PER_BATCH} пользователей"
                ));
            }
        }
        CommandKind::Modlog => {
            let mut target_tokens = tokens.as_slice();
            if let Some(last) = tokens.last()
                && let Ok(limit) = last.parse::<i64>()
                && tokens.len() > 1
            {
                if !(1..=100).contains(&limit) {
                    return Err("лимит /modlog должен быть от 1 до 100".to_string());
                }
                parsed.limit = limit;
                target_tokens = &tokens[..tokens.len() - 1];
            }
            parsed.targets = parse_targets(target_tokens)?;
            if parsed.targets.len() > 1 {
                return Err("/modlog принимает не больше одной цели".to_string());
            }
        }
        CommandKind::Undo => {}
    }

    Ok(parsed)
}

fn split_reason(args: &str) -> Result<(&str, Option<String>), String> {
    let Some((head, reason)) = args.split_once("--") else {
        return Ok((args, None));
    };
    if reason.contains("--") {
        return Err("используй разделитель -- только один раз".to_string());
    }
    let reason = reason.trim();
    Ok((head, (!reason.is_empty()).then(|| reason.to_string())))
}

fn parse_targets(tokens: &[&str]) -> Result<Vec<String>, String> {
    tokens
        .iter()
        .map(|token| {
            let token = token.trim();
            if let Ok(user_id) = token.parse::<i64>() {
                if user_id > 0 {
                    return Ok(user_id.to_string());
                }
                return Err("Telegram ID должен быть положительным числом".to_string());
            }
            let Some(username) = token.strip_prefix('@') else {
                return Err(format!(
                    "непонятная цель {token:?}; используй @username или Telegram ID"
                ));
            };
            if username.is_empty()
                || username.len() > 32
                || !username
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            {
                return Err(format!("неверный username {token:?}"));
            }
            Ok(format!("@{}", username.to_ascii_lowercase()))
        })
        .collect()
}

fn parse_duration_token(token: &str) -> Result<Option<Duration>, String> {
    if !token.chars().next().is_some_and(|ch| ch.is_ascii_digit())
        || !token.chars().any(char::is_alphabetic)
    {
        return Ok(None);
    }
    let duration = parse_compound_duration(token)
        .ok_or_else(|| format!("неверный формат срока {token:?}; примеры: 30m, 2h, 3d, 2d3h"))?;
    Ok(Some(duration))
}

pub fn parse_compound_duration(value: &str) -> Option<Duration> {
    if value.is_empty() {
        return None;
    }
    let mut number = 0u64;
    let mut total = 0u64;
    let mut seen_unit = false;
    for ch in value.chars() {
        if ch.is_ascii_digit() {
            number = number
                .checked_mul(10)?
                .checked_add(ch.to_digit(10)? as u64)?;
            continue;
        }
        let multiplier = match ch {
            's' => 1,
            'm' => 60,
            'h' => 60 * 60,
            'd' => 24 * 60 * 60,
            _ => return None,
        };
        if number == 0 {
            return None;
        }
        total = total.checked_add(number.checked_mul(multiplier)?)?;
        number = 0;
        seen_unit = true;
    }
    if number != 0 || !seen_unit || total == 0 {
        return None;
    }
    Some(Duration::from_secs(total))
}

fn validate_finite_duration(duration: Duration) -> Result<(), String> {
    if duration < MIN_DURATION || duration > MAX_DURATION {
        return Err("срок должен быть от 60 секунд до 365 дней".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mute_and_ban_defaults_are_explicit() {
        assert_eq!(
            parse_command(CommandKind::Mute, "").unwrap().duration,
            Some(DEFAULT_MUTE_DURATION)
        );
        assert_eq!(parse_command(CommandKind::Ban, "").unwrap().duration, None);
    }

    #[test]
    fn warning_defaults_to_thirty_days_and_has_optional_reason() {
        let parsed = parse_command(CommandKind::Warn, "@someone").unwrap();
        assert_eq!(parsed.duration, Some(WARNING_TTL));
        assert_eq!(parsed.targets, ["@someone"]);
        assert_eq!(parsed.reason, None);

        let custom = parse_command(CommandKind::Warn, "7d @someone -- repeat").unwrap();
        assert_eq!(custom.duration, Some(Duration::from_secs(7 * 24 * 60 * 60)));
        assert_eq!(custom.reason.as_deref(), Some("repeat"));
    }

    #[test]
    fn parses_multiple_targets_and_optional_reason() {
        let parsed = parse_command(CommandKind::Mute, "2d3h @Alice 123456 -- repeat spam").unwrap();
        assert_eq!(parsed.duration, Some(Duration::from_secs(183600)));
        assert_eq!(parsed.targets, ["@alice", "123456"]);
        assert_eq!(parsed.reason.as_deref(), Some("repeat spam"));
    }

    #[test]
    fn rejects_malformed_and_out_of_bounds_durations() {
        for value in ["0h", "2x", "999999999999999999999d", "30s", "366d"] {
            assert!(parse_command(CommandKind::Mute, value).is_err(), "{value}");
        }
        assert_eq!(
            parse_command(CommandKind::Mute, "60s").unwrap().duration,
            Some(Duration::from_secs(60))
        );
    }

    #[test]
    fn duration_tokens_do_not_accept_duplicate_or_unordered_units() {
        assert_eq!(
            parse_compound_duration("2d3h"),
            Some(Duration::from_secs(183600))
        );
        assert_eq!(
            parse_compound_duration("3h2d"),
            Some(Duration::from_secs(183600))
        );
        assert_eq!(parse_compound_duration("2dd"), None);
        assert_eq!(parse_compound_duration("2d3"), None);
    }

    #[test]
    fn batch_target_count_is_bounded() {
        let users = (1..=21)
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(parse_command(CommandKind::Warn, &users).is_err());
    }

    #[test]
    fn unwind_warn_parser_accepts_specific_or_all() {
        let by_id = parse_command(CommandKind::Unwarn, "@alice #8").unwrap();
        assert_eq!(by_id.warn_id, Some(8));
        let all = parse_command(CommandKind::Unwarn, "@alice all").unwrap();
        assert!(all.warn_all);
    }

    #[test]
    fn third_active_warning_escalates_once_until_restriction_is_cleared() {
        assert!(!should_escalate_warning(2, false));
        assert!(should_escalate_warning(3, false));
        assert!(!should_escalate_warning(4, true));
    }
}
