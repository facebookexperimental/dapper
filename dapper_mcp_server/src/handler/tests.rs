// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_dap_protocol::responses::ReadMemoryResponseBody;
use serde_json::Value;
use serde_json::from_value;
use serde_json::json;
use serde_json::to_value;

use super::format::parse_address;
use super::params::BreakpointSpec;
use super::params::MAX_STACK_DEPTH;
use super::params::MAX_THREADS_HARD_CAP;
use super::params::MAX_WRITE_BYTES;
use super::*;

#[test]
fn breakpoint_spec_from_object() {
    let spec: BreakpointSpec = from_value(json!({"line": 71})).unwrap();
    assert_eq!(spec.line, 71);
    assert_eq!(spec.condition, None);
    assert_eq!(spec.log_message, None);
}

#[test]
fn breakpoint_spec_from_object_with_options() {
    let spec: BreakpointSpec = from_value(json!({
        "line": 10,
        "condition": "x > 5",
        "logMessage": "val is {x}"
    }))
    .unwrap();
    assert_eq!(spec.line, 10);
    assert_eq!(spec.condition, Some("x > 5".to_string()));
    assert_eq!(spec.log_message, Some("val is {x}".to_string()));
}

#[test]
fn breakpoint_spec_from_stringified_json() {
    let spec: BreakpointSpec = from_value(json!("{\"line\": 71}")).unwrap();
    assert_eq!(spec.line, 71);
    assert_eq!(spec.condition, None);
}

#[test]
fn breakpoint_spec_from_stringified_json_with_condition() {
    let spec: BreakpointSpec =
        from_value(json!("{\"line\": 10, \"condition\": \"x > 5\"}")).unwrap();
    assert_eq!(spec.line, 10);
    assert_eq!(spec.condition, Some("x > 5".to_string()));
}

#[test]
fn breakpoint_spec_from_integer() {
    let spec: BreakpointSpec = from_value(json!(42)).unwrap();
    assert_eq!(spec.line, 42);
    assert_eq!(spec.condition, None);
}

#[test]
fn breakpoint_spec_from_string_integer() {
    let spec: BreakpointSpec = from_value(json!("71")).unwrap();
    assert_eq!(spec.line, 71);
    assert_eq!(spec.condition, None);
}

#[test]
fn breakpoint_spec_vec_mixed_formats() {
    let specs: Vec<BreakpointSpec> = from_value(json!([
        {"line": 10},
        "{\"line\": 20}",
        30,
        "40"
    ]))
    .unwrap();
    assert_eq!(specs.len(), 4);
    assert_eq!(specs[0].line, 10);
    assert_eq!(specs[1].line, 20);
    assert_eq!(specs[2].line, 30);
    assert_eq!(specs[3].line, 40);
}

#[test]
fn breakpoint_spec_rejects_invalid_string() {
    let result: Result<BreakpointSpec, _> = from_value(json!("not_a_number"));
    assert!(result.is_err());
}

#[test]
fn breakpoint_spec_schema_shape() {
    let schema =
        schemars::generate::SchemaGenerator::default().into_root_schema_for::<BreakpointSpec>();
    let json = to_value(&schema).unwrap();
    let props = json["properties"].as_object().unwrap();
    assert!(props.contains_key("line"));
    assert!(props.contains_key("condition"));
    assert!(props.contains_key("logMessage"));
    let required = json["required"].as_array().unwrap();
    assert!(required.contains(&json!("line")));
    assert!(!required.contains(&json!("condition")));
    assert!(!required.contains(&json!("logMessage")));
}

/// A store at a unique empty directory, so tests can never see — let
/// alone mutate — real sessions on the developer's machine.
fn isolated_store() -> SessionStore {
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    SessionStore::at(std::env::temp_dir().join(format!(
        "dapper-mcp-test-sessions-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )))
}

/// A handler environment that cannot touch the developer's real sessions
/// or configuration.
fn isolated_env() -> McpServerEnv {
    McpServerEnv {
        control_port: None,
        scope_id: None,
        sessions: isolated_store(),
        config: DapperConfig::default(),
    }
}

fn full_toolset_handler() -> McpHandler {
    let toolset = crate::toolsets::Toolset::from(crate::toolsets::BuiltinToolset::Full);
    McpHandler::new(isolated_env(), &toolset)
}

/// The cache-first fast path in `get_client` must return the cached client
/// without a sessions-directory scan when the requested session matches the
/// cached one *and the cached session is still alive* — both for an implicit
/// session (via `last_session_id`) and an explicit matching `session_id`. A
/// dead cached session must fall through to the slow path instead of serving
/// a stale client, so that fallback to another active session still works.
#[test]
fn get_client_fast_path_respects_cached_session_liveness() {
    use std::net::TcpListener;

    let handler = full_toolset_handler(); // control_port = None
    let sid = SessionId::from("fast-path-session");
    let client = Arc::new(DapperControlPlaneClient::discover(isolated_store(), None));

    // A live cached session: hold a listener so the port probe reports the
    // port as occupied, and use the current pid (via generate) so the
    // process looks alive -> is_active() == true.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = Port::try_new(listener.local_addr().unwrap().port()).unwrap();
    let live = SessionInfo::generate(sid.clone(), Some(port), None, None, None);
    assert!(
        live.is_active(),
        "precondition: seeded session must look active"
    );
    *handler.cached_client.write().unwrap() = Some(CachedClient {
        client: Arc::clone(&client),
        session: live,
    });
    *handler.last_session_id.lock().unwrap() = Some(sid.clone());

    // Implicit (via last_session_id) and explicit matching id both hit cache
    // and return the exact cached client without touching the filesystem.
    let got = handler
        .get_client(None)
        .expect("implicit live session should hit the cache");
    assert!(
        Arc::ptr_eq(&got, &client),
        "implicit session should return the cached client"
    );
    let got = handler
        .get_client(Some(&sid))
        .expect("explicit live session should hit the cache");
    assert!(
        Arc::ptr_eq(&got, &client),
        "explicit session should return the cached client"
    );

    // The fast path must not mutate last_session_id (only resolve_session does).
    assert_eq!(
        *handler.last_session_id.lock().unwrap(),
        Some(sid.clone()),
        "fast-path hit must leave last_session_id unchanged"
    );

    // A dead cached session (no reachable port -> not active): the fast path
    // must fall through. With an explicit id that resolves to no active
    // session, the slow path errors rather than serving the stale client.
    let dead = SessionInfo::generate(sid.clone(), None, None, None, None);
    assert!(
        !dead.is_active(),
        "precondition: seeded session must look dead"
    );
    *handler.cached_client.write().unwrap() = Some(CachedClient {
        client: Arc::clone(&client),
        session: dead,
    });
    assert!(
        handler.get_client(Some(&sid)).is_err(),
        "dead cached session must not be served from cache"
    );

    drop(listener);
}

/// When a fixed `control_port` is configured, `get_client` must NOT use the
/// cache-first fast path (that path keys on session_id, but a port-configured
/// handler resolves by port). It must fall through to `resolve_session`, which
/// resolves by port and errors here since no session listens on the port.
#[test]
fn get_client_skips_fast_path_when_control_port_set() {
    use std::net::TcpListener;

    let toolset = crate::toolsets::Toolset::from(crate::toolsets::BuiltinToolset::Full);
    // Port 1: no dapper session listens here, so resolve-by-port fails.
    let handler = McpHandler::new(
        McpServerEnv {
            control_port: Some(Port::try_new(1).unwrap()),
            ..isolated_env()
        },
        &toolset,
    );
    let sid = SessionId::from("control-port-session");
    let client = Arc::new(DapperControlPlaneClient::discover(isolated_store(), None));

    // Seed a *live* matching cached session; if the fast path were taken it
    // would return this client. The control_port gate must prevent that.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = Port::try_new(listener.local_addr().unwrap().port()).unwrap();
    let live = SessionInfo::generate(sid.clone(), Some(port), None, None, None);
    assert!(
        live.is_active(),
        "precondition: seeded session must look active"
    );
    *handler.cached_client.write().unwrap() = Some(CachedClient {
        client: Arc::clone(&client),
        session: live,
    });
    *handler.last_session_id.lock().unwrap() = Some(sid.clone());

    // control_port is Some -> fast path skipped -> resolve_session by port -> Err.
    assert!(
        handler.get_client(None).is_err(),
        "control_port handler must bypass the fast path and resolve by port"
    );
    assert!(
        handler.get_client(Some(&sid)).is_err(),
        "control_port handler must bypass the fast path even with explicit id"
    );

    drop(listener);
}

#[test]
fn all_tool_schemas_have_properties() {
    // Tools that take no parameters are allowed to have no properties
    let no_params_tools: &[&str] = &["debug_sessions_command"];
    let handler = full_toolset_handler();
    for (name, route) in &handler.tool_router.map {
        if no_params_tools.contains(&name.as_ref()) {
            continue;
        }
        let schema = &route.attr.input_schema;
        assert!(
            schema
                .get("properties")
                .and_then(|p| p.as_object())
                .is_some(),
            "tool '{}' schema has no 'properties' object",
            name
        );
    }
}

#[test]
fn no_degenerate_schemas() {
    let handler = full_toolset_handler();
    for (name, route) in &handler.tool_router.map {
        let schema = Value::Object(route.attr.input_schema.as_ref().clone());
        check_schema_tree(&schema, name, "root");
    }
}

fn check_schema_tree(schema: &Value, tool_name: &str, path: &str) {
    assert!(
        !is_degenerate_schema(schema),
        "tool '{}' has degenerate schema at {}: {}",
        tool_name,
        path,
        schema
    );

    let Some(obj) = schema.as_object() else {
        return;
    };

    for key in &[
        "$defs",
        "properties",
        "patternProperties",
        "dependentSchemas",
    ] {
        if let Some(map) = obj.get(*key).and_then(|v| v.as_object()) {
            for (entry_name, entry_schema) in map {
                check_schema_tree(
                    entry_schema,
                    tool_name,
                    &format!("{}/{}/{}", path, key, entry_name),
                );
            }
        }
    }

    for key in &[
        "items",
        "not",
        "if",
        "then",
        "else",
        "additionalProperties",
        "propertyNames",
        "contains",
    ] {
        if let Some(sub) = obj.get(*key) {
            if sub.is_boolean() {
                continue;
            }
            check_schema_tree(sub, tool_name, &format!("{}/{}", path, key));
        }
    }

    for key in &["oneOf", "anyOf", "allOf", "prefixItems"] {
        if let Some(items) = obj.get(*key).and_then(|v| v.as_array()) {
            for (i, item) in items.iter().enumerate() {
                check_schema_tree(item, tool_name, &format!("{}/{}[{}]", path, key, i));
            }
        }
    }
}

#[test]
fn no_type_arrays_in_schemas() {
    // The Claude API does not accept "type": ["string", "null"] (the JSON Schema
    // 2020-12 array form for nullable types). It requires the equivalent "anyOf"
    // form instead. This test ensures no tool schema contains type arrays, which
    // would cause HTTP 500 errors from the Claude API.
    let handler = full_toolset_handler();
    for (name, route) in &handler.tool_router.map {
        let schema = Value::Object(route.attr.input_schema.as_ref().clone());
        assert_no_type_arrays(&schema, name, "root");
    }
}

fn assert_no_type_arrays(schema: &Value, tool_name: &str, path: &str) {
    let Some(obj) = schema.as_object() else {
        return;
    };

    if let Some(type_val) = obj.get("type") {
        assert!(
            !type_val.is_array(),
            "tool '{}' has array-valued 'type' at {}: {} — \
             use anyOf instead for Claude API compatibility",
            tool_name,
            path,
            type_val
        );
    }

    // Recurse using the same subschema positions as check_schema_tree
    for key in &[
        "$defs",
        "properties",
        "patternProperties",
        "dependentSchemas",
    ] {
        if let Some(map) = obj.get(*key).and_then(|v| v.as_object()) {
            for (entry_name, entry_schema) in map {
                assert_no_type_arrays(
                    entry_schema,
                    tool_name,
                    &format!("{}/{}/{}", path, key, entry_name),
                );
            }
        }
    }

    for key in &["items", "not", "if", "then", "else", "additionalProperties"] {
        if let Some(sub) = obj.get(*key)
            && sub.is_object()
        {
            assert_no_type_arrays(sub, tool_name, &format!("{}/{}", path, key));
        }
    }

    for key in &["oneOf", "anyOf", "allOf", "prefixItems"] {
        if let Some(items) = obj.get(*key).and_then(|v| v.as_array()) {
            for (i, item) in items.iter().enumerate() {
                assert_no_type_arrays(item, tool_name, &format!("{}/{}[{}]", path, key, i));
            }
        }
    }
}

#[test]
fn sessions_tool_always_available() {
    for builtin in &[
        crate::toolsets::BuiltinToolset::Minimal,
        crate::toolsets::BuiltinToolset::Standard,
        crate::toolsets::BuiltinToolset::Full,
        crate::toolsets::BuiltinToolset::Raw,
    ] {
        let toolset = crate::toolsets::Toolset::from(*builtin);
        let handler = McpHandler::new(isolated_env(), &toolset);
        assert!(
            handler
                .tool_router
                .map
                .contains_key("debug_sessions_command"),
            "debug_sessions_command should be available in {:?} toolset",
            builtin
        );
    }
}

#[test]
fn sessions_tool_not_in_any_toolset_definition() {
    // Sessions is always-available, not part of any toolset definition
    for builtin in &[
        crate::toolsets::BuiltinToolset::Minimal,
        crate::toolsets::BuiltinToolset::Standard,
        crate::toolsets::BuiltinToolset::Full,
        crate::toolsets::BuiltinToolset::Raw,
    ] {
        let tools = builtin.tools();
        assert!(
            !tools.contains(&crate::toolsets::DebugTool::Sessions),
            "Sessions should not be in {:?} toolset definition",
            builtin
        );
    }
}

fn is_degenerate_schema(schema: &Value) -> bool {
    match schema {
        Value::Bool(b) => *b,
        Value::Object(obj) => {
            let meaningful_keys = [
                "type",
                "properties",
                "patternProperties",
                "additionalProperties",
                "propertyNames",
                "dependentSchemas",
                "$ref",
                "oneOf",
                "anyOf",
                "allOf",
                "not",
                "if",
                "then",
                "else",
                "enum",
                "const",
                "items",
                "prefixItems",
                "contains",
                "$defs",
            ];
            !meaningful_keys.iter().any(|k| obj.contains_key(*k))
        }
        _ => false,
    }
}

/// Send a `tools/call` MCP request through a full in-process MCP
/// client/server pair (using `tokio::io::duplex`), returning the
/// result or error exactly as a real MCP client would see it.
async fn call_tool_e2e(
    tool_name: impl Into<String>,
    arguments: Value,
) -> Result<CallToolResult, rmcp::service::ServiceError> {
    let tool_name: String = tool_name.into();
    use rmcp::ClientHandler;
    use rmcp::ServiceExt;
    use rmcp::model::ClientInfo;

    #[derive(Debug, Clone, Default)]
    struct DummyClientHandler;
    impl ClientHandler for DummyClientHandler {
        fn get_info(&self) -> ClientInfo {
            ClientInfo::default()
        }
    }

    let (server_transport, client_transport) = tokio::io::duplex(4096);

    let handler = full_toolset_handler();
    let server_handle = tokio::spawn(async move {
        handler.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClientHandler.serve(client_transport).await.unwrap();

    let result = client
        .call_tool(
            CallToolRequestParams::new(tool_name).with_arguments(
                arguments
                    .as_object()
                    .expect("test input must be a JSON object")
                    .clone(),
            ),
        )
        .await;

    client.cancel().await.ok();
    server_handle.abort();

    result
}

/// Assert that a `call_tool_e2e` result did not fail with a
/// deserialization error (MCP error code -32602). The tool itself
/// may return an application-level error (e.g. "no active session")
/// which is fine — we only care that parameter parsing succeeded.
fn assert_params_accepted(
    result: &Result<CallToolResult, rmcp::service::ServiceError>,
    context: &str,
) {
    match result {
        Err(rmcp::service::ServiceError::McpError(e)) if e.code.0 == -32602 => {
            panic!("{context}: parameter deserialization failed: {}", e.message);
        }
        _ => {} // Ok or any non-deserialization error is fine
    }
}

// -- stack_trace: ThreadId, Option<i64> levels, Option<i64> start_frame --

#[tokio::test]
async fn stack_trace_integer_values() {
    let result = call_tool_e2e(
        "debug_stack_trace_command",
        json!({"thread_id": 4743, "levels": 40}),
    )
    .await;
    assert_params_accepted(&result, "integer thread_id + integer levels");
}

#[tokio::test]
async fn stack_trace_string_thread_id() {
    let result = call_tool_e2e("debug_stack_trace_command", json!({"thread_id": "4743"})).await;
    assert_params_accepted(&result, "string thread_id");
}

#[tokio::test]
async fn stack_trace_string_levels() {
    let result = call_tool_e2e(
        "debug_stack_trace_command",
        json!({"thread_id": 4743, "levels": "40"}),
    )
    .await;
    assert_params_accepted(&result, "string levels");
}

#[tokio::test]
async fn stack_trace_string_start_frame() {
    let result = call_tool_e2e(
        "debug_stack_trace_command",
        json!({"thread_id": 4743, "start_frame": "5"}),
    )
    .await;
    assert_params_accepted(&result, "string start_frame");
}

#[tokio::test]
async fn stack_trace_all_strings() {
    let result = call_tool_e2e(
        "debug_stack_trace_command",
        json!({"thread_id": "4743", "levels": "40", "start_frame": "5"}),
    )
    .await;
    assert_params_accepted(&result, "all string values");
}

// -- navigate: ThreadId --

#[tokio::test]
async fn navigate_string_thread_id() {
    let result = call_tool_e2e(
        "debug_navigate_command",
        json!({"thread_id": "1", "navigation_type": "continue"}),
    )
    .await;
    assert_params_accepted(&result, "string thread_id in navigate");
}

#[tokio::test]
async fn navigate_step_back() {
    let result = call_tool_e2e(
        "debug_navigate_command",
        json!({"thread_id": 1, "navigation_type": "step_back"}),
    )
    .await;
    assert_params_accepted(&result, "step_back navigation_type");
}

#[tokio::test]
async fn navigate_reverse_continue() {
    let result = call_tool_e2e(
        "debug_navigate_command",
        json!({"thread_id": 1, "navigation_type": "reverse_continue"}),
    )
    .await;
    assert_params_accepted(&result, "reverse_continue navigation_type");
}

// -- scopes: FrameId --

#[tokio::test]
async fn scopes_string_frame_id() {
    let result = call_tool_e2e("debug_scopes_command", json!({"frame_id": "42"})).await;
    assert_params_accepted(&result, "string frame_id in scopes");
}

// -- variables: VariablesReference --

#[tokio::test]
async fn variables_string_variables_reference() {
    let result = call_tool_e2e(
        "debug_variables_command",
        json!({"variables_reference": "100"}),
    )
    .await;
    assert_params_accepted(&result, "string variables_reference");
}

// -- set_variable: VariablesReference --

#[tokio::test]
async fn set_variable_string_variables_reference() {
    let result = call_tool_e2e(
        "debug_set_variable_command",
        json!({"variables_reference": "100", "name": "x", "value": "42"}),
    )
    .await;
    assert_params_accepted(&result, "string variables_reference in set_variable");
}

// -- evaluate: Option<FrameId> --

#[tokio::test]
async fn evaluate_string_frame_id() {
    let result = call_tool_e2e(
        "debug_evaluate_command",
        json!({"expression": "1+1", "frame_id": "5"}),
    )
    .await;
    assert_params_accepted(&result, "string frame_id in evaluate");
}

#[tokio::test]
async fn evaluate_integer_frame_id() {
    let result = call_tool_e2e(
        "debug_evaluate_command",
        json!({"expression": "1+1", "frame_id": 5}),
    )
    .await;
    assert_params_accepted(&result, "integer frame_id in evaluate");
}

#[tokio::test]
async fn evaluate_no_frame_id() {
    let result = call_tool_e2e("debug_evaluate_command", json!({"expression": "1+1"})).await;
    assert_params_accepted(&result, "omitted frame_id in evaluate");
}

// -- set_breakpoints: BreakpointSpec.line --

#[tokio::test]
async fn breakpoints_string_line() {
    let result = call_tool_e2e(
        "debug_set_breakpoints_command",
        json!({"source_path": "/tmp/test.py", "breakpoints": [{"line": "10"}]}),
    )
    .await;
    assert_params_accepted(&result, "string line in breakpoint spec");
}

#[tokio::test]
async fn breakpoints_integer_line() {
    let result = call_tool_e2e(
        "debug_set_breakpoints_command",
        json!({"source_path": "/tmp/test.py", "breakpoints": [{"line": 10}]}),
    )
    .await;
    assert_params_accepted(&result, "integer line in breakpoint spec");
}

// -- set_exception_breakpoints --

#[tokio::test]
async fn set_exception_breakpoints_filters_only() {
    let result = call_tool_e2e(
        "debug_set_exception_breakpoints_command",
        json!({"filters": ["raised"]}),
    )
    .await;
    assert_params_accepted(&result, "filters list with default clear_existing");
}

#[tokio::test]
async fn set_exception_breakpoints_with_clear_existing() {
    let result = call_tool_e2e(
        "debug_set_exception_breakpoints_command",
        json!({"filters": ["raised", "uncaught"], "clear_existing": true}),
    )
    .await;
    assert_params_accepted(&result, "filters + clear_existing");
}

#[tokio::test]
async fn set_exception_breakpoints_rejects_empty_filters_without_clear() {
    // Strict empty-input validation: empty filters + !clear is a
    // user/agent mistake, the tool must reject before reaching the
    // library no-op.
    let result = call_tool_e2e(
        "debug_set_exception_breakpoints_command",
        json!({"filters": [], "clear_existing": false}),
    )
    .await
    .expect("call should succeed at the MCP layer");
    assert_eq!(
        result.is_error,
        Some(true),
        "expected tool error for empty filters + !clear_existing"
    );
    let text = match result.content.first() {
        Some(c) => match &c.raw {
            rmcp::model::RawContent::Text(t) => t.text.as_str(),
            other => panic!("expected text content, got {other:?}"),
        },
        None => panic!("expected at least one content block"),
    };
    assert!(
        text.contains("specify at least one filter") && text.contains("clear_existing: true"),
        "expected explanatory error message; got: {text}"
    );
}

#[tokio::test]
async fn set_exception_breakpoints_accepts_empty_filters_with_clear() {
    // Clear-all path: empty filters + clear_existing: true is the
    // documented way to disable all exception breakpoints. Should
    // pass parameter validation and reach the library.
    let result = call_tool_e2e(
        "debug_set_exception_breakpoints_command",
        json!({"filters": [], "clear_existing": true}),
    )
    .await;
    assert_params_accepted(&result, "empty filters + clear_existing: true (clear-all)");
}

// -- format_capabilities: exceptionBreakpointFilters extension --

#[test]
fn format_capabilities_renders_exception_filters() {
    let value = json!({
        "supportsStepBack": true,
        "exceptionBreakpointFilters": [
            {"filter": "uncaught", "label": "Uncaught", "default": true, "supportsCondition": true},
            {"filter": "raised", "label": "Raised"}
        ]
    });
    let rendered = format_capabilities(&value);
    // Bool capabilities come first.
    assert!(rendered.contains("Supported capabilities:\n  - supportsStepBack\n"));
    // Exception filters section follows, sorted by filter id.
    assert!(rendered.contains("Exception breakpoint filters:\n"));
    let raised_pos = rendered.find("- raised").expect("raised line missing");
    let uncaught_pos = rendered.find("- uncaught").expect("uncaught line missing");
    assert!(
        raised_pos < uncaught_pos,
        "filters should be sorted by id; got:\n{rendered}"
    );
    // Annotations are present where expected.
    assert!(
        rendered
            .contains("- uncaught (label: \"Uncaught\", default: true, supports_condition: true)"),
        "expected uncaught annotations: {rendered}"
    );
    assert!(
        rendered.contains("- raised (label: \"Raised\")"),
        "expected raised annotations: {rendered}"
    );
}

#[test]
fn format_capabilities_omits_section_when_array_missing() {
    let value = json!({"supportsStepBack": true});
    let rendered = format_capabilities(&value);
    assert!(!rendered.contains("Exception breakpoint filters:"));
}

#[test]
fn format_capabilities_omits_section_when_array_empty() {
    let value = json!({
        "supportsStepBack": true,
        "exceptionBreakpointFilters": []
    });
    let rendered = format_capabilities(&value);
    assert!(!rendered.contains("Exception breakpoint filters:"));
}

#[test]
fn format_capabilities_only_exception_filters_no_supported_caps() {
    let value = json!({
        "exceptionBreakpointFilters": [{"filter": "raised"}]
    });
    let rendered = format_capabilities(&value);
    // No "Supported capabilities:" preamble when there are no bool caps.
    assert!(!rendered.contains("Supported capabilities:"));
    assert!(rendered.contains("Exception breakpoint filters:\n  - raised\n"));
}

// -- parse_address: hex with prefix, decimal without, junk --

#[test]
fn parse_address_hex_prefix() {
    assert_eq!(parse_address("0xDEADBEEF"), Some(0xDEAD_BEEF));
    assert_eq!(parse_address("0XdeadBEEF"), Some(0xDEAD_BEEF));
    assert_eq!(parse_address("0x0"), Some(0));
    assert_eq!(parse_address("0x7fff5fbff8a0"), Some(0x7fff5fbff8a0));
}

#[test]
fn parse_address_decimal_no_prefix() {
    // Per DAP spec, no 0x/0X prefix means decimal — "4660" must NOT parse as 0x4660.
    assert_eq!(parse_address("4660"), Some(4660));
    assert_eq!(parse_address("0"), Some(0));
}

#[test]
fn parse_address_invalid_returns_none() {
    assert_eq!(parse_address(""), None);
    assert_eq!(parse_address("0xZZ"), None);
    assert_eq!(parse_address("not a number"), None);
    assert_eq!(parse_address("12.34"), None);
}

// -- hex_string_to_bytes: prefixes, parity, ASCII, payload cap, parse failures --

#[test]
fn hex_string_to_bytes_happy() {
    assert_eq!(hex_string_to_bytes("48656C6C6F").unwrap(), b"Hello");
    assert_eq!(hex_string_to_bytes("").unwrap(), Vec::<u8>::new());
}

#[test]
fn hex_string_to_bytes_with_prefix() {
    assert_eq!(hex_string_to_bytes("0x4142").unwrap(), b"AB");
    assert_eq!(hex_string_to_bytes("0X4142").unwrap(), b"AB");
}

#[test]
fn hex_string_to_bytes_rejects_odd_length() {
    let err = hex_string_to_bytes("123").unwrap_err().to_string();
    assert!(err.contains("even number"), "got: {err}");
}

#[test]
fn hex_string_to_bytes_rejects_non_ascii() {
    let err = hex_string_to_bytes("é4").unwrap_err().to_string();
    assert!(err.contains("ASCII"), "got: {err}");
}

#[test]
fn hex_string_to_bytes_rejects_bad_digit() {
    let err = hex_string_to_bytes("ZZZZ").unwrap_err().to_string();
    assert!(err.contains("invalid hex"), "got: {err}");
    assert!(err.contains("position 0"), "got: {err}");
}

#[test]
fn hex_string_to_bytes_rejects_oversized_payload() {
    // 2 hex chars = 1 byte; produce MAX_WRITE_BYTES + 1 bytes worth.
    let oversize = "AA".repeat(MAX_WRITE_BYTES + 1);
    let err = hex_string_to_bytes(&oversize).unwrap_err().to_string();
    assert!(err.contains("exceeds maximum"), "got: {err}");
}

// -- format_memory_read: header, ASCII sidebar, multi-chunk, decode failure, no-data --

fn make_body(
    address: &str,
    data: Option<&str>,
    unreadable_bytes: Option<i64>,
) -> ReadMemoryResponseBody {
    ReadMemoryResponseBody {
        address: address.to_string(),
        data: data.map(String::from),
        unreadable_bytes,
        ..Default::default()
    }
}

#[test]
fn format_memory_read_header_and_sidebar() {
    // "Hello World" + 5 nulls = 16 bytes, base64 = "SGVsbG8gV29ybGQAAAAAAA=="
    let body = make_body("0x7fff5fbff8a0", Some("SGVsbG8gV29ybGQAAAAAAA=="), None);
    let out = format_memory_read(&body).unwrap();
    assert!(out.starts_with("Memory at 0x7fff5fbff8a0 (16 bytes):\n"));
    assert!(
        out.contains("Hello World....."),
        "ASCII sidebar mismatch:\n{out}"
    );
    assert!(out.contains("0x00007FFF5FBFF8A0:"));
}

#[test]
fn format_memory_read_short_partial_row_padding() {
    // 5 bytes "Hello" → one short row, must still align ASCII sidebar.
    let body = make_body("0x10", Some("SGVsbG8="), None);
    let out = format_memory_read(&body).unwrap();
    assert!(out.contains("48 65 6C 6C 6F"));
    assert!(out.ends_with("Hello\n"));
}

#[test]
fn format_memory_read_decode_failure_is_err() {
    // "%%%" is not valid base64.
    let body = make_body("0x10", Some("%%%"), None);
    let err = format_memory_read(&body).unwrap_err().to_string();
    assert!(
        err.contains("0x10"),
        "error should reference address: {err}"
    );
}

#[test]
fn format_memory_read_no_data_with_unreadable_bytes() {
    let body = make_body("0x10", None, Some(8));
    let out = format_memory_read(&body).unwrap();
    assert_eq!(out, "Address: 0x10\n8 byte(s) unreadable.");
}

#[test]
fn format_memory_read_no_data_no_unreadable() {
    let body = make_body("0x10", None, None);
    let out = format_memory_read(&body).unwrap();
    assert_eq!(out, "Address: 0x10\nNo data returned.");
}

#[test]
fn format_memory_read_unparseable_address_uses_relative_offsets() {
    // Address that doesn't parse as hex or decimal — must not silently render as 0x0.
    let body = make_body("garbage", Some("SGVsbG8="), None);
    let out = format_memory_read(&body).unwrap();
    assert!(out.contains("Memory at garbage"));
    assert!(
        out.contains("+0x00000000:"),
        "should fall back to relative offsets:\n{out}"
    );
    assert!(!out.contains("0x0000000000000000:"));
}

// -- thread_snapshot: include_stacks, stack_depth, max_threads --

#[tokio::test]
async fn thread_snapshot_no_params() {
    let result = call_tool_e2e("debug_thread_snapshot", json!({})).await;
    assert_params_accepted(&result, "thread_snapshot with no params (all defaults)");
}

#[tokio::test]
async fn thread_snapshot_integer_params() {
    let result = call_tool_e2e(
        "debug_thread_snapshot",
        json!({"include_stacks": true, "stack_depth": 20, "max_threads": 100}),
    )
    .await;
    assert_params_accepted(&result, "thread_snapshot with integer params");
}

#[tokio::test]
async fn thread_snapshot_string_numeric_params() {
    let result = call_tool_e2e(
        "debug_thread_snapshot",
        json!({"stack_depth": "20", "max_threads": "100"}),
    )
    .await;
    assert_params_accepted(&result, "thread_snapshot with string-encoded integers");
}

#[test]
fn thread_snapshot_request_defaults() {
    let req: ThreadSnapshotRequest = from_value(json!({})).unwrap();
    assert!(req.include_stacks, "include_stacks defaults to true");
    assert_eq!(req.stack_depth, 10, "stack_depth defaults to 10");
    assert_eq!(req.max_threads, 50, "max_threads defaults to 50");
}

#[test]
fn thread_snapshot_request_explicit() {
    let req: ThreadSnapshotRequest =
        from_value(json!({"include_stacks": false, "stack_depth": 5, "max_threads": 10})).unwrap();
    assert!(!req.include_stacks);
    assert_eq!(req.stack_depth, 5);
    assert_eq!(req.max_threads, 10);
}

/// Hard caps protect against pathological inputs even if the deserializer accepts them.
#[test]
fn thread_snapshot_clamping_constants() {
    // Large requested values are clamped to MAX_STACK_DEPTH / MAX_THREADS_HARD_CAP rather than
    // passed through to the adapter — asserted against the same function debug_thread_snapshot
    // uses.
    let req: ThreadSnapshotRequest = serde_json::from_value(json!({
        "stack_depth": 100_000,
        "max_threads": 100_000,
    }))
    .unwrap();
    let (clamped_depth, clamped_threads) = clamp_snapshot_limits(&req);
    assert_eq!(
        clamped_depth, MAX_STACK_DEPTH,
        "stack_depth must clamp to MAX_STACK_DEPTH"
    );
    assert_eq!(
        clamped_threads, MAX_THREADS_HARD_CAP,
        "max_threads must clamp to MAX_THREADS_HARD_CAP"
    );

    // Negative / zero requests floor at 1, not 0 (avoids empty stack request).
    let req: ThreadSnapshotRequest = serde_json::from_value(json!({
        "stack_depth": 0,
        "max_threads": 0,
    }))
    .unwrap();
    let (floored_depth, floored_threads) = clamp_snapshot_limits(&req);
    assert_eq!(floored_depth, 1, "stack_depth of 0 must floor at 1");
    assert_eq!(floored_threads, 1, "max_threads of 0 must floor at 1");
}

/// The toolset filter in `McpHandler::new` matches router keys (derived
/// from `#[tool]` method names) against `DebugTool` strum names. A
/// mismatch silently strips the tool from every toolset, so pin the two
/// lists to each other in both directions.
#[test]
fn tool_routes_and_debug_tool_variants_match() {
    use strum::VariantNames;

    let router = McpHandler::tool_router();
    let always_available = McpHandler::always_available_tools();
    for name in router.map.keys() {
        assert!(
            DebugTool::VARIANTS.contains(&name.as_ref())
                || always_available.contains(&name.as_ref()),
            "tool '{name}' has no DebugTool variant and would be stripped from every toolset"
        );
    }
    for variant in DebugTool::VARIANTS {
        assert!(
            router.map.contains_key(*variant),
            "DebugTool variant '{variant}' has no #[tool] route — typo in the strum name?"
        );
    }
}
