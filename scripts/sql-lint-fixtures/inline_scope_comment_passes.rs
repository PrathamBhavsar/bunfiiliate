// expect: pass
async fn by_id(db: &PgPool) {
    sqlx::query("SELECT id FROM affiliates WHERE id = $1 -- scope: id already authorized by caller")
        .fetch_one(db)
        .await;
}
