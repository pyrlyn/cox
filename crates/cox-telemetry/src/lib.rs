// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Process-wide structured logging and optional OTLP export (T32.9; `docs/
//! design/crates.md` C9). Separate from `crates/cox` because it is the only
//! user of five otel crates (dependency (a)); it takes plain values rather
//! than `cox_protocol::Config` so it depends on no workspace crate. This
//! belongs at the binary boundary: core emits `tracing` spans but never
//! opens files or sockets, while this crate owns exporter lifecycle and
//! shutdown flushing.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

/// Why installing local logging or OTLP export failed.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    /// Creating `<home>/logs` failed.
    #[error("failed to create the log directory: {0}")]
    LogDir(#[from] std::io::Error),
    /// `log_level` (or the derived OTLP log filter) is not a valid `tracing`
    /// filter string.
    #[error("invalid log filter: {0}")]
    Filter(#[from] tracing_subscriber::filter::ParseError),
    /// The global subscriber was already installed, or installing it failed.
    #[error("failed to install the tracing subscriber: {0}")]
    Init(#[from] tracing_subscriber::util::TryInitError),
    /// `otel=true` but this binary was built with `--no-default-features`.
    #[error("telemetry.otel=true requires a cox build with the `otel` feature")]
    OtelFeatureDisabled,
    /// Building the OTLP span or log exporter failed.
    #[cfg(feature = "otel")]
    #[error("failed to build the OTLP exporter: {0}")]
    Otlp(#[from] opentelemetry_otlp::ExporterBuildError),
}

/// Keeps asynchronous file logging and OTLP providers alive until shutdown.
pub struct TelemetryGuard {
    _file: WorkerGuard,
    #[cfg(feature = "otel")]
    tracer: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
    #[cfg(feature = "otel")]
    logger: Option<opentelemetry_sdk::logs::SdkLoggerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        #[cfg(feature = "otel")]
        {
            if let Some(provider) = self.logger.take() {
                let _ = provider.shutdown();
            }
            if let Some(provider) = self.tracer.take() {
                let _ = provider.shutdown();
            }
        }
    }
}

/// Installs local JSON logging and, when `otel` is set, OTLP trace/log
/// layers. `log_level` is a `tracing` filter string (`core.log_level`);
/// `endpoint` is the OTLP collector base URL (`telemetry.endpoint`, empty
/// for the OTLP SDK default).
#[cfg_attr(not(feature = "otel"), allow(unused_variables))]
pub fn init(
    log_level: &str,
    otel: bool,
    endpoint: &str,
    home: &Path,
) -> Result<TelemetryGuard, TelemetryError> {
    let log_dir = home.join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let appender = tracing_appender::rolling::daily(log_dir, "cox.log");
    let (writer, file_guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_new(log_level)?;
    let file_layer = tracing_subscriber::fmt::layer()
        .json()
        .flatten_event(true)
        .with_current_span(true)
        .with_span_list(true)
        .with_ansi(false)
        .with_writer(writer)
        .with_filter(filter.clone());

    if !otel {
        tracing_subscriber::registry().with(file_layer).try_init()?;
        return Ok(TelemetryGuard {
            _file: file_guard,
            #[cfg(feature = "otel")]
            tracer: None,
            #[cfg(feature = "otel")]
            logger: None,
        });
    }

    #[cfg(not(feature = "otel"))]
    return Err(TelemetryError::OtelFeatureDisabled);

    #[cfg(feature = "otel")]
    {
        init_otel(log_level, endpoint, file_layer, file_guard)
    }
}

#[cfg(feature = "otel")]
fn init_otel<S>(
    log_level: &str,
    endpoint: &str,
    file_layer: S,
    file_guard: WorkerGuard,
) -> Result<TelemetryGuard, TelemetryError>
where
    S: tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync + 'static,
{
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::WithExportConfig as _;

    let mut span_builder = opentelemetry_otlp::SpanExporter::builder().with_http();
    let mut log_builder = opentelemetry_otlp::LogExporter::builder().with_http();
    if !endpoint.trim().is_empty() {
        span_builder = span_builder.with_endpoint(signal_endpoint(endpoint, "v1/traces"));
        log_builder = log_builder.with_endpoint(signal_endpoint(endpoint, "v1/logs"));
    }
    let resource = resource();
    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(span_builder.build()?)
        .with_resource(resource.clone())
        .build();
    let logger_provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
        .with_batch_exporter(log_builder.build()?)
        .with_resource(resource)
        .build();
    let trace_layer = tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer("cox"));
    let log_filter = EnvFilter::try_new(format!(
        "{log_level},hyper=off,h2=off,reqwest=off,opentelemetry=off"
    ))?;
    let log_layer =
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&logger_provider)
            .with_filter(log_filter);

    tracing_subscriber::registry()
        .with(file_layer)
        .with(trace_layer)
        .with(log_layer)
        .try_init()?;
    Ok(TelemetryGuard {
        _file: file_guard,
        tracer: Some(tracer_provider),
        logger: Some(logger_provider),
    })
}

#[cfg(feature = "otel")]
fn resource() -> opentelemetry_sdk::Resource {
    // A code-set service name overrides detectors. Apply the fallback first,
    // then resource attributes, then the explicit OTEL_SERVICE_NAME override.
    let builder = opentelemetry_sdk::Resource::builder()
        .with_service_name("cox")
        .with_detector(Box::new(
            opentelemetry_sdk::resource::EnvResourceDetector::new(),
        ));
    match std::env::var("OTEL_SERVICE_NAME")
        .ok()
        .filter(|s| !s.is_empty())
    {
        Some(name) => builder.with_service_name(name).build(),
        None => builder.build(),
    }
}

#[cfg(feature = "otel")]
fn signal_endpoint(base: &str, signal: &str) -> String {
    format!("{}/{signal}", base.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "otel")]
    use std::io::{Read as _, Write as _};

    #[cfg(feature = "otel")]
    #[test]
    fn telemetry_resource_service_name_precedence() {
        for (attributes, service, expected) in [
            ("deployment.environment=test", "", "cox"),
            (
                "service.name=from-attributes,deployment.environment=test",
                "",
                "from-attributes",
            ),
            (
                "service.name=from-attributes,deployment.environment=test",
                "from-service",
                "from-service",
            ),
        ] {
            let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .args([
                    "--exact",
                    "tests::telemetry_resource_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("OTEL_RESOURCE_ATTRIBUTES", attributes)
                .env("OTEL_SERVICE_NAME", service)
                .env("COX_TEST_SERVICE_NAME", expected)
                .output()
                .expect("resource child");
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[cfg(feature = "otel")]
    #[test]
    #[ignore = "isolated resource environment; run by precedence test"]
    fn telemetry_resource_child() {
        let resource = super::resource();
        assert_eq!(
            resource
                .get(&opentelemetry::Key::new("service.name"))
                .map(|v| v.to_string()),
            Some(std::env::var("COX_TEST_SERVICE_NAME").expect("expected name")),
        );
        assert_eq!(
            resource
                .get(&opentelemetry::Key::new("deployment.environment"))
                .map(|v| v.to_string()),
            Some("test".into()),
        );
    }

    #[cfg(feature = "otel")]
    #[test]
    fn telemetry_signal_endpoints_are_otlp_http_paths() {
        assert_eq!(
            super::signal_endpoint("http://localhost:4318/", "v1/traces"),
            "http://localhost:4318/v1/traces"
        );
        assert_eq!(
            super::signal_endpoint("https://ingest.example/otel", "v1/logs"),
            "https://ingest.example/otel/v1/logs"
        );
    }

    #[cfg(feature = "otel")]
    #[test]
    fn telemetry_otlp_collector_receives_span_and_log() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("collector bind");
        let address = listener.local_addr().expect("collector address");
        let collector = std::thread::spawn(move || {
            let mut paths = Vec::new();
            for stream in listener.incoming().take(2) {
                let mut stream = stream.expect("collector accept");
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .expect("read timeout");
                let mut request = Vec::new();
                let mut chunk = [0_u8; 4096];
                loop {
                    let read = stream.read(&mut chunk).expect("collector read");
                    request.extend_from_slice(&chunk[..read]);
                    let Some(header_end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&request[..header_end + 4]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + 4 + length {
                        assert!(length > 0, "OTLP payload is non-empty");
                        paths.push(
                            headers
                                .lines()
                                .next()
                                .and_then(|line| line.split_whitespace().nth(1))
                                .unwrap_or_default()
                                .to_string(),
                        );
                        break;
                    }
                }
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .expect("collector response");
            }
            paths
        });

        let home = tempfile::tempdir().expect("telemetry home");
        let guard = super::init("info", true, &format!("http://{address}"), home.path())
            .expect("telemetry init");
        let span = tracing::info_span!("telemetry_test_span", session.id = "session-1");
        let _entered = span.enter();
        tracing::info!(event.name = "telemetry_test_log", "test log");
        drop(_entered);
        drop(span);
        drop(guard);

        let mut paths = collector.join().expect("collector thread");
        paths.sort();
        assert_eq!(paths, ["/v1/logs", "/v1/traces"]);
    }
}
