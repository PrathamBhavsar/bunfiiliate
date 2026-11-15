-- C20: `clicks` partitioned by month (UTC bounds). PK becomes (id, ts): a partitioned
-- table's unique key must contain the partition key. See docs/ops/partitioning.md.
-- No FK points at clicks(id) (customers.first_click_id is a plain uuid).

-- Index names do not follow RENAME TABLE: free them for the new table first.
ALTER TABLE clicks RENAME TO clicks_old;
ALTER INDEX clicks_pkey RENAME TO clicks_old_pkey;
ALTER INDEX clicks_ts_brin RENAME TO clicks_old_ts_brin;
ALTER INDEX clicks_affiliate_idx RENAME TO clicks_old_affiliate_idx;
ALTER INDEX clicks_visitor_idx RENAME TO clicks_old_visitor_idx;
ALTER TABLE clicks_old RENAME CONSTRAINT clicks_program_id_fkey TO clicks_old_program_id_fkey;
ALTER TABLE clicks_old RENAME CONSTRAINT clicks_affiliate_id_fkey TO clicks_old_affiliate_id_fkey;

CREATE TABLE clicks (
    id             uuid NOT NULL,
    program_id     uuid NOT NULL REFERENCES programs (id),
    affiliate_id   uuid NOT NULL REFERENCES affiliates (id),
    code           text NOT NULL,
    visitor_id     text NOT NULL,
    ts             timestamptz NOT NULL,
    ip_hash        bytea,
    country        char(2),
    device         text,
    os             text,
    browser        text,
    referrer_host  text,
    landing_path   text,
    fraud_reason   text,
    PRIMARY KEY (id, ts)
) PARTITION BY RANGE (ts);
CREATE INDEX clicks_ts_brin ON clicks USING brin (ts);
CREATE INDEX clicks_affiliate_idx ON clicks (affiliate_id);
CREATE INDEX clicks_visitor_idx ON clicks (program_id, visitor_id);

-- Catch-all. Rows here mean a month partition is missing: see bf_clicks_default_rows().
CREATE TABLE clicks_default PARTITION OF clicks DEFAULT;

-- Highest day whose clicks retention has dropped. Rollups never rebuild days before it
-- (the clicks they would count are gone). Single row, created by the first drop.
CREATE TABLE clicks_retention (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    floor_day date NOT NULL
);

-- Create (or no-op) the partition for the UTC month containing `month`.
-- Rows already in clicks_default for that month are moved in first: attaching over them
-- would fail. Serialised by an advisory lock; returns true if a partition was created.
CREATE FUNCTION bf_ensure_clicks_partition(month date) RETURNS boolean
LANGUAGE plpgsql AS $$
DECLARE
    first_day timestamp := date_trunc('month', month::timestamp);
    lo        timestamptz := first_day AT TIME ZONE 'UTC';
    hi        timestamptz := (first_day + interval '1 month') AT TIME ZONE 'UTC';
    pname     text := 'clicks_y' || to_char(first_day, 'YYYY') || 'm' || to_char(first_day, 'MM');
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext('bf_clicks_partitions'));
    IF to_regclass(pname) IS NOT NULL THEN
        RETURN false;
    END IF;
    -- Block inserts into the default partition while its rows move and the attach validates.
    LOCK TABLE clicks_default IN SHARE ROW EXCLUSIVE MODE;
    EXECUTE format('CREATE TABLE %I (LIKE clicks INCLUDING DEFAULTS INCLUDING INDEXES)', pname);
    EXECUTE format(
        'WITH moved AS (DELETE FROM clicks_default WHERE ts >= %L AND ts < %L RETURNING *) '
        'INSERT INTO %I SELECT * FROM moved',
        to_char(lo AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00"'),
        to_char(hi AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00"'),
        pname);
    EXECUTE format(
        'ALTER TABLE clicks ATTACH PARTITION %I FOR VALUES FROM (%L) TO (%L)',
        pname,
        to_char(lo AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00"'),
        to_char(hi AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00"'));
    RETURN true;
END $$;

-- Rows stuck in the catch-all partition (should be 0). Exported as bf_clicks_default_rows.
CREATE FUNCTION bf_clicks_default_rows() RETURNS bigint
LANGUAGE sql STABLE AS $$ SELECT count(*) FROM clicks_default $$;

-- Partitions for every month that has data, plus the current month and the next three.
-- Copy month by month (bounded batches), oldest first.
DO $$
DECLARE
    m timestamp;
    lo text;
    hi text;
BEGIN
    FOR m IN
        SELECT DISTINCT date_trunc('month', ts AT TIME ZONE 'UTC') FROM clicks_old ORDER BY 1
    LOOP
        PERFORM bf_ensure_clicks_partition(m::date);
        lo := to_char(m, 'YYYY-MM-DD"T"HH24:MI:SS"+00"');
        hi := to_char(m + interval '1 month', 'YYYY-MM-DD"T"HH24:MI:SS"+00"');
        INSERT INTO clicks
            SELECT * FROM clicks_old WHERE ts >= lo::timestamptz AND ts < hi::timestamptz;
    END LOOP;
    FOR i IN 0..3 LOOP
        PERFORM bf_ensure_clicks_partition(
            ((date_trunc('month', now() AT TIME ZONE 'UTC') + make_interval(months => i)))::date);
    END LOOP;
END $$;

-- allow-destructive: clicks_old is the renamed pre-partition table, all rows were copied above
DROP TABLE clicks_old;
