// expect: fail
async fn fine(db: &PgPool) {
    sqlx::query("SELECT id FROM programs WHERE workspace_id = $1").execute(db).await;
}
