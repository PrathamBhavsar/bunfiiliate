//! Migration policy tests: upgrading from released snapshots, fresh == upgraded schema,
//! and applied migration files never change. See docs/ops/migrations.md.

use std::collections::BTreeSet;
use std::path::PathBuf;

use sqlx::postgres::PgConnectOptions;
use sqlx::{Connection, PgConnection, PgPool};

const SNAPSHOTS: [(&str, i64); 5] = [
    ("0001", 1),
    ("0018", 18),
    ("0040", 40),
    ("0050", 50),
    ("0077", 77),
];

fn dir(sub: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(sub)
}

/// Load a snapshot on a throwaway connection (the dump sets a session-level search_path).
async fn load_snapshot(pool: &PgPool, name: &str) {
    let sql = std::fs::read_to_string(dir(&format!("tests/snapshots/{name}.sql"))).unwrap();
    let opts: PgConnectOptions = (*pool.connect_options()).clone();
    let mut conn = PgConnection::connect_with(&opts).await.unwrap();
    sqlx::raw_sql(&sql).execute(&mut conn).await.unwrap();
    conn.close().await.unwrap();
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Normalised catalog description: columns, indexes, constraints.
async fn schema(pool: &PgPool) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let q = [
        "SELECT 'col ' || table_name || '.' || column_name || ' ' || data_type || ' ' || udt_name \
         || ' null=' || is_nullable || ' def=' || coalesce(column_default, '') \
         FROM information_schema.columns WHERE table_schema = 'public' \
         AND table_name <> '_sqlx_migrations'",
        "SELECT 'idx ' || tablename || ' ' || indexdef FROM pg_indexes \
         WHERE schemaname = 'public' AND tablename <> '_sqlx_migrations'",
        "SELECT 'con ' || c.conrelid::regclass::text || ' ' || c.conname || ' ' || pg_get_constraintdef(c.oid) \
         FROM pg_constraint c JOIN pg_namespace n ON n.oid = c.connamespace \
         WHERE n.nspname = 'public' AND c.conrelid::regclass::text <> '_sqlx_migrations'",
    ];
    for sql in q {
        let rows: Vec<String> = sqlx::query_scalar(sql).fetch_all(pool).await.unwrap();
        out.extend(rows);
    }
    out
}

#[sqlx::test(migrations = false)]
async fn upgrade_from_each_snapshot_reaches_head(pool: PgPool) {
    // sqlx::test hands out one fresh DB per test; run each snapshot in turn by resetting.
    for (name, version) in SNAPSHOTS {
        sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
            .execute(&pool)
            .await
            .unwrap();
        load_snapshot(&pool, name).await;
        let before = (
            count(&pool, "workspaces").await,
            count(&pool, "users").await,
        );
        assert_eq!(before, (1, 1), "snapshot {name} seed rows");
        assert_eq!(bf_db::applied_head(&pool).await.unwrap(), version);

        bf_db::migrate(&pool)
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));

        assert_eq!(
            bf_db::applied_head(&pool).await.unwrap(),
            bf_db::embedded_head()
        );
        let after = (
            count(&pool, "workspaces").await,
            count(&pool, "users").await,
        );
        assert_eq!(before, after, "seed rows preserved from {name}");
    }
}

#[sqlx::test(migrations = false)]
async fn fresh_install_schema_equals_upgraded_schema(pool: PgPool) {
    load_snapshot(&pool, "0001").await;
    bf_db::migrate(&pool).await.unwrap();
    let upgraded = schema(&pool).await;

    sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .execute(&pool)
        .await
        .unwrap();
    bf_db::migrate(&pool).await.unwrap();
    let fresh = schema(&pool).await;

    let only_fresh: Vec<_> = fresh.difference(&upgraded).collect();
    let only_upgraded: Vec<_> = upgraded.difference(&fresh).collect();
    assert!(
        only_fresh.is_empty() && only_upgraded.is_empty(),
        "schema drift\nonly fresh: {only_fresh:#?}\nonly upgraded: {only_upgraded:#?}"
    );
}

#[test]
fn applied_migration_checksum_unchanged() {
    let m = sqlx::migrate!("./migrations");
    let actual: Vec<String> = m
        .iter()
        .map(|m| {
            let hex: String = m.checksum.iter().map(|b| format!("{b:02x}")).collect();
            format!("{:04} {hex}", m.version)
        })
        .collect();
    let path = dir("tests/checksums.txt");
    if std::env::var_os("UPDATE_CHECKSUMS").is_some() {
        std::fs::write(&path, actual.join("\n") + "\n").unwrap();
    }
    let recorded: Vec<String> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        recorded, actual,
        "migration files changed or added. Never edit an applied migration; for a new one, \
         run UPDATE_CHECKSUMS=1 cargo test -p bf-db --test upgrade"
    );
}

/// C20: click rows present before 0080 survive the swap to a partitioned table, month by month.
#[sqlx::test(migrations = false)]
async fn migration_preserves_existing_clicks(pool: PgPool) {
    load_snapshot(&pool, "0050").await;
    sqlx::raw_sql(
        "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret) \
           SELECT '00000000-0000-0000-0000-0000000000a1', id, 'p', 'USD', 'pk-up', 's' FROM workspaces; \
         INSERT INTO affiliates (id, workspace_id, user_id) \
           SELECT '00000000-0000-0000-0000-0000000000b1', workspace_id, id FROM users; \
         INSERT INTO clicks (id, program_id, affiliate_id, code, visitor_id, ts) \
           SELECT gen_random_uuid(), '00000000-0000-0000-0000-0000000000a1', \
                  '00000000-0000-0000-0000-0000000000b1', 'c', 'v' || g, \
                  timestamptz '2025-01-31 23:59:59+00' + (g % 3) * interval '1 month' \
                    + (g % 7) * interval '1 second' \
           FROM generate_series(1, 1000) g;",
    )
    .execute(&pool)
    .await
    .unwrap();
    let per_month = "SELECT m || '=' || n FROM (SELECT to_char(ts AT TIME ZONE 'UTC', 'YYYY-MM') AS m, \
                     count(*) AS n FROM clicks GROUP BY 1) t ORDER BY m";
    let before: Vec<String> = sqlx::query_scalar(per_month)
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        before.len(),
        4,
        "boundary seconds straddle months: {before:?}"
    );

    bf_db::migrate(&pool).await.unwrap();

    let after: Vec<String> = sqlx::query_scalar(per_month)
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(count(&pool, "clicks").await, 1000);
    assert_eq!(bf_db::partitions::default_rows(&pool).await.unwrap(), 0);
    let old: Option<String> = sqlx::query_scalar("SELECT to_regclass('clicks_old')::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(old.is_none(), "legacy table dropped");
}
