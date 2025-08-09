pub mod repository;

use anyhow::Result;
use jobworkerp_client::{
    error,
    jobworkerp::data::{Runner, RunnerData, RunnerId, RunnerType},
};
pub use repository::JobworkerpRepository;
use rmcp::{
    model::{
        CallToolRequestMethod, CallToolRequestParam, CallToolResult, CancelledNotificationParam,
        Content, Implementation, ListToolsResult, PaginatedRequestParam, ProtocolVersion,
        ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    Error as McpError, RoleServer, ServerHandler,
};
use std::{future::Future, sync::Arc};

use crate::tool_conversion::ToolConverter;

pub struct JobworkerpRouterConfig {
    pub jobworkerp_address: String,
    pub request_timeout_sec: Option<u32>,
    pub exclude_worker_as_tool: bool,
    pub exclude_runner_as_tool: bool,
    pub set_name: Option<String>,
}

#[derive(Clone)]
pub struct JobworkerpRouter {
    pub repository: Arc<JobworkerpRepository>,
    pub exclude_worker_as_tool: bool,
    pub exclude_runner_as_tool: bool,
    pub set_name: Option<String>,
}

impl JobworkerpRouter {
    pub async fn new(config: JobworkerpRouterConfig) -> Result<Self> {
        let repository =
            JobworkerpRepository::new(&config.jobworkerp_address, config.request_timeout_sec)
                .await?;

        Ok(Self {
            repository: Arc::new(repository),
            exclude_worker_as_tool: config.exclude_worker_as_tool,
            exclude_runner_as_tool: config.exclude_runner_as_tool,
            set_name: config.set_name,
        })
    }

    // Router should not have any conversion logic

    async fn handle_reusable_workflow(
        &self,
        request: &CallToolRequestParam,
        runner_id: RunnerId,
        runner_data: RunnerData,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!("found calling to reusable workflow: {:?}", &runner_data);
        match self
            .repository
            .create_workflow(runner_id, runner_data, request.arguments.clone())
            .await
        {
            Ok(_) => {
                tracing::info!("Workflow created: {}", request.name);
                Ok(CallToolResult {
                    content: vec![Content::json(serde_json::json!({"status": "ok"}))?],
                    is_error: None,
                })
            }
            Err(e) => {
                tracing::error!("Failed to create workflow: {}", e);
                Err(McpError::internal_error(
                    format!("Failed to create workflow: {}", e),
                    None,
                ))
            }
        }
    }

    async fn handle_runner_call(
        &self,
        request: &CallToolRequestParam,
        runner: Runner,
        tool_name_opt: Option<String>,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!("found runner: {:?}, tool: {:?}", &runner, &tool_name_opt);
        let request_args = request.arguments.clone().unwrap_or_default();

        let result = self
            .repository
            .setup_worker_and_enqueue_with_json(&runner, request_args, tool_name_opt)
            .await
            .map_err(|e| match e.downcast_ref() {
                Some(error::ClientError::NotFound(m)) => {
                    tracing::info!("Not found: {}", m);
                    McpError::method_not_found::<CallToolRequestMethod>()
                }
                Some(e) => {
                    tracing::error!("Failed to enqueue job: {}", e);
                    McpError::internal_error(format!("Failed to enqueue job: {}", e), None)
                }
                None => McpError::internal_error(format!("Failed to enqueue job: {}", e), None),
            })?;

        Ok(CallToolResult {
            content: vec![Content::json(result)?],
            is_error: None,
        })
    }

    async fn handle_worker_call(
        &self,
        request: &CallToolRequestParam,
    ) -> Result<CallToolResult, McpError> {
        tracing::info!("runner not found, run as worker: {:?}", &request.name);
        let request_args = request.arguments.clone().unwrap_or_default();

        let (worker_data, tool_name_opt) = self
            .repository
            .find_worker_by_name_with_mcp(&request.name)
            .await
            .map_err(|e| {
                tracing::error!("Failed to find worker: {}", e);
                McpError::method_not_found::<CallToolRequestMethod>()
            })?
            .ok_or_else(|| {
                tracing::info!("worker not found");
                McpError::method_not_found::<CallToolRequestMethod>()
            })?;

        let result = self
            .repository
            .enqueue_with_json(&worker_data, request_args, tool_name_opt)
            .await
            .map_err(|e| match e.downcast_ref() {
                Some(error::ClientError::NotFound(m)) => {
                    tracing::info!("Not found: {}", m);
                    McpError::method_not_found::<CallToolRequestMethod>()
                }
                Some(e) => {
                    tracing::error!("Failed to enqueue job: {}", e);
                    McpError::internal_error(format!("Failed to enqueue job: {}", e), None)
                }
                None => McpError::internal_error(format!("Failed to enqueue job: {}", e), None),
            })?;

        Ok(CallToolResult {
            content: vec![Content::json(result)?],
            is_error: None,
        })
    }
}

impl ServerHandler for JobworkerpRouter {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
                protocol_version: ProtocolVersion::V_2024_11_05,
                capabilities: ServerCapabilities::builder()
                    .enable_tools()
                    .build(),
                server_info: Implementation::from_build_env(),
                instructions: Some(
                    "The system runs as an asynchronous job processing server that executes various functions in parallel. It supports general-purpose processing tasks like shell commands and HTTP/gRPC requests, while allowing users to create workflows through JSON-defined specifications. These workflows can compose multiple functions with defined input/output schemas, with all operations managed concurrently for efficient execution.".to_string(),
                ),
            }
    }
    #[allow(clippy::manual_async_fn)]
    fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        async move {
            tracing::debug!("call_tool: {:?}", &request);

            match self
                .repository
                .find_runner_by_name_with_mcp(&request.name)
                .await
            {
                Ok(Some((
                    Runner {
                        id: Some(rid),
                        data: Some(rdata),
                    },
                    _,
                ))) if rdata.runner_type == RunnerType::ReusableWorkflow as i32 => {
                    self.handle_reusable_workflow(&request, rid, rdata).await
                }
                Ok(Some((runner, tool_name_opt))) => {
                    self.handle_runner_call(&request, runner, tool_name_opt)
                        .await
                }
                Ok(None) => self.handle_worker_call(&request).await,
                Err(e) => {
                    tracing::error!("error: {:#?}", &e);
                    Err(McpError::method_not_found::<CallToolRequestMethod>())
                }
            }
        }
    }
    #[allow(clippy::manual_async_fn)]
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        async move {
            let functions = if let Some(name) = self.set_name.as_ref() {
                self.repository
                    .find_function_list_by_set(name.as_str())
                    .await
                    .map_err(|e| {
                        McpError::internal_error(format!("Failed to find tools: {}", e), None)
                    })
            } else {
                self.repository
                    .find_function_list(self.exclude_runner_as_tool, self.exclude_worker_as_tool)
                    .await
                    .map_err(|e| {
                        McpError::internal_error(format!("Failed to find tools: {}", e), None)
                    })
            }?;
            ToolConverter::convert_functions_to_mcp_tools(functions).map_err(|e| {
                McpError::internal_error(format!("Failed to convert tools: {}", e), None)
            })
        }
    }
    fn on_cancelled(
        &self,
        _notification: CancelledNotificationParam,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }
}
