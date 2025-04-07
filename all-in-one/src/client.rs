use anyhow::Result;
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParam, ClientCapabilities, ClientInfo, Implementation},
    transport::SseTransport,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use clap::Parser;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Tool name to call
    #[arg(short, long, default_value = "COMMAND")]
    name: String,

    /// Tool arguments as a JSON string
    #[arg(short, long, conflicts_with = "file")]
    arguments: Option<String>,

    /// Tool arguments from a JSON file
    #[arg(short = 'f', long, conflicts_with = "arguments")]
    file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments
    let args = Args::parse();

    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("info,{}=debug", env!("CARGO_CRATE_NAME")).into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();
    let transport = SseTransport::start("http://localhost:8000/sse").await?;
    println!("Connected to SSE server");

    let client_info = ClientInfo {
        protocol_version: Default::default(),
        capabilities: ClientCapabilities::default(),
        client_info: Implementation {
            name: "test sse client".to_string(),
            version: "0.0.1".to_string(),
        },
    };
    let client = client_info.serve(transport).await.inspect_err(|e| {
        tracing::error!("client error: {:?}", e);
    })?;
    println!("serve client");

    // Initialize
    let server_info = client.peer_info();
    tracing::debug!("Connected to server: {server_info:#?}");

    // List tools
    let tools = client.list_tools(Default::default()).await?;
    tracing::debug!("Available tools: {tools:#?}");

    // Parse arguments from JSON string, file, or use default
    let arguments = if let Some(json_str) = args.arguments {
        // Parse from direct JSON string input
        let value: Value = serde_json::from_str(&json_str)
            .map_err(|e| anyhow::anyhow!("Failed to parse JSON arguments: {}", e))?;
        value.as_object().cloned()
    } else if let Some(file_path) = args.file {
        // Parse from JSON file
        let file_content = fs::read_to_string(file_path)
            .map_err(|e| anyhow::anyhow!("Failed to read arguments file: {}", e))?;
        let value: Value = serde_json::from_str(&file_content)
            .map_err(|e| anyhow::anyhow!("Failed to parse JSON from file: {}", e))?;
        value.as_object().cloned()
    } else {
        // Use default arguments
        serde_json::json!({"settings":{}, "arguments":{
            "command": "echo",
            "args": ["hello world"],
            "with_memory_monitoring": true,
        }})
        .as_object()
        .cloned()
    };

    match client
        .call_tool(CallToolRequestParam {
            name: args.name.into(),
            arguments,
        })
        .await
    {
        Ok(tool_result) => {
            tracing::info!("Tool result: {tool_result:#?}");
        }
        Err(e) => {
            tracing::error!("Error calling tool: {e}");
        }
    }
    client.cancel().await?;
    Ok(())
}
