use crate::common::jsonrpc::SchemaCombiner;
use jobworkerp_client::jobworkerp::data::RunnerType;
use jobworkerp_client::jobworkerp::function::data::{function_specs, FunctionSpecs, McpToolList};
use rmcp::model::{ListToolsResult, Tool};
use rmcp::Error as McpError;
use serde_json;
use tracing;
pub const CREATION_TOOL_DESCRIPTION: &str =
    "Create Tools from workflow definitions provided as JSON. The workflow definition must:

- Conform to the specified JSON schema
- Include an input schema section that defines the parameters created workflow Tool will accept
- When this workflow is executed as a Tool, it will receive parameters matching this input schema
- Specify execution steps that utilize any available runner(function) in the system (except this creation Tool)";


pub struct ToolConverter;

impl ToolConverter {
    const DELIMITER: &str = "___";
    pub fn combine_names(server_name: &str, tool_name: &str) -> String {
        format!("{}{}{}", server_name, Self::DELIMITER, tool_name)
    }

    pub fn divide_names(combined: &str) -> Option<(String, String)> {
        let delimiter = Self::DELIMITER;
        let mut v: std::collections::VecDeque<&str> = combined.split(delimiter).collect();
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
                        .reduce(|acc, n| format!("{}{}{}", acc, delimiter, n))
                        .unwrap_or_default(),
                ))
            }
        }
    }

    pub fn convert_reusable_workflow(tool: &FunctionSpecs) -> Option<Tool> {
        Some(Tool::new(
            tool.name.clone(),
            CREATION_TOOL_DESCRIPTION,
            tool.schema
                .as_ref()
                .and_then(|s| match s {
                    function_specs::Schema::SingleSchema(function) => {
                        function.settings.as_ref().and_then(|f| {
                            serde_json::from_str(f.as_str())
                                .or_else(|e1| {
                                    tracing::warn!("Failed to parse settings as json: {}", e1);
                                    serde_yaml::from_str(f.as_str()).inspect_err(|e2| {
                                        tracing::warn!("Failed to parse settings as yaml: {}", e2);
                                    })
                                })
                                .ok()
                        })
                    }
                    function_specs::Schema::McpTools(mcp) => {
                        let mes = format!("error: expect workflow but got mcp: {:?}", mcp);
                        tracing::error!(mes);
                        None
                    }
                })
                .unwrap_or(serde_json::json!({}))
                .as_object()
                .cloned()
                .unwrap_or_default(),
        ))
    }

    pub fn convert_mcp_server(tool: &FunctionSpecs) -> Vec<Tool> {
        let server_name = tool.name.as_str();
        match &tool.schema {
            Some(function_specs::Schema::McpTools(McpToolList { list })) => list
                .iter()
                .map(|tool| {
                    Tool::new(
                        Self::combine_names(server_name, tool.name.as_str()),
                        tool.description.clone().unwrap_or_default(),
                        serde_json::from_str(tool.input_schema.as_str())
                            .unwrap_or(serde_json::json!({}))
                            .as_object()
                            .cloned()
                            .unwrap_or_default(),
                    )
                })
                .collect(),
            Some(function_specs::Schema::SingleSchema(function)) => {
                tracing::error!("error: expect workflow but got function: {:?}", function);
                vec![]
            }
            None => {
                tracing::error!("error: expect workflow but got none: {:?}", &tool);
                vec![]
            }
        }
    }

    pub fn convert_normal_function(tool: &FunctionSpecs) -> Option<Tool> {
        let mut schema_combiner = SchemaCombiner::new();
        tool.schema
            .as_ref()
            .and_then(|s| match s {
                function_specs::Schema::SingleSchema(function) => function.settings.clone(),
                function_specs::Schema::McpTools(_) => {
                    tracing::error!("got mcp tool in not mcp tool runner type: {:#?}", &tool);
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
                    .inspect_err(|e| tracing::error!("Failed to parse schema: {}", e))
                    .ok()
            });
        tool.schema
            .as_ref()
            .map(|s| match s {
                function_specs::Schema::SingleSchema(function) => function.arguments.clone(),
                function_specs::Schema::McpTools(_) => {
                    tracing::error!("got mcp tool in not mcp tool runner type: {:#?}", &tool);
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
                    .inspect_err(|e| tracing::error!("Failed to parse schema: {}", e))
                    .ok()
            });
        match schema_combiner.generate_combined_schema() {
            Ok(schema) => Some(Tool::new(
                tool.name.clone(),
                tool.description.clone(),
                schema,
            )),
            Err(e) => {
                tracing::error!("Failed to generate schema: {}", e);
                None
            }
        }
    }

    pub fn convert_functions_to_mcp_tools(
        functions: Vec<FunctionSpecs>,
    ) -> Result<ListToolsResult, McpError> {
        let tool_list = functions
            .into_iter()
            .flat_map(|tool| {
                if tool.worker_id.is_none()
                    && tool.runner_type == RunnerType::ReusableWorkflow as i32
                {
                    Self::convert_reusable_workflow(&tool)
                        .into_iter()
                        .collect::<Vec<_>>()
                } else if tool.runner_type == RunnerType::McpServer as i32 {
                    Self::convert_mcp_server(&tool)
                } else {
                    Self::convert_normal_function(&tool)
                        .into_iter()
                        .collect::<Vec<_>>()
                }
            })
            .collect::<Vec<_>>();

        Ok(ListToolsResult {
            tools: tool_list,
            next_cursor: None,
        })
    }
}
