// expect: fail
// Known blind spot: format!-built SQL cannot be read, so it fails closed.
async fn list(db: &PgPool, cols: &str) {
    sqlx::query_as(&format!("SELECT {cols} FROM programs")).fetch_all(db).await;
}
