use clap::Parser;
use ctp_gateway::{app, state_from_env_with};
use std::path::{Path, PathBuf};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "ctp-gateway",
    about = "Anthropic Messages facade for Claude Portable"
)]
struct Args {
    /// Порт локального шлюза.
    #[arg(long, default_value_t = 17889)]
    port: u16,

    /// Адрес; по умолчанию только петля.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Файл журнала; без него — stderr.
    #[arg(long)]
    log: Option<PathBuf>,

    /// JSON с провайдерами; иначе `PROVIDERS_FILE` или `PROVIDERS_JSON`.
    #[arg(long)]
    providers_file: Option<PathBuf>,

    /// Писать в журнал system и последнее сообщение (только для отладки).
    #[arg(long)]
    debug_prompts: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let _guard = init_logging(args.log.as_deref())?;

    let mut state = state_from_env_with(args.providers_file.as_deref())?;
    state.debug_prompts =
        args.debug_prompts || std::env::var("GATEWAY_DEBUG").as_deref() == Ok("1");

    let listener = tokio::net::TcpListener::bind((args.host.as_str(), args.port)).await?;
    tracing::info!(
        host = %args.host,
        port = args.port,
        providers = state.router.providers.len(),
        models = state.router.routes.len(),
        debug_prompts = state.debug_prompts,
        "gateway listening"
    );

    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

fn init_logging(
    path: Option<&Path>,
) -> anyhow::Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("ctp_gateway=info,warn"));

    let Some(path) = path else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
        return Ok(None);
    };

    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("gateway.log");
    let appender = tracing_appender::rolling::never(directory, name);
    let (writer, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .init();
    Ok(Some(guard))
}

async fn shutdown() {
    if let Err(exc) = tokio::signal::ctrl_c().await {
        tracing::warn!(error = %exc, "failed to subscribe to Ctrl+C");
    }
    tracing::info!("gateway shutting down");
}
