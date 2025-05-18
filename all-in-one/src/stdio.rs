use anyhow::Result;
use proxy_server::jobworkerp::JobworkerpRouterConfig;
use tracing_subscriber::{self, EnvFilter};

/// npx @modelcontextprotocol/inspector cargo run -p mcp-server-examples --example std_io
#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();

    // Initialize the tracing subscriber with file and stdout logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let jobworkerp_server = tokio::spawn(jobworkerp_main::boot_all_in_one());
    // wait for boot
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

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
    let set_name = std::env::var("TOOL_SET_NAME")
        .ok()
        .and_then(|s| s.parse::<String>().ok());

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
        set_name,
    };

    let stdio_server = tokio::spawn(proxy_server::boot_stdio_server(config));

    let (stdio_result, jobworkerp_result) = tokio::join!(stdio_server, jobworkerp_server);

    // 各タスクの結果をチェック
    stdio_result??;
    jobworkerp_result??;

    Ok(())
}
