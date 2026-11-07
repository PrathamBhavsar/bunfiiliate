// expect: fail
async fn list(db: &PgPool) {
    sqlx::query(
        "SELECT a.id \
         FROM affiliates a \
         WHERE a.status = 'approved'",
    )
    .fetch_all(db)
    .await;
}
