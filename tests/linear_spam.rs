//! Совместимость уже используемого ботом артефакта остаётся тестом адаптера.
#[test]
fn legacy_deployed_model_keeps_python_parity() {
    let model =
        teloxide_antispam::logreg::load_model(include_str!("../models/linear_spam_word12_v1.json"))
            .unwrap();
    let text = "Недавно ехал за рулем слушал книгу  «Время - деньги». Понравилась тем что она нас учит относиться ко времени и деньгам не как к бесконечному ресурсу, а как к топливу для ракеты: не тратить его на бессмысленную гонку, а мыслить стратегически и фокусироваться на главном. Есть аудио - могу переслать. Пишите в личку, перешлю бесплатно";
    let probability = teloxide_antispam::logreg::spam_probability(&model, text);
    assert!(
        (probability - 0.984_341).abs() < 1e-4,
        "parity drift: {probability}"
    );
}
