-- Bumping this revokes every access token issued before (tokens carry `ver`; no `ver` = 0).
ALTER TABLE users ADD COLUMN token_version int NOT NULL DEFAULT 0;
