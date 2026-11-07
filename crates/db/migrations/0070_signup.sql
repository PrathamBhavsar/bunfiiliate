ALTER TABLE workspaces ADD COLUMN created_via text NOT NULL DEFAULT 'bootstrap'
    CHECK (created_via IN ('bootstrap', 'self_serve'));
