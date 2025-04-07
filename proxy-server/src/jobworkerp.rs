use crate::common::jsonrpc::SchemaCombiner;
use anyhow::Result;
use jobworkerp_client::{
    client::{helper::UseJobworkerpClientHelper, wrapper::JobworkerpClientWrapper},
    error,
    jobworkerp::data::{ResponseType, Runner, RunnerData, RunnerId, RunnerType, WorkerData},
    proto::JobworkerpProto,
};
use rmcp::{
    model::{
        CallToolRequestMethod, CallToolRequestParam, CallToolResult, CancelledNotificationParam,
        Content, Implementation, ListToolsResult, PaginatedRequestParam, ProtocolVersion,
        ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    Error as McpError, RoleServer, ServerHandler,
};
use serde_json::Value;
use std::{future::Future, sync::Arc};

const TIMEOUT_SEC: u32 = 60;
const CREATION_TOOL_DESCRIPTION: &str =
    "Create Tools from workflow definitions provided as JSON. The workflow definition must:

- Conform to the specified JSON schema
- Include an input schema section that defines the parameters created workflow Tool will accept
- When this workflow is executed as a Tool, it will receive parameters matching this input schema
- Specify execution steps that utilize any available runner(function) in the system (except this creation Tool)";

#[derive(Clone)]
pub struct JobworkerpRouter {
    pub jobworkerp_client: Arc<JobworkerpClientWrapper>,
}

impl JobworkerpRouter {
    const WORKFLOW_CHANNEL: Option<&str> = Some("workflow");
    pub async fn new(address: &str, request_timeout_sec: Option<u32>) -> Result<Self> {
        let jobworkerp_client = JobworkerpClientWrapper::new(address, request_timeout_sec).await?;
        Ok(Self {
            jobworkerp_client: Arc::new(jobworkerp_client),
        })
    }
    pub async fn create_workflow(
        &self,
        runner_id: RunnerId,
        runner_data: RunnerData,
        workflow_name: &str,
        workflow_description: &str,
        workflow_definition: serde_json::Value,
    ) -> Result<()> {
        let settings = serde_json::json!({
            "json_data": workflow_definition.to_string()
        });
        let runner_settings_descriptor = JobworkerpProto::parse_runner_settings_schema_descriptor(
            &runner_data,
        )
        .map_err(|e| {
            anyhow::anyhow!(
                "Failed to parse runner_settings schema descriptor: {:#?}",
                e
            )
        })?;
        let runner_settings = if let Some(ope_desc) = runner_settings_descriptor {
            tracing::debug!("runner settings schema exists: {:#?}", &settings);
            JobworkerpProto::json_value_to_message(ope_desc, &settings, true)
                .map_err(|e| anyhow::anyhow!("Failed to parse runner_settings schema: {:#?}", e))?
        } else {
            tracing::debug!("runner settings schema empty");
            vec![]
        };

        let data = WorkerData {
            name: workflow_name.to_string(),
            description: workflow_description.to_string(),
            runner_id: Some(runner_id),
            runner_settings,
            channel: Self::WORKFLOW_CHANNEL.map(|s| s.to_string()),
            response_type: ResponseType::Direct as i32,
            broadcast_results: true,
            ..Default::default()
        };
        let worker = self.jobworkerp_client.find_or_create_worker(&data).await;
        match worker {
            Ok(worker) => {
                tracing::info!("Worker created: {:?}", worker);
                Ok(())
            }
            Err(e) => {
                tracing::error!("Failed to create worker: {}", e);
                Err(e)
            }
        }
    }
    // try value.remove(key) -> (string -> parsed json) or json
    pub fn parse_as_json_and_string_with_key_or_noop(
        &self,
        key: &str,
        mut value: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Map<String, serde_json::Value>> {
        if let Some(candidate_value) = value.remove(key) {
            if candidate_value.is_object()
                && candidate_value.as_object().is_some_and(|o| !o.is_empty())
            {
                match candidate_value {
                    serde_json::Value::Object(obj) => Ok(obj.clone()),
                    _ => Ok(serde_json::Map::new()),
                }
            } else if candidate_value.is_string() {
                match candidate_value {
                    serde_json::Value::String(s) if !s.is_empty() => {
                        // try parse str as json
                        let parsed =
                            serde_json::from_str::<serde_json::Value>(s.as_str()).or_else(|e| {
                                tracing::warn!("Failed to parse string as json: {}", e);
                                serde_yaml::from_str::<serde_json::Value>(s.as_str()).inspect_err(
                                    |e| {
                                        tracing::warn!("Failed to parse string as yaml: {}", e);
                                    },
                                )
                            });
                        if let Ok(parsed_value) = parsed {
                            if parsed_value.is_object()
                                && parsed_value.as_object().is_some_and(|o| !o.is_empty())
                            {
                                match parsed_value {
                                    serde_json::Value::Object(obj) => Ok(obj),
                                    _ => Ok(serde_json::Map::new()),
                                }
                            } else {
                                tracing::warn!(
                                    "data is not an object(logic error): {:#?}",
                                    &parsed_value
                                );
                                Ok(value)
                            }
                        } else {
                            tracing::warn!(
                                "data string is not a valid json: {:#?}, {:?}",
                                &parsed,
                                &s
                            );
                            Ok(value)
                        }
                    }
                    _ => {
                        tracing::warn!(
                            "data is not a valid json(logic error): {:#?}",
                            &candidate_value
                        );
                        Ok(value)
                    }
                }
            } else {
                tracing::warn!(
                    "data key:{} is not a valid json: {:#?}",
                    key,
                    &candidate_value
                );
                Ok(value)
            }
        } else {
            Ok(value)
        }
    }
    fn parse_arguments_for_reusable_workflow(
        &self,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Map<String, serde_json::Value>> {
        // XXX LLM may prefer to use `arguments` instead of `settings` in the request
        let arguments = self.parse_as_json_and_string_with_key_or_noop("arguments", arguments)?;
        let arguments = self.parse_as_json_and_string_with_key_or_noop("settings", arguments)?;
        // XXX LLM may misread the arguments as same as the INLINE_WORKFLOW (workflow_data)
        self.parse_as_json_and_string_with_key_or_noop("workflow_data", arguments)
    }
}

// https://github.com/modelcontextprotocol/rust-sdk/blob/main/crates/rmcp/src/handler/server.rs
impl ServerHandler for JobworkerpRouter {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                // .enable_prompts()
                // .enable_resources()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "The system runs as an asynchronous job processing server that executes various functions in parallel. It supports general-purpose processing tasks like shell commands and HTTP/gRPC requests, while allowing users to create workflows through JSON-defined specifications. These workflows can compose multiple functions with defined input/output schemas, with all operations managed concurrently for efficient execution.".to_string(),
            ),
        }
    }
    fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>, // TODO use to cancel job
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        // std::future::ready(Err(McpError::method_not_found::<CallToolRequestMethod>()))
        async move {
            tracing::debug!("call_tool: {:?}", &request);
            match self
                .jobworkerp_client
                .find_runner_by_name(&request.name)
                .await
            {
                // create workflow
                Ok(Some(Runner {
                    id: Some(rid),
                    data: Some(rdata),
                })) if rdata.runner_type == RunnerType::ReusableWorkflow as i32 => {
                    tracing::debug!("found calling to reusable workflow: {:?}", &rdata);
                    let arguments = request
                        .arguments
                        .and_then(|a| self.parse_arguments_for_reusable_workflow(a).ok());
                    match arguments {
                        Some(arguments) => {
                            tracing::trace!("workflow_data: {:?}", &arguments);
                            let document = arguments.get("document").cloned();
                            let name = document
                                .as_ref()
                                .and_then(|t| t.get("name"))
                                .and_then(|t| t.as_str().map(|s| s.to_string()))
                                .unwrap_or(rdata.name.clone());
                            let description = document
                                .as_ref()
                                .and_then(|d| d.get("summary"))
                                .and_then(|d| d.as_str().map(|s| s.to_string()))
                                .unwrap_or_default();
                            match self
                                .create_workflow(
                                    rid,
                                    rdata,
                                    &name,
                                    &description,
                                    serde_json::Value::Object(arguments),
                                )
                                .await
                            {
                                Ok(_) => {
                                    tracing::info!("Workflow created: {}", request.name);
                                    Ok(CallToolResult {
                                        content: vec![Content::json(
                                            serde_json::json!({"status": "ok"}),
                                        )?],
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
                        _ => {
                            tracing::warn!("Workflow data is not found");
                            return Err(McpError::invalid_params(
                                "Workflow creation requires a workflow json arguments.",
                                None,
                            ));
                        }
                    }
                }
                Ok(Some(runner)) => {
                    tracing::debug!("found runner: {:?}", &runner);
                    // runner
                    let request_args = request.arguments.unwrap_or_default();
                    let settings = request_args.get("settings").cloned();
                    let arguments = request_args.get("arguments").cloned();
                    // TODO set worker name and other params
                    let result = self
                        .jobworkerp_client
                        .setup_worker_and_enqueue_with_json(
                            runner.data.map(|r| r.name).unwrap().as_str(), // runner(runner) name
                            settings,                                      // runner_settings data
                            None, // worker parameters (if not exists, use default values)
                            arguments.unwrap_or(Value::Null), // enqueue job args
                            TIMEOUT_SEC, // job timeout in seconds
                        )
                        .await
                        .map_err(|e| match e.downcast_ref() {
                            Some(error::ClientError::NotFound(m)) => {
                                tracing::info!("Not found: {}", m);
                                McpError::method_not_found::<CallToolRequestMethod>()
                            }
                            Some(e) => {
                                tracing::error!("Failed to enqueue job: {}", e);
                                McpError::internal_error(
                                    format!("Failed to enqueue job: {}", e),
                                    None,
                                )
                            }
                            None => McpError::internal_error(
                                format!("Failed to enqueue job: {}", e),
                                None,
                            ),
                        })?;
                    Ok(CallToolResult {
                        content: vec![Content::json(result)?],
                        is_error: None,
                    })
                }
                Ok(None) => {
                    tracing::warn!("runner not found: {:?}", &request.name);
                    Err(McpError::method_not_found::<CallToolRequestMethod>())
                }
                Err(e) => {
                    tracing::error!("error: {:#?}", &e);
                    // Err(McpError::internal_error(
                    //     format!("Failed to find runner: {}", e),
                    //     None,
                    // ))
                    Err(McpError::method_not_found::<CallToolRequestMethod>())
                }
            }
        }
    }
    fn list_tools(
        &self,
        _request: PaginatedRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        async move {
            let functions = self
                .jobworkerp_client
                .find_function_list(false, false)
                .await
                .map_err(|e| {
                    McpError::internal_error(format!("Failed to find tools: {}", e), None)
                })?;
            let tool_list = functions
                .into_iter()
                .flat_map(|tool| {
                    // ReusableWorkflow Runner is displayed as workflow(function) creation tool
                    if tool.worker_id.is_none() && tool.runner_id.is_some_and(|id| id.value < 0) {
                        Some(Tool::new(
                            tool.name,
                            CREATION_TOOL_DESCRIPTION,
                            tool.input_schema
                                .and_then(|s| {
                                    s.settings.and_then(|f| {
                                        serde_json::from_str(f.as_str())
                                            .or_else(|e1| {
                                                tracing::warn!(
                                                    "Failed to parse settings as json: {}",
                                                    e1
                                                );
                                                serde_yaml::from_str(f.as_str()).inspect_err(|e2| {
                                                    tracing::warn!(
                                                        "Failed to parse settings as yaml: {}",
                                                        e2
                                                    );
                                                })
                                            })
                                            .ok()
                                    })
                                })
                                .unwrap_or(serde_json::json!({}))
                                .as_object()
                                .cloned()
                                .unwrap_or_default(),
                        ))
                    } else {
                        let mut schema_combiner = SchemaCombiner::new();
                        tool.input_schema
                            .as_ref()
                            .and_then(|s| s.settings.clone())
                            .and_then(|s| {
                                schema_combiner
                                    .add_schema_from_string(
                                        "settings",
                                        s.as_str(),
                                        Some("Tool init settings".to_string()),
                                    )
                                    .inspect_err(|e| {
                                        tracing::error!("Failed to parse schema: {}", e)
                                    })
                                    .ok()
                            });
                        tool.input_schema.and_then(|s| {
                            schema_combiner
                                .add_schema_from_string(
                                    "arguments",
                                    s.arguments.as_str(),
                                    Some("Tool arguments".to_string()),
                                )
                                .inspect_err(|e| tracing::error!("Failed to parse schema: {}", e))
                                .ok()
                        });
                        match schema_combiner.generate_combined_schema() {
                            Ok(schema) => Some(Tool::new(tool.name, tool.description, schema)),
                            Err(e) => {
                                tracing::error!("Failed to generate schema: {}", e);
                                None
                            }
                        }
                    }
                })
                .collect::<Vec<_>>();

            Ok(ListToolsResult {
                tools: tool_list,
                next_cursor: None,
            })
            // std::future::ready(Ok(ListToolsResult::default()))
        }
    }
    fn on_cancelled(
        &self,
        _notification: CancelledNotificationParam,
    ) -> impl Future<Output = ()> + Send + '_ {
        // TODO cancel the job
        std::future::ready(())
    }
    // TODO: Implement the following methods
    // fn get_prompt(
    //     &self,
    //     request: GetPromptRequestParam,
    //     context: RequestContext<RoleServer>,
    // ) -> impl Future<Output = Result<GetPromptResult, McpError>> + Send + '_ {
    //     std::future::ready(Err(McpError::method_not_found::<GetPromptRequestMethod>()))
    // }
    // fn list_prompts(
    //     &self,
    //     request: PaginatedRequestParam,
    //     context: RequestContext<RoleServer>,
    // ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + Send + '_ {
    //     std::future::ready(Ok(ListPromptsResult::default()))
    // }
    // fn list_resources(
    //     &self,
    //     request: PaginatedRequestParam,
    //     context: RequestContext<RoleServer>,
    // ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
    //     std::future::ready(Ok(ListResourcesResult::default()))
    // }
    // fn list_resource_templates(
    //     &self,
    //     request: PaginatedRequestParam,
    //     context: RequestContext<RoleServer>,
    // ) -> impl Future<Output = Result<ListResourceTemplatesResult, McpError>> + Send + '_ {
    //     std::future::ready(Ok(ListResourceTemplatesResult::default()))
    // }
    // fn read_resource(
    //     &self,
    //     request: ReadResourceRequestParam,
    //     context: RequestContext<RoleServer>,
    // ) -> impl Future<Output = Result<ReadResourceResult, McpError>> + Send + '_ {
    //     std::future::ready(Err(
    //         McpError::method_not_found::<ReadResourceRequestMethod>(),
    //     ))
    // }
}
