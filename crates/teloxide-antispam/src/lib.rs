//! Общее антиспам-ядро инстансов NedoBot.
//!
//! Чистые функции без Telegram/SQL/LLM: текстовые нормализации, скоринг
//! профилей и сообщений, внешние репутационные клиенты. Конвейер
//! (загрузка snapshots, Bot API, LLM, персист) остаётся в боте, который
//! маппит свои DB-строки на типы этого крейта.

pub mod assessment;
pub mod external;
pub mod policy;
pub mod scoring;
pub mod signals;
pub mod text;
