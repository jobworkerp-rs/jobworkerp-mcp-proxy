use crate::jobworkerp::JobworkerpRouter;
use anyhow::Result;
use jobworkerp::JobworkerpRouterConfig;
use rmcp::{
    transport::{sse_server::SseServerConfig, stdio, SseServer},
    ServiceExt,
};
use tokio_util::sync::CancellationToken;

mod common;
pub mod jobworkerp;

pub async fn boot_stdio_server(config: JobworkerpRouterConfig) -> Result<()> {
    let job_service = JobworkerpRouter::new(config).await?;

    // Create an instance of our counter router
    let service = job_service.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("serving error: {:?}", e);
    })?;

    tracing::debug!("Serving MCP server");

    service.waiting().await?;
    Ok(())
}

pub async fn boot_sse_server() -> Result<()> {
    let mcp_address = std::env::var("MCP_ADDR").unwrap_or_else(|_| "127.0.0.1:8000".to_string());

    let jobworkerp_address =
        std::env::var("JOBWORKERP_ADDR").unwrap_or_else(|_| "http://127.0.0.1:9000".to_string());
    let request_timeout_sec = std::env::var("REQUEST_TIMEOUT_SEC")
        .ok()
        .and_then(|s| s.parse::<u32>().ok());
    let exclude_runner_as_tool = std::env::var("EXCLUDE_RUNNER_AS_TOOL")
        .ok()
        .and_then(|s| s.parse::<bool>().ok())
        .unwrap_or_default();
    let exclude_worker_as_tool = std::env::var("EXCLUDE_WORKER_AS_TOOL")
        .ok()
        .and_then(|s| s.parse::<bool>().ok())
        .unwrap_or_default();

    tracing::info!(
        "Starting MCP server {} {}",
        if exclude_runner_as_tool {
            "without runner"
        } else {
            "with runner"
        },
        if exclude_worker_as_tool {
            "without worker as tool"
        } else {
            "with worker as tool"
        }
    );
    let config = JobworkerpRouterConfig {
        jobworkerp_address,
        request_timeout_sec,
        exclude_runner_as_tool,
        exclude_worker_as_tool,
    };

    let sse_config = SseServerConfig {
        sse_keep_alive: None,
        bind: mcp_address.parse()?,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: CancellationToken::new(),
    };

    let mut sse_server = SseServer::serve_with_config(sse_config).await?;
    let service = JobworkerpRouter::new(config).await?;

    let ct = sse_server.config.ct.clone();
    tokio::spawn(async move {
        while let Some(transport) = sse_server.next_transport().await {
            let service = service.clone();
            let ct = sse_server.config.ct.child_token();
            tokio::spawn(async move {
                let server = service.serve_with_ct(transport, ct).await?;
                server.waiting().await?;
                tokio::io::Result::Ok(())
            });
        }
    });
    tokio::signal::ctrl_c().await?;
    ct.cancel();

    Ok(())
}
