// expect: pass
async fn sweep(db: &PgPool) {
    sqlx::query("DELETE FROM clicks WHERE ts < now()").execute(db).await;
}
