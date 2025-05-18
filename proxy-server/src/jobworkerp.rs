use crate::common::jsonrpc::SchemaCombiner;
use anyhow::Result;
use jobworkerp_client::{
    client::{helper::UseJobworkerpClientHelper, wrapper::JobworkerpClientWrapper},
    error,
    jobworkerp::data::{ResponseType, Runner, RunnerData, RunnerId, RunnerType, WorkerData},
    jobworkerp::function::data::{function_specs, McpToolList},
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
use std::{collections::VecDeque, future::Future, sync::Arc};

const CREATION_TOOL_DESCRIPTION: &str =
    "Create Tools from workflow definitions provided as JSON. The workflow definition must:

- Conform to the specified JSON schema
- Include an input schema section that defines the parameters created workflow Tool will accept
- When this workflow is executed as a Tool, it will receive parameters matching this input schema
- Specify execution steps that utilize any available runner(function) in the system (except this creation Tool)";

pub struct JobworkerpRouterConfig {
    pub jobworkerp_address: String,
    pub request_timeout_sec: Option<u32>,
    pub exclude_worker_as_tool: bool,
    pub exclude_runner_as_tool: bool,
    pub set_name: Option<String>,
}

#[derive(Clone)]
pub struct JobworkerpRouter {
    pub jobworkerp_client: Arc<JobworkerpClientWrapper>,
    pub exclude_worker_as_tool: bool,
    pub exclude_runner_as_tool: bool,
    pub set_name: Option<String>,
    pub timeout_sec: u32,
}

impl JobworkerpRouter {
    const DELIMITER: &str = "___";
    const DEFAULT_TIMEOUT_SEC: u32 = 60 * 60;
    const WORKFLOW_CHANNEL: Option<&str> = Some("workflow");

    pub async fn new(config: JobworkerpRouterConfig) -> Result<Self> {
        let jobworkerp_client =
            JobworkerpClientWrapper::new(&config.jobworkerp_address, config.request_timeout_sec)
                .await?;
        Ok(Self {
            jobworkerp_client: Arc::new(jobworkerp_client),
            exclude_worker_as_tool: config.exclude_worker_as_tool,
            exclude_runner_as_tool: config.exclude_runner_as_tool,
            set_name: config.set_name,
            timeout_sec: config
                .request_timeout_sec
                .unwrap_or(Self::DEFAULT_TIMEOUT_SEC),
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
        let arguments = self.parse_as_json_and_string_with_key_or_noop("arguments", arguments)?;
        let arguments = self.parse_as_json_and_string_with_key_or_noop("settings", arguments)?;
        self.parse_as_json_and_string_with_key_or_noop("workflow_data", arguments)
    }
    fn combine_names(server_name: &str, tool_name: &str) -> String {
        format!("{}{}{}", server_name, Self::DELIMITER, tool_name)
    }
    fn divide_names(combined: &str) -> Option<(String, String)> {
        let mut v: VecDeque<&str> = combined.split(Self::DELIMITER).collect();
        match v.len().cmp(&2) {
            std::cmp::Ordering::Less => {
                tracing::error!("Failed to parse combined name: {:#?}", &combined);
                None
            }
            std::cmp::Ordering::Equal => Some((v[0].to_string(), v[1].to_string())),
            std::cmp::Ordering::Greater => {
                let server_name = v.pop_front();
                Some((
                    server_name.unwrap_or_default().to_string(),
                    v.into_iter()
                        .map(|s| s.to_string())
                        .reduce(|acc, n| format!("{}{}{}", acc, Self::DELIMITER, n))
                        .unwrap_or_default(),
                ))
            }
        }
    }
    async fn find_runner_by_name_with_mcp(
        &self,
        name: &str,
    ) -> Result<Option<(Runner, Option<String>)>> {
        match self.jobworkerp_client.find_runner_by_name(name).await {
            Ok(Some(runner)) => {
                tracing::debug!("found runner: {:?}", &runner);
                Ok(Some((runner, None)))
            }
            Ok(None) => match Self::divide_names(name) {
                Some((server_name, tool_name)) => {
                    tracing::debug!(
                        "found calling to mcp server: {}:{}",
                        &server_name,
                        &tool_name
                    );
                    self.jobworkerp_client
                        .find_runner_by_name(&server_name)
                        .await
                        .map(|res| res.map(|r| (r, Some(tool_name))))
                }
                None => Ok(None),
            },
            Err(e) => Err(e),
        }
    }
    async fn find_worker_by_name_with_mcp(
        &self,
        name: &str,
    ) -> Result<Option<(WorkerData, Option<String>)>> {
        match self.jobworkerp_client.find_worker_by_name(name).await {
            Ok(Some(worker)) => {
                tracing::debug!("found worker: {:?}", &worker);
                Ok(Some((worker.1, None)))
            }
            Ok(None) => match Self::divide_names(name) {
                Some((server_name, tool_name)) => {
                    tracing::debug!(
                        "found calling to mcp server: {}:{}",
                        &server_name,
                        &tool_name
                    );
                    self.jobworkerp_client
                        .find_worker_by_name(&server_name)
                        .await
                        .map(|res| res.map(|r| (r.1, Some(tool_name))))
                }
                None => Ok(None),
            },
            Err(e) => Err(e),
        }
    }

    async fn handle_reusable_workflow(
        &self,
        request: &CallToolRequestParam,
        runner_id: RunnerId,
        runner_data: RunnerData,
    ) -> Result<CallToolResult, McpError> {
        tracing::debug!("found calling to reusable workflow: {:?}", &runner_data);
        let arguments = request
            .arguments
            .clone()
            .and_then(|a| self.parse_arguments_for_reusable_workflow(a).ok());

        match arguments {
            Some(arguments) => {
                tracing::trace!("workflow_data: {:?}", &arguments);
                let document = arguments.get("document").cloned();
                let name = document
                    .as_ref()
                    .and_then(|t| t.get("name"))
                    .and_then(|t| t.as_str().map(|s| s.to_string()))
                    .unwrap_or(runner_data.name.clone());
                let description = document
                    .as_ref()
                    .and_then(|d| d.get("summary"))
                    .and_then(|d| d.as_str().map(|s| s.to_string()))
                    .unwrap_or_default();

                match self
                    .create_workflow(
                        runner_id,
                        runner_data,
                        &name,
                        &description,
                        serde_json::Value::Object(arguments),
                    )
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
            _ => {
                tracing::warn!("Workflow data is not found");
                Err(McpError::invalid_params(
                    "Workflow creation requires a workflow json arguments.",
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
        let settings = request_args.get("settings").cloned();
        let arguments = if runner
            .data
            .as_ref()
            .is_some_and(|r| r.runner_type() == RunnerType::McpServer)
        {
            Some(serde_json::json!(
                {
                    "tool_name": tool_name_opt,
                    "arg_json": serde_json::to_string(&request_args)
                        .inspect_err(|e| tracing::error!("Failed to parse settings as json: {}", e)).unwrap_or_default()
                }
            ))
        } else {
            request_args.get("arguments").cloned()
        };
        tracing::debug!(
            "runner settings: {:#?}, arguments: {:#?}",
            settings,
            arguments
        );

        let result = self
            .jobworkerp_client
            .setup_worker_and_enqueue_with_json(
                runner.data.map(|r| r.name).unwrap().as_str(),
                settings,
                None,
                arguments.unwrap_or(Value::Null),
                self.timeout_sec,
            )
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

        let args = request_args
            .get("arguments")
            .cloned()
            .unwrap_or(Value::Null);

        let arguments = if worker_data.runner_id.is_some_and(|id| id.value < 0) {
            tracing::info!("worker is reusable workflow");
            serde_json::json!({
                "input": args.to_string(),
            })
        } else if let Some(tool_name) = tool_name_opt {
            serde_json::json!(
                {
                    "tool_name": tool_name,
                    "arg_json": args
                }
            )
        } else {
            args
        };

        let result = self
            .jobworkerp_client
            .enqueue_with_json(&worker_data, arguments.clone(), self.timeout_sec)
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

            match self.find_runner_by_name_with_mcp(&request.name).await {
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
                self.jobworkerp_client
                    .find_function_list_by_set(name.as_str())
                    .await
                    .map_err(|e| {
                        McpError::internal_error(format!("Failed to find tools: {}", e), None)
                    })
            } else {
                self.jobworkerp_client
                    .find_function_list(self.exclude_runner_as_tool, self.exclude_worker_as_tool)
                    .await
                    .map_err(|e| {
                        McpError::internal_error(format!("Failed to find tools: {}", e), None)
                    })
            }?;
            let tool_list = functions
                .into_iter()
                .flat_map(|tool| {
                    if tool.worker_id.is_none()
                        && tool.runner_type == RunnerType::ReusableWorkflow as i32
                    {
                        vec![Tool::new(
                            tool.name,
                            CREATION_TOOL_DESCRIPTION,
                            tool.schema
                                .and_then(|s| match s {
                                    function_specs::Schema::SingleSchema(function) => {
                                        function.settings.and_then(|f| {
                                            serde_json::from_str(f.as_str())
                                                .or_else(|e1| {
                                                    tracing::warn!(
                                                        "Failed to parse settings as json: {}",
                                                        e1
                                                    );
                                                    serde_yaml::from_str(f.as_str()).inspect_err(
                                                        |e2| {
                                                            tracing::warn!(
                                                        "Failed to parse settings as yaml: {}",
                                                        e2
                                                    );
                                                        },
                                                    )
                                                })
                                                .ok()
                                        })
                                    }
                                    function_specs::Schema::McpTools(mcp) => {
                                        let mes = format!(
                                            "error: expect workflow but got mcp: {:?}",
                                            mcp
                                        );
                                        tracing::error!(mes);
                                        None
                                    }
                                })
                                .unwrap_or(serde_json::json!({}))
                                .as_object()
                                .cloned()
                                .unwrap_or_default(),
                        )]
                    } else if tool.runner_type == RunnerType::McpServer as i32 {
                        let server_name = tool.name.as_str();
                        match tool.schema {
                            Some(function_specs::Schema::McpTools(McpToolList { list })) => list
                                .into_iter()
                                .map(|tool| {
                                    Tool::new(
                                        Self::combine_names(server_name, tool.name.as_str()),
                                        tool.description.unwrap_or_default(),
                                        serde_json::from_str(tool.input_schema.as_str())
                                            .unwrap_or(serde_json::json!({}))
                                            .as_object()
                                            .cloned()
                                            .unwrap_or_default(),
                                    )
                                })
                                .collect(),
                            Some(function_specs::Schema::SingleSchema(function)) => {
                                tracing::error!(
                                    "error: expect workflow but got function: {:?}",
                                    function
                                );
                                vec![]
                            }
                            None => {
                                tracing::error!("error: expect workflow but got none: {:?}", &tool);
                                vec![]
                            }
                        }
                    } else {
                        let mut schema_combiner = SchemaCombiner::new();
                        tool.schema
                            .as_ref()
                            .and_then(|s| match s {
                                function_specs::Schema::SingleSchema(function) => {
                                    function.settings.clone()
                                }
                                function_specs::Schema::McpTools(_) => {
                                    tracing::error!(
                                        "got mcp tool in not mcp tool runner type: {:#?}",
                                        &tool
                                    );
                                    None
                                }
                            })
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
                        tool.schema
                            .as_ref()
                            .map(|s| match s {
                                function_specs::Schema::SingleSchema(function) => {
                                    function.arguments.clone()
                                }
                                function_specs::Schema::McpTools(_) => {
                                    tracing::error!(
                                        "got mcp tool in not mcp tool runner type: {:#?}",
                                        &tool
                                    );
                                    "".to_string()
                                }
                            })
                            .and_then(|args| {
                                schema_combiner
                                    .add_schema_from_string(
                                        "arguments",
                                        args.as_str(),
                                        Some("Tool arguments".to_string()),
                                    )
                                    .inspect_err(|e| {
                                        tracing::error!("Failed to parse schema: {}", e)
                                    })
                                    .ok()
                            });
                        match schema_combiner.generate_combined_schema() {
                            Ok(schema) => vec![Tool::new(tool.name, tool.description, schema)],
                            Err(e) => {
                                tracing::error!("Failed to generate schema: {}", e);
                                vec![]
                            }
                        }
                    }
                })
                .collect::<Vec<_>>();

            Ok(ListToolsResult {
                tools: tool_list,
                next_cursor: None,
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
