// expect: fail
async fn list(db: &PgPool) {
    sqlx::query("SELECT id FROM programs WHERE id = $1").bind(1).fetch_all(db).await;
}
