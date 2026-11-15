# Формат отчёта тестового случая

Отчёт содержит только метаданные. Полный вход, URL, Telegram ID, DSN и auth
значения в обычные логи не включаются.

```json
{
  "case_id": "synthetic-url-property",
  "invariant_id": "URL-01",
  "validity_class": "v2",
  "origin": "property_generated",
  "seed": 72623859790382856,
  "generated": 256,
  "accepted": 256,
  "rejected": 0
}
```

`validity_class` — `v1`, `v2` или `v3`; `origin` — `public_synthetic_corpus`,
`property_generated` или `minimized_replay`. Свойства генератора обязаны иметь
целочисленный seed. Для текущих тестов используются только V2 synthetic cases;
реальные обновления Telegram для V1 пока не нужны.
