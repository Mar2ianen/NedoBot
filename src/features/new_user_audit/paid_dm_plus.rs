use teloxide_antispam::preprocess::prepare_text;

/// Высокоточное правило для предложений «поставь плюс в личку за деньги».
/// Нужны сумма в рублях, предложение выплаты, личный канал связи и явный CTA с плюсом.
pub(crate) fn matches_paid_dm_plus_offer(text: &str) -> bool {
    let canonical = prepare_text(text).canonical.replace('ё', "е");
    let tokens = canonical
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();

    has_amount_and_currency(&canonical, &tokens)
        && has_payment_offer(&tokens)
        && has_private_message_destination(&tokens)
        && has_plus_call_to_action(&canonical)
}

fn has_amount_and_currency(canonical: &str, tokens: &[&str]) -> bool {
    let amount_before_currency = tokens.windows(2).any(|pair| {
        is_amount(pair[0]) && is_ruble_currency(pair[1])
            || is_ruble_currency(pair[0]) && is_amount(pair[1])
    });
    let compact_amount = tokens.iter().any(|token| {
        let digit_count = token.chars().take_while(|ch| ch.is_ascii_digit()).count();
        digit_count >= 2 && is_ruble_currency(&token[digit_count..])
    });
    let symbol_amount = canonical.contains('₽') && tokens.iter().any(|token| is_amount(token));

    amount_before_currency || compact_amount || symbol_amount
}

fn is_amount(token: &str) -> bool {
    token.chars().count() >= 2 && token.chars().all(|ch| ch.is_ascii_digit())
}

fn is_ruble_currency(token: &str) -> bool {
    token == "р" || token.starts_with("руб")
}

fn has_payment_offer(tokens: &[&str]) -> bool {
    const PAYMENT_PHRASES: &[&[&str]] = &[
        &["с", "меня"],
        &["дам"],
        &["плачу"],
        &["оплачу"],
        &["получишь"],
        &["заработаешь"],
        &["заработок"],
        &["доход"],
        &["вознаграждение"],
    ];

    PAYMENT_PHRASES
        .iter()
        .any(|phrase| tokens.windows(phrase.len()).any(|window| window == *phrase))
}

fn has_private_message_destination(tokens: &[&str]) -> bool {
    tokens
        .iter()
        .any(|token| matches!(*token, "личку" | "личке" | "лс" | "директ" | "dm"))
        || tokens.windows(2).any(|pair| {
            pair[0] == "личные" && pair[1] == "сообщения"
                || pair[0] == "в" && matches!(pair[1], "личку" | "лс" | "директ")
        })
}

fn has_plus_call_to_action(canonical: &str) -> bool {
    const CTA_PHRASES: &[&str] = &[
        "тебя +",
        "кидай +",
        "скинь +",
        "поставь +",
        "отправь +",
        "дай +",
        "напиши +",
        "пиши +",
        "поставь плюс",
        "скинь плюс",
        "кидай плюс",
        "отправь плюс",
        "напиши плюс",
        "пиши плюс",
        "плюс в личку",
        "плюс в лс",
        "плюс в директ",
    ];

    CTA_PHRASES.iter().any(|phrase| canonical.contains(phrase))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_paid_plus_offers_sent_to_private_messages() {
        for text in [
            "С тебя + в личку — с меня 300 ₽",
            "Кидай + в директ — с меня 400 руб.",
            "Поставь плюс и напиши в лс, дам 500 рублей",
            "С тебя + в личку — с меня 300 р",
        ] {
            assert!(matches_paid_dm_plus_offer(text), "should match {text:?}");
        }
    }

    #[test]
    fn normalizes_cyrillic_lookalikes_before_matching() {
        assert!(matches_paid_dm_plus_offer(
            "С тебя + в личкy — с меня 300 ₽"
        ));
    }

    #[test]
    fn does_not_match_without_all_three_signal_groups() {
        for text in [
            "Напишите в личку, ищу няню",
            "С меня 300 ₽ за помощь, спасибо",
            "Кидай плюс в директ, обсудим тему",
            "С меня 300 ₽, плюс был в предыдущем сообщении; напиши в личку",
            "Зарплата 5000 рублей, вакансия уборщицы",
            "В директе есть информация о выплатах 300 ₽",
        ] {
            assert!(!matches_paid_dm_plus_offer(text), "must not match {text:?}");
        }
    }
}
