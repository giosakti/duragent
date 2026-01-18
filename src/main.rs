mod build_info;
mod config;
mod handlers;
mod response;

use axum::routing::get;
use axum::Router;
use clap::Parser;
use config::Config;
use std::net::{IpAddr, SocketAddr};
use tokio::signal;

/// Agnx - A minimal and fast self-hosted runtime for durable and portable AI agents
#[derive(Parser, Debug)]
#[command(version = build_info::VERSION_STRING, about, long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config.yaml")]
    config: String,

    /// Port to listen on (overrides config file)
    #[arg(short, long)]
    port: Option<u16>,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let mut config = Config::load(&args.config)?;

    // CLI port overrides config
    if let Some(port) = args.port {
        config.server.port = port;
    }

    let api_v1 = Router::new().route("/agents", get(handlers::list_agents));

    let app = Router::new()
        .route("/livez", get(handlers::livez))
        .route("/readyz", get(handlers::readyz))
        .route("/version", get(handlers::version))
        .nest("/api/v1", api_v1)
        .route("/example-bad-request", get(handlers::example_bad_request))
        .route("/example-not-found", get(handlers::example_not_found))
        .route(
            "/example-internal-error",
            get(handlers::example_internal_error),
        );

    let ip: IpAddr = config.server.host.parse()?;
    let addr = SocketAddr::new(ip, config.server.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;

    println!("Starting server on {addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    println!("Server stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => println!("\nReceived Ctrl+C, shutting down..."),
        _ = terminate => println!("\nReceived SIGTERM, shutting down..."),
    }
}
