-- B07: workspace notification channels. Both URLs embed a bot token, so they are sealed with
-- Vault::seal_col (workspaces.notify_*) and `notify_enc_at` marks a sealed row.
ALTER TABLE workspaces ADD COLUMN notify_telegram_webhook text;
ALTER TABLE workspaces ADD COLUMN notify_discord_webhook text;
ALTER TABLE workspaces ADD COLUMN notify_enc_at timestamptz;
