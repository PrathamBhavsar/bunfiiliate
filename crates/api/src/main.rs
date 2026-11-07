use std::error::Error;
use std::sync::Arc;

use api::{AppState, app, bootstrap::bootstrap, routes::ApiDoc};
use bf_rt::Config;
use clap::{Parser, Subcommand};
use utoipa::OpenApi;

#[derive(Parser)]
#[command(name = "api", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the HTTP API.
    Serve {
        /// Also run the background worker in this process.
        #[arg(long)]
        with_worker: bool,
    },
    /// Run the background worker (ingest processor, auto-approve).
    Worker,
    /// Print the OpenAPI document.
    Openapi,
    /// Create the first workspace and its super_admin.
    Bootstrap {
        #[arg(long, env = "BF_ADMIN_EMAIL")]
        email: String,
        #[arg(long, env = "BF_ADMIN_PASSWORD", hide_env_values = true)]
        password: String,
        #[arg(long, env = "BF_WORKSPACE_NAME", default_value = "Default")]
        workspace: String,
    },
    SeedDemo,
    /// Manage secrets at rest.
    Secrets {
        #[command(subcommand)]
        action: SecretsCmd,
    },
}

#[derive(Subcommand)]
enum SecretsCmd {
    /// Seal plaintext secrets and re-wrap those sealed under an older master key.
    Reencrypt {
        /// Count only; change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Exit 1 if any plaintext or old-key secret remains.
    Check,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    api::ssrf::init_from_env();
    bf_rt::init_tracing();
    match Cli::parse().command {
        Command::Serve { with_worker } => serve(with_worker).await?,
        Command::Openapi => println!("{}", ApiDoc::openapi().to_pretty_json()?),
        Command::Bootstrap {
            email,
            password,
            workspace,
        } => {
            let pool = bf_db::connect(&Config::from_env()?.database_url).await?;
            bf_db::migrate(&pool).await?;
            let id = bootstrap(&pool, &email, &password, &workspace)
                .await
                .map_err(|e| format!("bootstrap failed: {e:?}"))?;
            println!("created super_admin {id}");
        }
        Command::Worker => {
            let pool = bf_db::connect(&Config::from_env()?.database_url).await?;
            bf_db::migrate(&pool).await?;
            let mailer = api::mail::Mailer::from_env()?;
            let vault = Arc::new(api::secrets::load_vault()?);
            tracing::info!("worker running");
            tokio::spawn(bf_rt::metrics::serve(
                "127.0.0.1:9101",
                api::metrics::render,
            ));
            tokio::select! {
                () = api::worker::run(pool, mailer, vault) => {}
                _ = tokio::signal::ctrl_c() => {}
            }
        }
        Command::Secrets { action } => {
            let ring = bf_core::secrets::KeyRing::from_env()?;
            let pool = bf_db::connect(&Config::from_env()?.database_url).await?;
            let (report, clean_needed) = match action {
                SecretsCmd::Reencrypt { dry_run } => {
                    (api::secrets::reencrypt(&pool, &ring, dry_run).await?, false)
                }
                SecretsCmd::Check => (api::secrets::check(&pool, &ring).await?, true),
            };
            println!(
                "encrypted={} rewrapped={} failed={}",
                report.encrypted, report.rewrapped, report.failed
            );
            if report.failed > 0 || (clean_needed && report.pending() > 0) {
                std::process::exit(1);
            }
        }
        Command::SeedDemo => println!("seed-demo: not implemented"),
    }
    Ok(())
}

async fn serve(with_worker: bool) -> Result<(), Box<dyn Error>> {
    let cfg = Config::from_env()?;
    let pool = bf_db::connect(&cfg.database_url).await?;
    bf_db::migrate(&pool).await?;
    api::worker::partition_maintenance(&pool).await?;
    let mailer = api::mail::Mailer::from_env()?;
    let vault = Arc::new(api::secrets::load_vault()?);
    let metrics = tokio::spawn(bf_rt::metrics::serve(
        "127.0.0.1:9101",
        api::metrics::render,
    ));
    let worker =
        with_worker.then(|| tokio::spawn(api::worker::run(pool.clone(), mailer, vault.clone())));
    let state = AppState {
        pool,
        jwt_keys: Arc::new(api::auth::JwtKeys::parse(
            cfg.jwt_secrets.as_deref(),
            cfg.jwt_secret.as_deref(),
        )?),
        version_cache: Default::default(),
        vault,
        secure_cookies: !matches!(std::env::var("COOKIE_SECURE").as_deref(), Ok("0" | "false")),
        limiter: Default::default(),
        tracker_url: Default::default(),
    };
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", cfg.api_port)).await?;
    tracing::info!(port = cfg.api_port, "api listening");
    axum::serve(listener, app(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    metrics.abort();
    if let Some(w) = worker {
        w.abort();
    }
    Ok(())
}
