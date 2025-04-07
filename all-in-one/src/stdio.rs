use anyhow::Result;
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
    let stdio_server = tokio::spawn(proxy_server::boot_stdio_server());

    let (stdio_result, jobworkerp_result) = tokio::join!(stdio_server, jobworkerp_server);

    // 各タスクの結果をチェック
    stdio_result??;
    jobworkerp_result??;

    Ok(())
}
