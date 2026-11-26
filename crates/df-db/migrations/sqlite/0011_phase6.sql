-- decor-pos Phase 6 (v1.0) — SQLite edge schema additions (spec §4 scheduling_plus, §5.5, §7 Phase 6 row).
-- Same tables as migrations/postgres/0011_phase6.sql, adapted for SQLite:
--   UUID -> TEXT, TIMESTAMPTZ -> TEXT (RFC 3339), booleans as INTEGER 0/1.
--   Template wall-clock times are TEXT "HH:MM" in both dialects.
--   No f32/f64 anywhere. staff_id references users(id) conceptually;
--   like other HQ reference data the edge schema deliberately omits
--   the FOREIGN KEY (the edge must keep working when that data has
--   not synced down yet — see df-db lib.rs).

CREATE TABLE shift_templates (
    id              TEXT PRIMARY KEY,
    org_id          TEXT NOT NULL,
    store_id        TEXT NOT NULL,
    name            TEXT NOT NULL,
    weekday         INTEGER NOT NULL,   -- 0=Sunday..6=Saturday
    starts_at       TEXT NOT NULL,      -- "HH:MM"
    ends_at         TEXT NOT NULL,      -- "HH:MM", after starts_at
    role            TEXT,
    staff_id        TEXT,
    required_count  INTEGER NOT NULL DEFAULT 1,
    is_active       INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX idx_shift_templates_store ON shift_templates (store_id, weekday);

CREATE TABLE staff_shifts (
    id              TEXT PRIMARY KEY,
    org_id          TEXT NOT NULL,
    store_id        TEXT NOT NULL,
    template_id     TEXT,
    staff_id        TEXT NOT NULL,
    shift_date      TEXT NOT NULL,      -- YYYY-MM-DD
    starts_at       TEXT NOT NULL,      -- RFC 3339
    ends_at         TEXT NOT NULL,      -- RFC 3339, after starts_at
    status          TEXT NOT NULL DEFAULT 'scheduled',  -- scheduled|confirmed|cancelled
    notes           TEXT
);

CREATE INDEX idx_staff_shifts_store_date ON staff_shifts (store_id, shift_date);
CREATE INDEX idx_staff_shifts_staff ON staff_shifts (staff_id, starts_at);

CREATE TABLE timeclock_entries (
    id              TEXT PRIMARY KEY,
    org_id          TEXT NOT NULL,
    store_id        TEXT NOT NULL,
    staff_id        TEXT NOT NULL,
    clock_in        TEXT NOT NULL,      -- RFC 3339
    clock_out       TEXT,               -- NULL while open
    source          TEXT NOT NULL DEFAULT 'counter'
);

CREATE INDEX idx_timeclock_store_staff ON timeclock_entries (store_id, staff_id, clock_in);
