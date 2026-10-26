use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "api", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve,
    Worker,
    Openapi,
    Bootstrap,
    SeedDemo,
}

fn main() {
    bf_rt::init_tracing();
    let cli = Cli::parse();
    let name = match cli.command {
        Command::Serve => "serve",
        Command::Worker => "worker",
        Command::Openapi => "openapi",
        Command::Bootstrap => "bootstrap",
        Command::SeedDemo => "seed-demo",
    };
    println!("{name}: not implemented");
}
