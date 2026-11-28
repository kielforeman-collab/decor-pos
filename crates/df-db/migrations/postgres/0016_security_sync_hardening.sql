-- decor-pos Security & Sync Hardening — PostgreSQL HQ schema additions.
-- Same schema changes as migrations/sqlite/0016_security_sync_hardening.sql,
-- in the TEXT encoding both dialects use for ids/timestamps (see df-db lib.rs:
-- the `Any` driver cannot decode uuid / timestamptz columns).
--   * sync_outbox dead-letter / quarantine columns (closes F8 + F10 retries).
--   * sync_conflicts operator resolution columns (`resolved_at`,
--     `resolution_note`) for HQ conflict triage.
--   * service_tokens table for long-lived, revocable, store-scoped `dfs_`
--     bearer tokens (SHA-256 hex hash only; never raw secret).
-- No f32/f64 anywhere. No COLORx code. No PAN/CVV columns.

ALTER TABLE sync_outbox ADD COLUMN quarantined_at TEXT;
ALTER TABLE sync_outbox ADD COLUMN quarantine_reason TEXT;

CREATE INDEX idx_outbox_quarantined ON sync_outbox (store_id, quarantined_at, id);

ALTER TABLE sync_conflicts ADD COLUMN resolved_at TEXT;
ALTER TABLE sync_conflicts ADD COLUMN resolution_note TEXT;

CREATE TABLE service_tokens (
    id              TEXT PRIMARY KEY,
    org_id          TEXT NOT NULL,
    store_id        TEXT NOT NULL,
    label           TEXT NOT NULL,
    token_hash      TEXT NOT NULL UNIQUE,
    scopes          TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    expires_at      TEXT,
    revoked_at      TEXT,
    last_used_at    TEXT
);

CREATE INDEX idx_service_tokens_org_store ON service_tokens (org_id, store_id, created_at);
