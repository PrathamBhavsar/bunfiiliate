-- Failure counters for auth throttles. A06a creates the table; A06b wires `reset_user`
-- (key = user id text) and adds further scopes in its own migration.
CREATE TABLE auth_throttle (
    scope             text NOT NULL CHECK (scope IN ('reset_user')),
    key               text NOT NULL,
    failures          int NOT NULL DEFAULT 0,
    window_started_at timestamptz NOT NULL DEFAULT now(),
    locked_until      timestamptz,
    PRIMARY KEY (scope, key)
);
