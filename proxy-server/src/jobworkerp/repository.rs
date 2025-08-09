use anyhow::Result;
use jobworkerp_client::{
    client::{
        helper::UseJobworkerpClientHelper, wrapper::JobworkerpClientWrapper, JobworkerpClient,
    },
    jobworkerp::{
        data::{ResponseType, Runner, RunnerData, RunnerId, RunnerType, WorkerData},
        function::data::FunctionSpecs,
    },
    proto::JobworkerpProto,
};
use serde_json::{Map, Value};
use std::{collections::HashMap, sync::Arc};
use tracing;

use crate::tool_conversion::ToolConverter;

pub struct JobworkerpRepository {
    pub jobworkerp_client: Arc<JobworkerpClientWrapper>,
    pub timeout_sec: u32,
}

impl net_utils::trace::Tracing for JobworkerpRepository {}

impl UseJobworkerpClientHelper for JobworkerpRepository {}

impl jobworkerp_client::client::UseJobworkerpClient for JobworkerpRepository {
    fn jobworkerp_client(&self) -> &JobworkerpClient {
        &self.jobworkerp_client.jobworkerp_client
    }
}

impl JobworkerpRepository {
    const WORKFLOW_CHANNEL: Option<&str> = Some("workflow");

    pub async fn new(jobworkerp_address: &str, request_timeout_sec: Option<u32>) -> Result<Self> {
        let jobworkerp_client =
            JobworkerpClientWrapper::new(jobworkerp_address, request_timeout_sec).await?;
        Ok(Self {
            jobworkerp_client: Arc::new(jobworkerp_client),
            timeout_sec: request_timeout_sec.unwrap_or(60 * 60),
        })
    }

    pub fn parse_as_json_and_string_with_key_or_noop(
        &self,
        key: &str,
        mut value: Map<String, Value>,
    ) -> Result<Map<String, Value>> {
        if let Some(candidate_value) = value.remove(key) {
            if candidate_value.is_object()
                && candidate_value.as_object().is_some_and(|o| !o.is_empty())
            {
                match candidate_value {
                    Value::Object(obj) => Ok(obj.clone()),
                    _ => Ok(Map::new()),
                }
            } else if candidate_value.is_string() {
                match candidate_value {
                    Value::String(s) if !s.is_empty() => {
                        let parsed = serde_json::from_str::<Value>(s.as_str()).or_else(|e| {
                            tracing::warn!("Failed to parse string as json: {}", e);
                            serde_yaml::from_str::<Value>(s.as_str()).inspect_err(|e| {
                                tracing::warn!("Failed to parse string as yaml: {}", e);
                            })
                        });
                        if let Ok(parsed_value) = parsed {
                            if parsed_value.is_object()
                                && parsed_value.as_object().is_some_and(|o| !o.is_empty())
                            {
                                match parsed_value {
                                    Value::Object(obj) => Ok(obj),
                                    _ => Ok(Map::new()),
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

    pub fn parse_arguments_for_reusable_workflow(
        &self,
        arguments: Map<String, Value>,
    ) -> Result<Map<String, Value>> {
        let arguments = self.parse_as_json_and_string_with_key_or_noop("arguments", arguments)?;
        let arguments = self.parse_as_json_and_string_with_key_or_noop("settings", arguments)?;
        self.parse_as_json_and_string_with_key_or_noop("workflow_data", arguments)
    }

    pub async fn find_runner_by_name_with_mcp(
        &self,
        name: &str,
    ) -> Result<Option<(Runner, Option<String>)>> {
        let empty_cx = None;
        let empty = Arc::new(HashMap::new());
        match self
            .jobworkerp_client
            .find_runner_by_name(empty_cx, empty.clone(), name)
            .await
        {
            Ok(Some(runner)) => {
                tracing::debug!("found runner: {:?}", &runner);
                Ok(Some((runner, None)))
            }
            Ok(None) => match ToolConverter::divide_names(name) {
                Some((server_name, tool_name)) => {
                    tracing::debug!(
                        "found calling to mcp server: {}:{}",
                        &server_name,
                        &tool_name
                    );
                    self.jobworkerp_client
                        .find_runner_by_name(empty_cx, empty.clone(), &server_name)
                        .await
                        .map(|res| res.map(|r| (r, Some(tool_name))))
                }
                None => Ok(None),
            },
            Err(e) => Err(e),
        }
    }

    pub async fn find_worker_by_name_with_mcp(
        &self,
        name: &str,
    ) -> Result<Option<(WorkerData, Option<String>)>> {
        let empty_cx = None;
        let empty = Arc::new(HashMap::new());
        match self
            .jobworkerp_client
            .find_worker_by_name(empty_cx, empty.clone(), name)
            .await
        {
            Ok(Some(worker)) => {
                tracing::debug!("found worker: {:?}", &worker);
                Ok(Some((worker.1, None)))
            }
            Ok(None) => match ToolConverter::divide_names(name) {
                Some((server_name, tool_name)) => {
                    tracing::debug!(
                        "found calling to mcp server: {}:{}",
                        &server_name,
                        &tool_name
                    );
                    self.jobworkerp_client
                        .find_worker_by_name(empty_cx, empty.clone(), &server_name)
                        .await
                        .map(|res| res.map(|r| (r.1, Some(tool_name))))
                }
                None => Ok(None),
            },
            Err(e) => Err(e),
        }
    }

    pub async fn create_workflow(
        &self,
        runner_id: RunnerId,
        runner_data: RunnerData,
        definition: Option<Map<String, Value>>,
    ) -> Result<()> {
        let empty_cx = None;
        let empty = Arc::new(HashMap::new());

        tracing::debug!("found calling to reusable workflow: {:?}", &runner_data);
        let arguments = definition.and_then(|a| self.parse_arguments_for_reusable_workflow(a).ok());

        if let Some(arguments) = arguments {
            tracing::trace!("workflow_data: {:?}", &arguments);
            let workflow_definition = serde_json::Value::Object(arguments);
            let document = workflow_definition.get("document").cloned();
            let workflow_name = document
                .as_ref()
                .and_then(|t| t.get("name"))
                .and_then(|t| t.as_str().map(|s| s.to_string()))
                .unwrap_or(runner_data.name.clone());
            let workflow_description = document
                .as_ref()
                .and_then(|d| d.get("summary"))
                .and_then(|d| d.as_str().map(|s| s.to_string()))
                .unwrap_or_default();

            let settings = serde_json::json!({
                "json_data": workflow_definition.to_string()
            });
            let runner_settings_descriptor =
                JobworkerpProto::parse_runner_settings_schema_descriptor(&runner_data).map_err(
                    |e| {
                        anyhow::anyhow!(
                            "Failed to parse runner_settings schema descriptor: {:#?}",
                            e
                        )
                    },
                )?;
            let runner_settings = if let Some(ope_desc) = runner_settings_descriptor {
                tracing::debug!("runner settings schema exists: {:#?}", &settings);
                JobworkerpProto::json_value_to_message(ope_desc, &settings, true).map_err(|e| {
                    anyhow::anyhow!("Failed to parse runner_settings schema: {:#?}", e)
                })?
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
            let worker = self
                .jobworkerp_client
                .find_or_create_worker(empty_cx, empty, &data)
                .await;
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
        } else {
            tracing::warn!("Workflow data is not found");
            Err(anyhow::anyhow!(
                "Workflow creation requires a workflow json arguments.",
            ))
        }
    }

    pub async fn prepare_runner_call_arguments(
        request_args: Map<String, Value>,
        runner: &Runner,
        tool_name_opt: Option<String>,
    ) -> (Option<Value>, Value) {
        let settings = request_args.get("settings").cloned();
        let arguments = if runner
            .data
            .as_ref()
            .is_some_and(|r| r.runner_type() == RunnerType::McpServer)
        {
            let mut obj_map = Map::new();
            obj_map.insert(
                "tool_name".to_string(),
                serde_json::to_value(tool_name_opt).unwrap_or(Value::Null),
            );
            obj_map.insert(
                "arg_json".to_string(),
                Value::String(
                    serde_json::to_string(&request_args)
                        .inspect_err(|e| tracing::error!("Failed to parse settings as json: {}", e))
                        .unwrap_or_default(),
                ),
            );
            Value::Object(obj_map)
        } else {
            request_args
                .get("arguments")
                .cloned()
                .unwrap_or(Value::Null)
        };

        tracing::debug!(
            "runner settings: {:#?}, arguments: {:#?}",
            settings,
            arguments
        );

        (settings, arguments)
    }

    pub async fn setup_worker_and_enqueue_with_json(
        &self,
        runner: &Runner,
        request_args: Map<String, Value>,
        tool_name_opt: Option<String>,
    ) -> Result<Value> {
        let empty_cx = None;
        let empty = Arc::new(HashMap::new());

        let (settings, arguments) =
            Self::prepare_runner_call_arguments(request_args, &runner, tool_name_opt).await;

        self.jobworkerp_client
            .setup_worker_and_enqueue_with_json(
                empty_cx,
                empty,
                runner.data.as_ref().map(|r| &r.name).unwrap().as_str(),
                settings,
                None,
                arguments,
                self.timeout_sec,
            )
            .await
    }

    pub async fn prepare_worker_call_arguments(
        request_args: Map<String, Value>,
        worker_data: &WorkerData,
        tool_name_opt: Option<String>,
    ) -> Value {
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

        arguments
    }

    pub async fn enqueue_with_json(
        &self,
        worker_data: &WorkerData,
        request_args: Map<String, Value>,
        tool_name_opt: Option<String>,
    ) -> Result<Value> {
        let empty_cx = None;
        let empty = Arc::new(HashMap::new());

        let arguments =
            Self::prepare_worker_call_arguments(request_args, &worker_data, tool_name_opt).await;

        self.jobworkerp_client
            .enqueue_with_json(empty_cx, empty, worker_data, arguments, self.timeout_sec)
            .await
    }

    pub async fn find_function_list(
        &self,
        exclude_runner_as_tool: bool,
        exclude_worker_as_tool: bool,
    ) -> Result<Vec<FunctionSpecs>> {
        let empty_cx = None;
        let empty = Arc::new(HashMap::new());

        self.jobworkerp_client
            .find_function_list(
                empty_cx,
                empty,
                exclude_runner_as_tool,
                exclude_worker_as_tool,
            )
            .await
    }

    pub async fn find_function_list_by_set(&self, name: &str) -> Result<Vec<FunctionSpecs>> {
        let empty_cx = None;
        let empty = Arc::new(HashMap::new());

        self.jobworkerp_client
            .find_function_list_by_set(empty_cx, empty, name)
            .await
    }
}
