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

#[test]
fn deployed_gemma_head_loads_with_validated_calibration() {
    let head = teloxide_antispam::embedding::EmbeddingSpamModel::load(include_str!(
        "../models/gemma_768_fx_2026-10-04.json"
    ))
    .unwrap();
    assert_eq!(head.version, "gemma-768-fx-2026-10-04");
    assert_eq!(head.dim, 768);
    assert_eq!(
        head.calibration.version,
        "gemma-fx-validation-2026-10-04-v1"
    );
    // Uniform unit vector: must produce a finite probability, never panic.
    let uniform = vec![1.0f32 / (768.0f32).sqrt(); 768];
    let probability = head
        .spam_probability(&uniform)
        .expect("uniform vector must score");
    assert!(probability.is_finite() && (0.0..=1.0).contains(&probability));
    assert_eq!(head.spam_probability(&vec![0.0; 768]), None);
    assert_eq!(head.spam_probability(&[1.0; 31]), None);
}
