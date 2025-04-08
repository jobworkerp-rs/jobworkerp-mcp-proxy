# Jobworkerp MCP Proxy Server (Experimental)

[日本語版はこちら](README_ja.md)

Jobworkerp MCP Proxy is a proxy server implementation that mediates between remote [jobworkerp-rs](https://github.com/jobworkerp-rs/jobworkerp-rs) servers and MCP tools (Model Control Protocol).

## Overview

The Jobworkerp MCP proxy server receives MCP tool list requests and call_tool requests from MCP clients, converts them into asynchronous jobs for [jobworkerp-rs](https://github.com/jobworkerp-rs/jobworkerp-rs), and executes the tools. The results are then returned to the client.

## Features

- Proxy MCP client requests to jobworkerp
  - Convert requests into asynchronous jobs and forward them to the jobworkerp server
  - Return asynchronous processing results to MCP clients
- Tool creation capabilities
  - Create Reusable Workflows: Build workflows that can be reused as tools
  - Create Custom Workers: Implement specialized tools for specific processes
  - Automatic Tool Creation by LLMs: LLM used as MCP clients can automatically create necessary tools (by Tool: REUSABLE_WORKFLOW)

## Structure

- `proxy-server/`: MCP Proxy server implementation
- `all-in-one/`: Integrated package including jobworkerp-rs as an MCP server

## Operation Modes

### All-in-One Mode

When built as an All-in-One package (in the `all-in-one/` directory) including jobworkerp-rs, it operates as an MCP server in a single binary. In this mode, both the proxy server and jobworkerp server functionalities run in a single process.

**Features:**

- Single binary operation
- No need to start a separate jobworkerp server
- Simple deployment and management
- Ideal for local development and testing

### Proxy Mode

Operates as a proxy connecting to an existing jobworkerp server. In this mode, it receives MCP requests and forwards them to the configured remote jobworkerp server.

**Features:**

- Integration with existing jobworkerp infrastructure
- Enables scalable environment setup
- Load distribution through resource distribution
- Suitable for large-scale production environments

## Build and Run

```bash
# Build the project
cargo build

# Run the SSE server in All-in-One mode
cargo run --bin sse-server

# Run the stdio server in All-in-One mode
cargo run --bin stdio-server

# Run the SSE proxy server in Proxy mode (requires a remote jobworkerp server)
cargo run --bin sse-proxy-server

# Run the stdio proxy server in Proxy mode (requires a remote jobworkerp server)
cargo run --bin stdio-proxy-server
```

## Environment Variables and Configuration

### Main Environment Variables

- `MCP_ADDR`: MCP proxy server bind address (default: `127.0.0.1:8000`)
- `JOBWORKERP_ADDR`: URL of the jobworkerp server to proxy to (default: `http://127.0.0.1:9000`)
- `REQUEST_TIMEOUT_SEC`: Request timeout in seconds (default: `60`)
- `RUST_LOG`: Log level configuration (recommended: `info,h2=warn`)

### Environment Configuration File

Environment settings can also be defined in a `.env` file. A sample configuration file `dot.env` is available in the project root.
To use it, copy this file to `.env` and modify the settings as needed.

```bash
# Copy sample configuration file to .env
cp dot.env .env

# Edit the .env file as needed
```

### Configuration Examples

```bash
# Proxy mode settings
JOBWORKERP_ADDR="http://127.0.0.1:9010"  # jobworkerp server address
REQUEST_TIMEOUT_SEC=60                   # Request timeout in seconds
RUST_LOG=info,h2=warn                    # Log level settings

# jobworkerp settings for All-in-One mode
GRPC_ADDR=0.0.0.0:9010                   # gRPC server address
SQLITE_URL="sqlite:///path/to/db.sqlite3" # SQLite database path
SQLITE_MAX_CONNECTIONS=20                # SQLite maximum connections

# Distributed queue mode settings (if needed)
REDIS_URL="redis://127.0.0.1:6379/0"     # Redis connection URL
```

## Related Projects

- [jobworkerp-rs](https://github.com/jobworkerp-rs/jobworkerp-rs): Asynchronous job worker system implemented in Rust

## Using Claude Desktop as an MCP Client

Claude Desktop can be used as an MCP client with this proxy server. You can configure Claude Desktop's settings file to integrate with the MCP server.

### Setup Instructions

1. Edit the Claude Desktop configuration file (typically `~/Library/Application Support/Claude/claude_desktop_config.json`)
2. Add the MCP server settings as follows:

```json
{
    "mcpServers": {
        "rust-test-server": {
            "command": "/Users/user_name/works/rust/jobworkerp-rs/mcp-proxy/target/debug/stdio-server",
            "args": [],
            "env": {
                "RUST_BACKTRACE": "1",
                "JOBWORKERP_ADDR":"http://127.0.0.1:9010",
                "REQUEST_TIMEOUT_SEC":"60",
                "RUST_LOG":"debug,h2=warn",
                "GRPC_ADDR":"0.0.0.0:9010",
                "SQLITE_URL":"sqlite:///Users/user_name/jobworkerp_local.sqlite3",
                "SQLITE_MAX_CONNECTIONS":"10"
            }
        }
    }
}
```

### Usage

1. When you launch the Claude Desktop application, you should see a tool list icon at the bottom of the chat panel if everything is working correctly
2. Engage with Claude AI as usual, utilizing job worker functionality through the All-in-One server

**Note**: Please modify paths and URL settings appropriately for your environment.

**Note**: When using stdio-server with Claude Desktop, processes may sometimes remain running.
