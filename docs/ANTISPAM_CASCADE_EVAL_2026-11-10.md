# Replay локального каскада, 10 ноября 2026

Private export использует последнее ручное решение, максимум десять сообщений
до метки, исключает post-label edits. Оценка имеет единицу **автор**, а не
независимо проверенное сообщение. Raw dataset/token/model vocabulary не опубликованы.

| Инстанс | Сообщения | Spam авторы | Normal авторы | Default local review spam | Normal review | Только прошлые метки |
|---|---:|---:|---:|---:|---:|---:|
| NedoNews | 233 | 168 | 18 | 21 | 0 | 7 |
| PVO | 110 | 60 | 2 | 7 | 0 | 2 |

Default: template similarity 0.85, normal-veto 0.80, два независимых автора,
linear threshold 0.99, минимум 32 символа/5 tokens. NedoNews использует прежнюю
модель alt-word-char-v2-2026-10-03; PVO проверен template-only.

Проверены 30 комбинаций similarity (0.75/0.85/0.95), числа независимых авторов
(1/2) и linear threshold (0.90/0.95/0.98/0.99/0.999). Более мягкий вариант
давал 42/168 и 15/60 spam-авторов при нуле наблюдавшихся normal review.
Он не выбран: 18 и особенно 2 normal автора недостаточны для такой калибровки.

При default на NedoNews 20 сообщений получили template decision и одно —
linear; остальные abstain/short. В PVO 18 сообщений получили template decision.
Авторские итоги в таблице агрегируют несколько сообщений одного автора.

Это потенциальная review coverage, **не** end-to-end recall и не измеренная
экономия LLM: export не воспроизводит все live gates, часть совпадений уже
покрывал прежний deterministic fast path, а существующая модель могла видеть
исторические данные. Temporal-вариант тоже не восстанавливает отозванную
историю: это replay текущих меток с временным фильтром.

## Offline candidate

NedoNews: grouped train/validation/test = 158/31/44 сообщения; из шести C выбран
0.1. В test 36 spam и 5 normal авторов, на пороге 0.99 новый candidate не
выделил ни одного. Temporal train/test = 132/39, но train не содержит обоих
классов; полноценная временная оценка невозможна. Candidate не заменяет
рабочую модель. Python/Rust inference совпали на всех 233 сообщениях;
максимальная разница probability — 2.22e-16.

PVO: недостаточно классов после группового split; report=insufficient_split,
модель не экспортирована. Молчаливого fallback к random message split нет.

Следующая полезная разметка — независимые normal сообщения и новые кампании,
а не увеличение веса уже известных копий. Фоновое обучение/автопубликация
модели не включены; DB-backed templates обновляются текущими ручными решениями.

## Воспроизводимость

- nedonews dataset SHA-256: c9c5e18aa1e92527e41a8f9b4860976282512616ad311f0ed160b84cd0cc86f4
- nedonews model SHA-256: b90d9ebae22abde31238269e595164e41025ddf4e0d1c40d5ba1810f387b2d63
- pvo dataset SHA-256: de9021067ebea5e0526306077e015d1a0071c4af0d412cd38d9f152ddc155126
- pvo model SHA-256: template-only

[Команды и границы проверки](LOCAL_ANTISPAM_CASCADE.md).
