use std::{
    fs::OpenOptions,
    future::Future,
    path::{Path, PathBuf},
    time::SystemTime,
};

use futures::StreamExt;
use mirrord_config::LayerConfig;
use rand::distr::{Alphanumeric, SampleString};
use tokio::io::AsyncWriteExt;
use tokio_stream::Stream;
use tracing_subscriber::{prelude::*, EnvFilter};

use crate::{
    config::Commands,
    error::{CliError, ExternalProxyError, InternalProxyError},
};

/// Tries to initialize tracing in the current process.
pub async fn init_tracing_registry(
    command: &Commands,
    watch: drain::Watch,
) -> Result<(), CliError> {
    // Logging to the mirrord-console always takes precedence.
    if let Ok(console_addr) = std::env::var("MIRRORD_CONSOLE_ADDR") {
        mirrord_console::init_async_logger(&console_addr, watch.clone(), 124).await?;

        return Ok(());
    }

    // Proxies initialize tracing independently.
    if matches!(
        command,
        Commands::InternalProxy { .. } | Commands::ExternalProxy { .. }
    ) {
        return Ok(());
    }

    let do_init = match command {
        Commands::ListTargets(_) | Commands::ExtensionExec(_) => {
            // `ls` and `ext` commands need the errors in json format.
            let _ = miette::set_hook(Box::new(|_| Box::new(miette::JSONReportHandler::new())));

            // There are situations where even if running "ext" commands that shouldn't log,
            // we need the logs for debugging issues.
            std::env::var("MIRRORD_FORCE_LOG")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(false)
        }

        _ => true,
    };

    if do_init {
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_file(true)
                    .with_line_number(true)
                    .pretty(),
            )
            .with(tracing_subscriber::EnvFilter::from_default_env())
            .init();
    }

    Ok(())
}

/// Returns a default randomized path for intproxy/extproxy logs.
fn default_logfile_path(prefix: &str) -> PathBuf {
    let random_name: String = Alphanumeric.sample_string(&mut rand::rng(), 7);
    let timestamp = SystemTime::UNIX_EPOCH
        .elapsed()
        .expect("now must have some delta from UNIX_EPOCH, it isn't 1970 anymore")
        .as_secs();

    PathBuf::from(format!("/tmp/{prefix}-{timestamp}-{random_name}.log"))
}

fn init_proxy_tracing_registry(
    log_destination: &Path,
    log_level: Option<&str>,
) -> std::io::Result<()> {
    if std::env::var("MIRRORD_CONSOLE_ADDR").is_ok() {
        return Ok(());
    }

    let output_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_destination)?;

    let env_filter = log_level
        .map(|log_level| EnvFilter::builder().parse_lossy(log_level))
        .unwrap_or_else(EnvFilter::from_default_env);

    tracing_subscriber::fmt()
        .with_writer(output_file)
        .with_ansi(false)
        .with_env_filter(env_filter)
        .with_file(true)
        .with_line_number(true)
        .pretty()
        .init();

    Ok(())
}

pub fn init_intproxy_tracing_registry(config: &LayerConfig) -> Result<(), InternalProxyError> {
    if !config.internal_proxy.container_mode {
        // Setting up default logging for intproxy.
        let log_destination = config
            .internal_proxy
            .log_destination
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| default_logfile_path("mirrord-intproxy"));

        init_proxy_tracing_registry(&log_destination, config.internal_proxy.log_level.as_deref())
            .map_err(|fail| {
                InternalProxyError::OpenLogFile(log_destination.to_string_lossy().to_string(), fail)
            })
    } else {
        let env_filter = config
            .internal_proxy
            .log_level
            .as_ref()
            .map(|log_level| EnvFilter::builder().parse_lossy(log_level))
            .unwrap_or_else(EnvFilter::from_default_env);

        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .with_env_filter(env_filter)
            .with_file(true)
            .with_line_number(true)
            .pretty()
            .init();

        Ok(())
    }
}

pub fn init_extproxy_tracing_registry(config: &LayerConfig) -> Result<(), ExternalProxyError> {
    // Setting up default logging for extproxy.
    let log_destination = config
        .external_proxy
        .log_destination
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| default_logfile_path("mirrord-extproxy"));

    init_proxy_tracing_registry(&log_destination, config.external_proxy.log_level.as_deref())
        .map_err(|fail| {
            ExternalProxyError::OpenLogFile(log_destination.to_string_lossy().to_string(), fail)
        })
}

pub async fn pipe_intproxy_sidecar_logs<'s, S>(
    config: &LayerConfig,
    stream: S,
) -> Result<impl Future<Output = ()> + 's, InternalProxyError>
where
    S: Stream<Item = std::io::Result<String>> + 's,
{
    let log_destination = config
        .internal_proxy
        .log_destination
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| default_logfile_path("mirrord-intproxy"));

    let mut output_file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_destination)
        .await
        .map_err(|fail| {
            InternalProxyError::OpenLogFile(log_destination.to_string_lossy().to_string(), fail)
        })?;

    Ok(async move {
        let mut stream = std::pin::pin!(stream);

        while let Some(line) = stream.next().await {
            let result: std::io::Result<_> = try {
                output_file.write_all(line?.as_bytes()).await?;
                output_file.write_u8(b'\n').await?;

                output_file.flush().await?;
            };

            if let Err(error) = result {
                tracing::error!(?error, "unable to pipe logs from intproxy");
            }
        }
    })
}
