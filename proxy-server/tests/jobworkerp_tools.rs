// テスト用のモックデータと変換関数のテスト
#[cfg(test)]
mod tests {
    use super::*;
    use futures::TryFutureExt;
    use jobworkerp_client::jobworkerp::data::RunnerType;
    use jobworkerp_client::jobworkerp::function::data::{
        function_specs, FunctionSchema, FunctionSpecs, McpTool, McpToolList,
    };
    use proxy_server::jobworkerp::{JobworkerpRouter, JobworkerpRouterConfig};
    use std::sync::Arc;

    async fn make_router() -> JobworkerpRouter {
        JobworkerpRouter::new(JobworkerpRouterConfig {
            jobworkerp_address: "http://localhost:9000".to_string(),
            request_timeout_sec: None,
            exclude_runner_as_tool: false,
            exclude_worker_as_tool: false,
            set_name: None,
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn test_convert_functions_to_tools_reusable_workflow() {
        let func = FunctionSpecs {
            runner_type: RunnerType::ReusableWorkflow as i32,
            runner_id: Some(Default::default()),
            worker_id: None,
            name: "workflow1".to_string(),
            description: "desc".to_string(),
            output_type: 0,
            schema: Some(function_specs::Schema::SingleSchema(FunctionSchema {
                settings: Some("{\"type\":\"object\"}".to_string()),
                arguments: "{\"type\":\"object\"}".to_string(),
                result_output_schema: None,
            })),
        };
        let tools = JobworkerpRouter::convert_functions_to_tools(vec![func]).unwrap();
        assert_eq!(tools.tools.len(), 1);
        assert_eq!(tools.tools[0].name, "workflow1");
    }

    #[tokio::test]
    async fn test_convert_functions_to_tools_mcp_server() {
        let mcp_tool = McpTool {
            name: "toolA".to_string(),
            description: Some("descA".to_string()),
            input_schema: "{\"type\":\"object\"}".to_string(),
            annotations: None,
        };
        let func = FunctionSpecs {
            runner_type: RunnerType::McpServer as i32,
            runner_id: Some(Default::default()),
            worker_id: None,
            name: "server1".to_string(),
            description: "desc".to_string(),
            output_type: 0,
            schema: Some(function_specs::Schema::McpTools(McpToolList {
                list: vec![mcp_tool],
            })),
        };
        let tools = JobworkerpRouter::convert_functions_to_tools(vec![func]).unwrap();
        assert_eq!(tools.tools.len(), 1);
        assert!(tools.tools[0].name.contains("server1"));
        assert!(tools.tools[0].name.contains("toolA"));
    }

    #[tokio::test]
    async fn test_convert_functions_to_tools_normal_function() {
        let func = FunctionSpecs {
            runner_type: RunnerType::Command as i32,
            runner_id: Some(Default::default()),
            worker_id: None,
            name: "cmd1".to_string(),
            description: "desc".to_string(),
            output_type: 0,
            schema: Some(function_specs::Schema::SingleSchema(FunctionSchema {
                settings: Some("{\"type\":\"object\"}".to_string()),
                arguments: "{\"type\":\"object\"}".to_string(),
                result_output_schema: None,
            })),
        };
        let tools = JobworkerpRouter::convert_functions_to_tools(vec![func]).unwrap();
        assert_eq!(tools.tools.len(), 1);
        assert_eq!(tools.tools[0].name, "cmd1");
    }
}
