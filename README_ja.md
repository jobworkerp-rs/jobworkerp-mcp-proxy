# Jobworkerp MCP Proxy (実験的)

[English version here](README.md)

Jobworkerp MCP Proxyは、リモートの[jobworkerp-rs](https://github.com/jobworkerp-rs/jobworkerp-rs)サーバーとMCPツール（Model Control Protocol）間のプロキシサーバー実装です。

## 概要

Jobworkerp MCPプロキシサーバーは、MCPクライアントからMCPツールリストリクエストおよびcall_toolリクエストを受け取り、それらを[jobworkerp-rs](https://github.com/jobworkerp-rs/jobworkerp-rs)の非同期ジョブに変換してツール実行します。得られた結果はクライアントに返されます。

## 機能

- MCP clientのリクエストをjobworkerpにproxyする
  - リクエストを非同期ジョブに変換してjobworkerpサーバーへ転送
  - 非同期処理の結果をMCPクライアントに返却
- ツール作成機能
  - ReusableWorkflowの作成：作成したワークフローをツールとして再利用可能
  - Toolの作成：Worker作成により特定パラメータで実行するツールを簡単に作成
  - LLMによる自動ツール作成：MCPクライアントとして使用されるLLMにより必要なツールを自動的に作成 (Tool: REUSABLE_WORKFLOW)

## 構成

- `proxy-server/`: MCP Proxyサーバー実装
- `all-in-one/`: jobworkerp-rsを含めたMCPサーバーの統合パッケージ

## 動作モード

### All-in-Oneモード

jobworkerp-rsを含めたAll-in-Oneパッケージ（`all-in-one/`ディレクトリ）としてビルドすると、単一のバイナリでMCPサーバーとして動作します。このモードでは、プロキシサーバーとjobworkerpサーバーの両方の機能が一つのプロセスで実行されます。

**特徴:**

- 単一バイナリでの動作
- 別途jobworkerpサーバーを起動する必要なし
- 簡易なデプロイと管理
- ローカル開発やテストに最適

### プロキシモード

既存のjobworkerpサーバーに接続するプロキシとして動作します。このモードでは、MCPリクエストを受け取り、設定されたリモートのjobworkerpサーバーに転送します。

**特徴:**

- 既存のjobworkerpインフラストラクチャとの連携
- スケーラブルな環境構築が可能
- リソース分散による負荷分散
- 本番環境などの大規模運用に適している

## ビルドと実行

```bash
# プロジェクトをビルドする
cargo build

# All-in-OneモードでSSEサーバーを実行する
cargo run --bin sse-server

# All-in-Oneモードでstdioサーバーを実行する
cargo run --bin stdio-server

# プロキシモードでSSEサーバーを実行する（リモートjobworkerpサーバーが必要）
cargo run --bin sse-proxy-server

# プロキシモードでstdioサーバーを実行する（リモートjobworkerpサーバーが必要）
cargo run --bin stdio-proxy-server
```

## 環境変数と設定

### 主要な環境変数

- `MCP_ADDR`: MCPプロキシサーバーのバインドアドレス（デフォルト: `127.0.0.1:8000`）
- `JOBWORKERP_ADDR`: プロキシ先のjobworkerpサーバーのURL（デフォルト: `http://127.0.0.1:9000`）
- `REQUEST_TIMEOUT_SEC`: リクエストタイムアウト時間（秒）（デフォルト: `60`）
- `RUST_LOG`: ログレベル設定（推奨: `info,h2=warn`）
- `EXCLUDE_RUNNER_AS_TOOL`: jobworkerpのRunnerをツールから除外します (作成したワークフローやworkerの利用時にコンテキストを減らすために役立ちます)
- `EXCLUDE_WORKER_AS_TOOL`: jobworkerpのWorkerをツールから除外します (ワークフローの作成時にWorkerを利用しない場合にコンテキストを減らすために役立ちます)


### 環境設定ファイル

環境設定は`.env`ファイルに定義することも可能です。プロジェクトルートにサンプル設定ファイル`dot.env`が用意されています。
利用する際には、このファイルを`.env`にコピーして必要に応じて設定を変更してください。

```bash
# サンプル設定ファイルを.envとしてコピー
cp dot.env .env

# 必要に応じて.envファイルを編集
```

### 環境設定例

```bash
# プロキシモード用設定
JOBWORKERP_ADDR="http://127.0.0.1:9010"  # jobworkerpサーバーのアドレス
REQUEST_TIMEOUT_SEC=60                   # リクエストのタイムアウト時間（秒）
RUST_LOG=info,h2=warn                    # ログレベル設定

# All-in-Oneモード用のjobworkerp設定
GRPC_ADDR=0.0.0.0:9010                    # gRPCサーバーのアドレス
SQLITE_URL="sqlite:///path/to/db.sqlite3" # SQLiteデータベースのパス
SQLITE_MAX_CONNECTIONS=20                 # SQLite最大接続数

# 分散キューモード用設定（必要な場合）
#REDIS_URL="redis://127.0.0.1:6379/0"     # Redis接続URL
```

## 関連プロジェクト

- [jobworkerp-rs](https://github.com/jobworkerp-rs/jobworkerp-rs): Rustで実装された非同期ジョブワーカーシステム

## Claude DesktopをMCPクライアントとして使用する

Claude Desktopは、このプロキシサーバーとともにMCPクライアントとして使用できます。Claude Desktopの設定ファイルを構成することで、MCPサーバーと連携させることができます。

### 設定手順

1. Claude Desktopの設定ファイルを編集します（通常は `~/Library/Application Support/Claude/claude_desktop_config.json`）
2. 以下のようにMCPサーバー設定を追加します

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
                "SQLITE_MAX_CONNECTIONS":"10",
                "EXCLUDE_RUNNER_AS_TOOL":"false",
                "EXCLUDE_WORKER_AS_TOOL":"false"
            }
        }
    }
}
```

### 使用方法

1. Claude Desktopアプリケーションを起動するとチャット欄下にツール一覧表示アイコンが付いていれば正常稼動
2. 通常通りClaude AIとの対話を行い、All-in-Oneサーバー経由でジョブワーカー機能を利用可能

**注意**: パスやURL設定は環境に合わせて適切に変更してください。

**注意**: claude desktopでstdio-serverを利用した場合にプロセスが残ることがあります
