// expect: fail
// Known blind spot: sqlx::query! macros fail closed.
async fn list(db: &PgPool) {
    sqlx::query!("SELECT id FROM programs").fetch_all(db).await;
}
