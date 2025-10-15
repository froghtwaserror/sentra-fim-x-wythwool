
use anyhow::{Context, Result};
use axum::{routing::get, Router};
use prometheus::{Encoder, Registry, TextEncoder, IntCounter, IntGauge};
use std::{net::SocketAddr, sync::Arc};
use tokio::task::JoinHandle;
use tokio::sync::RwLock;
use tracing::info;

#[derive(Clone)]
pub struct Metrics {
    registry: Registry,
    pub created: IntCounter,
    pub modified: IntCounter,
    pub deleted: IntCounter,
    pub tracked_files: IntGauge,
}

impl Metrics {
    pub fn try_new() -> Result<Self> {
        let registry = Registry::new();
        let created = IntCounter::new("fim_created_total", "Files created")
            .context("create metric created")?;
        let modified = IntCounter::new("fim_modified_total", "Files modified")
            .context("create metric modified")?;
        let deleted = IntCounter::new("fim_deleted_total", "Files deleted")
            .context("create metric deleted")?;
        let tracked_files = IntGauge::new("fim_tracked_files", "Currently tracked files")
            .context("create metric tracked_files")?;

        registry.register(Box::new(created.clone()))
            .context("register created")?;
        registry.register(Box::new(modified.clone()))
            .context("register modified")?;
        registry.register(Box::new(deleted.clone()))
            .context("register deleted")?;
        registry.register(Box::new(tracked_files.clone()))
            .context("register tracked_files")?;

        Ok(Self { registry, created, modified, deleted, tracked_files })
    }

    pub fn registry(&self) -> Registry {
        self.registry.clone()
    }
}

pub async fn serve_metrics(bind: String, registry: Registry) -> Result<JoinHandle<()>> {
    let reg = Arc::new(RwLock::new(registry));
    let app = Router::new()
        .route("/metrics", get({
            let reg = reg.clone();
            move || metrics_handler(reg.clone())
        }))
        .route("/healthz", get(|| async { "ok" }));

    let addr: SocketAddr = bind.parse().context("parse metrics bind addr")?;
    info!("metrics server listening on http://{}/ (paths: /metrics, /healthz)", addr);
    let handle = tokio::spawn(async move {
        if let Err(e) = axum::Server::bind(&addr)
            .serve(app.into_make_service())
            .await {
            eprintln!("metrics server failed: {e}");
        }
    });
    Ok(handle)
}

async fn metrics_handler(registry: Arc<RwLock<Registry>>) -> String {
    let encoder = TextEncoder::new();
    let metric_families = registry.read().await.gather();
    let mut buffer = Vec::new();
    if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
        return format!("# metrics encode error: {e}");
    }
    String::from_utf8(buffer).unwrap_or_else(|_| "# metrics utf8 error".to_string())
}
