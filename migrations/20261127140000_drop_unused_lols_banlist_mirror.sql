-- LOLS-зеркало не использовалось и больше не читается приложением.
-- Ограничиваем ожидание, чтобы активный snapshot не удерживал deployment.
set local lock_timeout = '30s';
drop table if exists lols_spam_users;
