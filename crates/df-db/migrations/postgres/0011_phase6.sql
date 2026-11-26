-- decor-pos Phase 6 (v1.0) — PostgreSQL HQ schema additions (spec §4 scheduling_plus, §5.5, §7 Phase 6 row).
-- Same tables as migrations/sqlite/0011_phase6.sql, adapted for PostgreSQL:
--   TEXT ids/timestamps, BOOLEAN. Template wall-clock times are TEXT
--   "HH:MM" in both dialects (the repos cross them as text, like
--   dates elsewhere in df-db).
--   No f32/f64 anywhere.

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
    is_active       BOOLEAN NOT NULL DEFAULT TRUE
);

CREATE INDEX idx_shift_templates_store ON shift_templates (store_id, weekday);

CREATE TABLE staff_shifts (
    id              TEXT PRIMARY KEY,
    org_id          TEXT NOT NULL,
    store_id        TEXT NOT NULL,
    template_id     TEXT,
    staff_id        TEXT NOT NULL,
    shift_date      TEXT NOT NULL,
    starts_at       TEXT NOT NULL,
    ends_at         TEXT NOT NULL,
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
    clock_in        TEXT NOT NULL,
    clock_out       TEXT,        -- NULL while open
    source          TEXT NOT NULL DEFAULT 'counter'
);

CREATE INDEX idx_timeclock_store_staff ON timeclock_entries (store_id, staff_id, clock_in);
