-- decor-pos Phase 7 (v1.1) — PostgreSQL HQ schema additions (auth core).
-- Same tables as migrations/sqlite/0012_phase7.sql, adapted for
-- PostgreSQL: UUID, TIMESTAMPTZ. No f32/f64 anywhere. No COLORx
-- code (spec §9: deferred post-v1.0). No PAN/CVV columns (spec §8
-- guardrail 3).
--
-- Findings closed (in part — see docs/auth.md): F1/F2/F4 get their
-- primitives here (sessions, portal login link); HTTP enforcement
-- (middleware + route wiring) is the sibling workers' job, and F3
-- (RBAC enforcement) is theirs too.
--
-- roles: `users.role_id` (0001) is a UUID with no table behind it in
-- Phases 0-6 and no code ever wrote one. Phase 7 seeds the table
-- with the fixed well-known ids defined in `df-auth::role_id`
-- (one per `Role` variant) and resolves them in Rust. Deliberately
-- NO foreign key is added on `users.role_id`: a deployment that
-- hand-inserted a user with an arbitrary role_id must keep
-- migrating, and the app rejects an unmapped id at read time.
--
-- users.portal_customer_id: the portal login link (closes F4's "no
-- separate portal login"). It lives on the user, not on the
-- customer: one portal user is scoped to exactly one contractor
-- customer, while a customer row predates any portal user and must
-- not name one. NULL for every staff user. PostgreSQL enforces the
-- reference to customers(id); SQLite deliberately omits it (see the
-- edge FK convention in `df-db` lib.rs).
--
-- sessions: one row per login. Only the SHA-256 hex of the opaque
-- `dfp_` token is stored (`token_hash` UNIQUE — the uniqueness
-- constraint is the token_hash index); the raw token is returned to
-- the caller once at login and is never persisted or logged.
-- `role` is the user's role cached as text at login; `customer_id`
-- is the portal scope (NULL for staff). `revoked_at` NULL while
-- live; `last_used_at` is refreshed on every successful validation.

CREATE TABLE roles (
    id              UUID PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE
);

INSERT INTO roles (id, name) VALUES
    ('10000000-0000-4000-8000-000000000001', 'Cashier'),
    ('10000000-0000-4000-8000-000000000002', 'SalesAssociate'),
    ('10000000-0000-4000-8000-000000000003', 'Warehouse'),
    ('10000000-0000-4000-8000-000000000004', 'Driver'),
    ('10000000-0000-4000-8000-000000000005', 'Manager'),
    ('10000000-0000-4000-8000-000000000006', 'Admin'),
    ('10000000-0000-4000-8000-000000000007', 'PortalContractor');

ALTER TABLE users ADD COLUMN portal_customer_id UUID REFERENCES customers(id);

CREATE TABLE sessions (
    id              UUID PRIMARY KEY,
    user_id         UUID NOT NULL REFERENCES users(id),
    token_hash      TEXT NOT NULL UNIQUE,
    role            TEXT NOT NULL,
    customer_id     UUID,
    created_at      TIMESTAMPTZ NOT NULL,
    expires_at      TIMESTAMPTZ NOT NULL,
    revoked_at      TIMESTAMPTZ,
    last_used_at    TIMESTAMPTZ NOT NULL
);

CREATE INDEX idx_sessions_user_id ON sessions (user_id, created_at);
