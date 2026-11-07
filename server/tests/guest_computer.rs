// Checks guest-owned configuration and capability routing.
use miniclaw_server::guest_computer::{Policy, Route};
// Shares the exact owned capability definition.
use miniclaw_server::scoped::tool_definition;
// Builds independent controller requests and expectations.
use serde_json::json;
// Exercises policy over the authenticated worker stream.
use miniclaw_server::worker;
// Builds a local peer without inference.
use tokio::{io::duplex, process::Command};

/// The guest channel must not expose administration, shell, or future RPC methods.
#[test]
fn rejects_management_rpc() {
    let policy = Policy::new(None);
    for method in [
        "command/exec",
        "process/spawn",
        "config/value/write",
        "config/batchWrite",
        "fs/readFile",
        "mcpServer/oauth/login",
        "thread/fork",
        "plugin/install",
        "future/unsafe",
    ] {
        assert!(
            matches!(policy.route(json!({"id": 10, "method": method,
            "params": {}})), Route::Reply(value) if value["error"]["code"] == -32602),
            "Method reached the guest: {method}"
        );
    }
}

/// Allowed chat methods cannot re-enable tools or replace the guest sandbox.
#[test]
fn rejects_policy_overrides() {
    let policy = Policy::new(None);
    for method in ["thread/start", "thread/resume"] {
        for params in [
            json!({"sandbox": "danger-full-access"}),
            json!({"config": {"features.shell_tool": true}}),
            json!({"config": {"features": {"shell_tool": true}}}),
            json!({"config": {"model_provider": "external"}}),
            json!({"approvalPolicy": "on-request"}),
            json!({"environments": [{"id": "external"}]}),
            json!({"dynamicTools": [{"name": "shell"}]}),
            json!({"runtimeWorkspaceRoots": ["/"]}),
            json!({"configProfile": "unrestricted"}),
        ] {
            assert!(matches!(policy.route(json!({"id": 11, "method": method,
                "params": params})), Route::Reply(value) if value["error"].is_object()));
        }
    }
    for params in [
        json!({"sandboxPolicy": {"type": "externalSandbox"}}),
        json!({"input": [{"type": "localImage", "path": "/private/file"}]}),
        json!({"environments": [{"id": "external"}]}),
    ] {
        assert!(
            matches!(policy.route(json!({"id": 12, "method": "turn/start",
            "params": params})), Route::Reply(value) if value["error"].is_object())
        );
    }
}

/// Forwards only MiniClaw's closed scoped tool definition on guest chats.
#[test]
fn accepts_only_owned_scoped_tool() {
    let policy = Policy::new(None);
    let approved = json!({"id": 17, "method": "thread/start", "params": {
        "dynamicTools": [tool_definition()]}});
    assert_eq!(policy.route(approved.clone()), Route::Forward(approved));
    let mut forged = tool_definition();
    forged["description"] = json!("Do whatever the model wants");
    assert!(matches!(policy.route(json!({"id": 18, "method": "thread/start",
        "params": {"dynamicTools": [forged]}})), Route::Reply(_)));
}

/// Beside the file tool the guest accepts MiniClaw's tools that the Mac answers: by name and
/// shape, not by their exact words, so tuning a description does not break an installed guest.
#[test]
fn accepts_tools_the_mac_answers() {
    use miniclaw_server::{ask, mcp_gateway, project_tool};
    let policy = Policy::new(None);
    let start = |tools: serde_json::Value| json!({"id": 30, "method": "thread/start", "params": {"dynamicTools": tools}});
    let (project, asking, gateway) = (project_tool::tool_definition(), ask::tool_definition(), mcp_gateway::tool_definition());
    for tools in [json!([tool_definition(), project.clone()]), json!([tool_definition(), project.clone(), asking.clone(), gateway.clone()]),
        json!([tool_definition(), asking.clone()]), json!([])] {
        assert_eq!(policy.route(start(tools.clone())), Route::Forward(start(tools.clone())), "{tools}");
    }
    // A reworded description is still the same tool.
    let mut reworded = project.clone();
    reworded["description"] = json!("Reads and writes the project's notes.");
    assert!(matches!(policy.route(start(json!([tool_definition(), reworded]))), Route::Forward(_)));

    let mut forged = tool_definition();
    forged["description"] = json!("Do whatever the model wants");
    let mut huge = project.clone();
    huge["description"] = json!("x".repeat(9000));
    let mut untyped = project.clone();
    untyped.as_object_mut().unwrap().remove("type");
    let mut unshaped = project.clone();
    unshaped["inputSchema"] = json!("anything");
    let mut extra = project.clone();
    extra["command"] = json!("/bin/sh");
    for tools in [json!([forged, project.clone()]), json!([project.clone(), tool_definition()]), json!([tool_definition(), project.clone(), project.clone()]),
        json!([tool_definition(), {"type": "function", "name": "miniclaw_delegate", "description": "d", "inputSchema": {"type": "object"}}]),
        json!([tool_definition(), {"type": "function", "name": "shell", "description": "d", "inputSchema": {"type": "object"}}]),
        json!([tool_definition(), huge]), json!([tool_definition(), untyped]), json!([tool_definition(), unshaped]), json!([tool_definition(), extra]),
        json!([tool_definition(), tool_definition()]), json!("not a list"), json!([tool_definition(), "miniclaw_project"])] {
        assert!(matches!(policy.route(start(tools.clone())), Route::Reply(value) if value["error"]["code"] == -32602), "{tools}");
    }
}

/// A Computer Use conversation never carries the file tool, but may carry the tools the Mac answers.
#[test]
fn computer_use_excludes_only_the_file_tool() {
    use miniclaw_server::{ask, project_tool};
    let policy = Policy::new(Some("/bin/echo".into()));
    let start = |tools: serde_json::Value| json!({"id": 31, "method": "thread/start", "params": {"dynamicTools": tools, "miniclawComputerUse": true}});
    assert!(matches!(policy.route(start(json!([tool_definition()]))), Route::Reply(_)));
    assert!(matches!(policy.route(start(json!([tool_definition(), ask::tool_definition()]))), Route::Reply(_)));
    assert!(matches!(policy.route(start(json!([ask::tool_definition(), project_tool::tool_definition()]))), Route::Forward(_)));
}

/// The current text-turn envelope remains available without permission overrides.
#[test]
fn keeps_text_turn() {
    let policy = Policy::new(None);
    let frame = json!({"id": 13, "method": "turn/start", "params": {
        "threadId": "kept", "environments": [], "model": "selected", "effort": "low",
        "input": [{"type": "text", "text": "Hello", "text_elements": []}],
        "additionalContext": {"miniclaw_skills": {"kind": "application", "value": "Instructions"}}
    }});
    assert_eq!(policy.route(frame.clone()), Route::Forward(frame));
}

/// Reports missing guest software without using controller-supplied paths.
#[test]
fn missing_helper() {
    let policy = Policy::new(None);
    assert_eq!(
        policy.route(json!({"id": 1, "method": "miniclaw/computer/read"})),
        Route::Reply(json!({"id": 1, "result": {"available": false}}))
    );
    assert!(
        matches!(policy.route(json!({"id": 2, "method": "thread/start",
        "params": {"miniclawComputerUse": true}})), Route::Reply(value)
        if value["error"]["code"] == -32602)
    );
}

/// Resolves computer use inside the guest while preserving conversation parameters.
#[test]
fn uses_guest_paths() {
    let policy = Policy::new(Some("/guest/helper".into()));
    for method in ["thread/start", "thread/resume"] {
        let frame = json!({"id": 3, "method": method, "params": {
            "threadId": "kept", "miniclawComputerUse": true,
            "config": {"model_reasoning_effort": "low"}}});
        let Route::Forward(value) = policy.route(frame) else {
            panic!("Expected guest configuration")
        };
        assert!(value["params"].get("miniclawComputerUse").is_none());
        assert_eq!(value["params"]["threadId"], "kept");
        assert_eq!(value["params"]["config"]["model_reasoning_effort"], "low");
        let server = &value["params"]["config"]["mcp_servers.miniclaw_computer"];
        assert_eq!(
            server["command"],
            std::env::current_exe().unwrap().to_str().unwrap()
        );
        assert_eq!(
            server["args"],
            json!(["--computer-helper", "/guest/helper"])
        );
        assert_eq!(value["params"]["approvalsReviewer"], "user");
        assert_eq!(server["required"], true);
    }
}

/// Rejects malformed markers and executable overrides before Codex sees them.
#[test]
fn rejects_external_helpers() {
    let policy = Policy::new(Some("/guest/helper".into()));
    for params in [
        json!({"miniclawComputerUse": "/host/helper"}),
        json!({"config": {"mcp_servers.miniclaw_computer": {"command": "/host/helper"}}}),
        json!({"config": {"mcp_servers": {"other": {"command": "/bin/sh"}}}}),
        json!({"config": {"mcp_servers.other.command": "/bin/sh"}}),
    ] {
        assert!(
            matches!(policy.route(json!({"id": 4, "method": "thread/start",
            "params": params})), Route::Reply(value) if value["error"]["code"] == -32602)
        );
    }
}

/// Keeps ordinary chat and approval decisions unchanged across the guest boundary.
#[test]
fn keeps_normal_frames() {
    let policy = Policy::new(Some("/guest/helper".into()));
    for frame in [
        json!({"id": 5, "method": "thread/start", "params": {"config": {"mcp_servers": {}}}}),
        json!({"id": "approval", "result": {"action": "decline", "content": null}}),
        json!({"id": 6, "method": "turn/interrupt", "params": {"threadId": "kept"}}),
    ] {
        assert_eq!(policy.route(frame.clone()), Route::Forward(frame));
    }
}

/// Answers discovery locally while ordinary RPC still reaches the guest child.
#[tokio::test]
async fn routes_guest_discovery() {
    let (controller, guest) = duplex(4096);
    let serving = tokio::spawn(worker::serve_with(
        guest,
        [4; 32],
        || {
            let mut command = Command::new("node");
            command.args([
                "--input-type=module",
                "-e",
                r#"
            import { createInterface } from 'node:readline';
            createInterface({input: process.stdin}).on('line', line => {
                const m = JSON.parse(line);
                if (m.id !== undefined) console.log(JSON.stringify({id: m.id, result: {}}));
            });
        "#,
            ]);
            command
        },
        Some(Policy::new(None)),
    ));
    let client = worker::connect(controller, [4; 32]).await.unwrap();
    assert_eq!(
        client
            .call("miniclaw/computer/read", json!({}))
            .await
            .unwrap(),
        json!({"available": false})
    );
    assert_eq!(
        client.call("config/read", json!({})).await.unwrap(),
        json!({})
    );
    // The fake child accepts everything, so only the worker can deny this request.
    assert!(
        client
            .call("command/exec", json!({"command": "forbidden"}))
            .await
            .is_err()
    );
    client.stop().await;
    serving.await.unwrap().unwrap();
}

/// A controller capability request contains no executable or helper path.
#[test]
fn guest_target_has_no_paths() {
    let mut params = json!({"model": "gpt-6-astra"});
    miniclaw_server::computer::Target::Guest.configure(&mut params);
    assert_eq!(
        params,
        json!({"model": "gpt-6-astra", "miniclawComputerUse": true})
    );
}

/// Disabling restored guest tools uses a marker, then a guest-local transport shape.
#[test]
fn disables_guest_tools() {
    let mut params = json!({"threadId": "kept", "config": {"mcp_servers": {}}});
    miniclaw_server::computer::disable(&mut params, true).unwrap();
    assert_eq!(
        params,
        json!({"threadId": "kept", "miniclawComputerUse": false,
        "config": {"mcp_servers": {}}})
    );
    let policy = Policy::new(None);
    let Route::Forward(frame) = policy.route(json!({"id": 7,
        "method": "thread/resume", "params": params}))
    else {
        panic!("Expected disabled guest configuration")
    };
    assert!(frame["params"].get("miniclawComputerUse").is_none());
    let server = &frame["params"]["config"]["mcp_servers.miniclaw_computer"];
    assert_eq!(server["enabled"], false);
    assert_eq!(
        server["command"],
        std::env::current_exe().unwrap().to_str().unwrap()
    );
}
