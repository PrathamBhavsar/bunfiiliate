// expect: pass
async fn list(db: &PgPool) {
    sqlx::query("SELECT id FROM programs WHERE workspace_id = $1").bind(1).fetch_all(db).await;
}
