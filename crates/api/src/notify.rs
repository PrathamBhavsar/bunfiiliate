//! User notifications: in-app always, mail only when `SMTP_URL` is set.

use std::future::Future;
use std::pin::Pin;

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;
use crate::mail::Mailer;

pub type NotifyFuture<'a> = Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>>;

pub trait Notifier: Send + Sync {
    fn notify<'a>(
        &'a self,
        user_id: Uuid,
        kind: &'a str,
        title: &'a str,
        body: &'a str,
    ) -> NotifyFuture<'a>;
}

/// Row in `notifications`.
pub struct InApp(pub PgPool);

impl Notifier for InApp {
    fn notify<'a>(
        &'a self,
        user_id: Uuid,
        kind: &'a str,
        _: &'a str,
        body: &'a str,
    ) -> NotifyFuture<'a> {
        Box::pin(async move {
            bf_db::auth::insert_notification(&self.0, user_id, kind, body).await?;
            Ok(())
        })
    }
}

/// Email to the user's address.
pub struct MailNotifier {
    pub pool: PgPool,
    pub mailer: Mailer,
}

impl Notifier for MailNotifier {
    fn notify<'a>(
        &'a self,
        user_id: Uuid,
        _: &'a str,
        title: &'a str,
        body: &'a str,
    ) -> NotifyFuture<'a> {
        Box::pin(async move {
            let Some(user) = bf_db::auth::user_by_id(&self.pool, user_id).await? else {
                return Ok(());
            };
            self.mailer
                .send(&user.email, title, body)
                .await
                .map_err(|e| {
                    tracing::warn!(user = %user_id, error = %e, "notification mail failed");
                    AppError::Internal
                })
        })
    }
}

/// Runs every notifier; a failure is logged and does not stop the others.
pub struct Composite(pub Vec<Box<dyn Notifier>>);

impl Notifier for Composite {
    fn notify<'a>(
        &'a self,
        user_id: Uuid,
        kind: &'a str,
        title: &'a str,
        body: &'a str,
    ) -> NotifyFuture<'a> {
        Box::pin(async move {
            let mut first = Ok(());
            for n in &self.0 {
                if let Err(e) = n.notify(user_id, kind, title, body).await {
                    tracing::warn!(user = %user_id, kind, "notifier failed");
                    if first.is_ok() {
                        first = Err(e);
                    }
                }
            }
            first
        })
    }
}

/// Mail notifier, only when `SMTP_URL` is set (default: none).
pub fn mail_from_env(pool: &PgPool) -> Option<MailNotifier> {
    std::env::var_os("SMTP_URL")?;
    match Mailer::from_env() {
        Ok(mailer) => Some(MailNotifier {
            pool: pool.clone(),
            mailer,
        }),
        Err(e) => {
            tracing::warn!(error = %e, "mail notifier disabled");
            None
        }
    }
}

/// In-app plus optional mail.
pub fn from_env(pool: &PgPool) -> Composite {
    let mut v: Vec<Box<dyn Notifier>> = vec![Box::new(InApp(pool.clone()))];
    if let Some(m) = mail_from_env(pool) {
        v.push(Box::new(m));
    }
    Composite(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fail;
    impl Notifier for Fail {
        fn notify<'a>(&'a self, _: Uuid, _: &'a str, _: &'a str, _: &'a str) -> NotifyFuture<'a> {
            Box::pin(async { Err(AppError::Internal) })
        }
    }
    struct Count(Arc<AtomicUsize>);
    impl Notifier for Count {
        fn notify<'a>(&'a self, _: Uuid, _: &'a str, _: &'a str, _: &'a str) -> NotifyFuture<'a> {
            Box::pin(async {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn composite_continues_on_error() {
        let n = Arc::new(AtomicUsize::new(0));
        let c = Composite(vec![Box::new(Fail), Box::new(Count(n.clone()))]);
        assert!(c.notify(Uuid::nil(), "k", "t", "b").await.is_err());
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn mail_notifier_off_without_smtp_url() {
        // SMTP_URL is not set in the test environment.
        if std::env::var_os("SMTP_URL").is_none() {
            let pool = sqlx::PgPool::connect_lazy("postgres://x@localhost/x").unwrap();
            assert!(mail_from_env(&pool).is_none());
        }
    }
}
