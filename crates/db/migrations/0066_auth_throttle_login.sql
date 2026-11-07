-- A06b: login backoff scope (key = lower(email) || '|' || ip) and a lock counter so repeated
-- locks double.
ALTER TABLE auth_throttle DROP CONSTRAINT auth_throttle_scope_check;
ALTER TABLE auth_throttle ADD CONSTRAINT auth_throttle_scope_check
    CHECK (scope IN ('reset_user', 'login_ip'));
ALTER TABLE auth_throttle ADD COLUMN lock_count int NOT NULL DEFAULT 0;
