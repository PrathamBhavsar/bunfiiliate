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
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
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
            tracing::info!("worker running");
            tokio::select! {
                () = api::worker::run(pool, mailer) => {}
                _ = tokio::signal::ctrl_c() => {}
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
    let mailer = api::mail::Mailer::from_env()?;
    let worker = with_worker.then(|| tokio::spawn(api::worker::run(pool.clone(), mailer)));
    let state = AppState {
        pool,
        jwt_secret: Arc::from(cfg.jwt_secret),
        secure_cookies: !matches!(std::env::var("COOKIE_SECURE").as_deref(), Ok("0" | "false")),
    };
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", cfg.api_port)).await?;
    tracing::info!(port = cfg.api_port, "api listening");
    axum::serve(listener, app(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    if let Some(w) = worker {
        w.abort();
    }
    Ok(())
}
