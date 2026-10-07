# Сравнение EmbeddingGemma 2 и RuBERT для spam scoring

Дата: 2026-10-07. Результаты получены на CPU для замороженных Q4 ONNX-векторов;
обучалась только линейная логистическая голова.

## Протокол

- Gemma 2: pinned Q4 export `onnx-community/embeddinggemma-2-ONNX`, revision
  `daa72c51243991dfcaf9f9137d2c573d8f7790c0`; проверены MRL-срезы 768, 512,
  256 и 128. Выбранный для бота размер — **512**.
- RuBERT baseline: 312-мерный ONNX export
  [`fissium/rubert-tiny2`](https://huggingface.co/fissium/rubert-tiny2), который
  заявлен как ONNX-версия [`cointegrated/rubert-tiny2`](https://huggingface.co/cointegrated/rubert-tiny2).
- Текст классифицировался с Google prefix
  `task: classification | query: `; после MRL-среза каждый вектор повторно
  нормализован.
- Публичный ALT corpus зафиксирован на revision
  `7b435036d572a8139ab2e679532eca336b6d90dc`. После исключения конфликтующих
  template groups и дедупликации осталось 32 546 строк: 19 535 train, 6 509
  validation и 6 502 test. Группы шаблонов разделены между split-ами.
- Голова обучена на train. Threshold выбран только на validation: не более двух
  false positives и probability floor 0.90. Test использовался только для
  итогового отчёта.

Google описывает EmbeddingGemma 2 как мультимодальную модель с общим 768-мерным
пространством и Matryoshka-размерами 768/512/256/128; срезы нормализуются заново.
Q4 ONNX-файл здесь — community export, закреплённый на точном revision, а не
непосредственно Google runtime.

## Результаты классификатора

| Энкодер | Размер | Validation recall / FP | Test recall / FP | Test ROC-AUC |
|---|---:|---:|---:|---:|
| RuBERT Tiny2 | 312 | 89.89% / 2 | 91.39% / 0 | 0.99928 |
| EmbeddingGemma 2 Q4 | 768 | 92.39% / 0 | 92.86% / 0 | 0.99984 |
| **EmbeddingGemma 2 Q4** | **512** | **92.39% / 0** | **92.74% / 0** | **0.99982** |
| EmbeddingGemma 2 Q4 | 256 | 91.86% / 0 | 92.11% / 0 | 0.99973 |
| EmbeddingGemma 2 Q4 | 128 | 87.36% / 1 | 88.23% / 0 | 0.99885 |

512 выбран по заранее заданному правилу: минимальный размер в пределах 0.5
процентного пункта validation recall от лучшего Gemma 2 варианта. Он даёт тот же
validation recall, что 768, при векторе на треть меньше. 256 отстаёт на 0.530
пункта и не проходит правило. На test у 512 было 0 false positives среди 3 307
ham-строк; односторонняя верхняя 95% граница FPR при допущении независимости —
около 0.091%.

## Similarity с ранее помеченным спамом

Старые фиксированные RuBERT cosine-пороги 0.78/0.88 на этом ALT split дали
184/6 false positives на validation и 182/2 на test соответственно. Для Gemma 2
512 validation выбрал пороги 0.898344 (supporting, 2 FP) и 0.901562 (strong,
0 FP). На test supporting дал 1 FP и 89.33% recall; strong — 0 FP и 88.42%
recall. Пороги не включены по умолчанию: соседство с 9 621 train spam-текстами не
эквивалентно пользовательскому корпусу подтверждённого спама в production.

Raw cosine-пороги нового энкодера в коде включаются только совместной явной
настройкой обоих порогов; в этом rollout они остаются выключены. Новая 512d
голова задаётся профилем и заменяет прежний 768d head в runtime. Историческая
оценка старой головы сообщала 95.50% test recall при 0 FP, но её dataset/split
не совпадает с зафиксированным здесь протоколом, поэтому прямое сравнение
точности некорректно. Старые таблицы и модель остаются только для rollback.

## Границы результатов

ALT — публичный текстовый корпус, не сообщения и не аватары NedoBot. Split
изолирует нормализованные template groups, но не авторов, кампании или время;
публичный корпус и похожий split уже использовались в прежней модельной работе.
Поэтому цифры нельзя считать production FPR. В corpus нет разметки категорий
спама и изображений: сохранён только бинарный текстовый candidate head; category
heads пусты, image spam classifier не обучался. TF-IDF и reputation heads не
переобучались: для них нужна соответствующая production разметка и признаки.

128 текстовых примеров дали cosine similarity не ниже 0.999999999997 между
Python ONNX baseline и локальным Transformers.js service на выбранных первых
512 компонентах. Image endpoint прошёл smoke на синтетическом изображении, но
его качество на размеченных аватарах не измерялось.

Кандидат обученной головы: `models/embeddinggemma2_q4_classification_512_alt_2026-10-07.json`.
В rollout он включается только как supporting review-risk signal, без
автоматической модерации. Метрики получены на открытом ALT text split; качество
на production данных ещё предстоит оценить. Старые головы и векторы сохранены
для rollback на время backfill и production validation.

Источники: [Google model card](https://ai.google.dev/gemma/docs/embeddinggemma/model_card_2),
[Google inference guide](https://ai.google.dev/gemma/docs/embeddinggemma/inference-embeddinggemma-with-sentence-transformers),
[pinned Q4 ONNX export](https://huggingface.co/onnx-community/embeddinggemma-2-ONNX/tree/daa72c51243991dfcaf9f9137d2c573d8f7790c0),
[ALT corpus](https://huggingface.co/datasets/alt-gnome/telegram-spam-20251030),
[RuBERT Tiny2](https://huggingface.co/cointegrated/rubert-tiny2).
