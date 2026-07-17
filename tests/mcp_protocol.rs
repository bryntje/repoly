//! MCP stdio protocol tests: initialize, tools/list, tools/call.

mod common;

use common::WorkspaceFixture;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

struct McpSession {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl McpSession {
    fn start(args: &[&str]) -> Self {
        let mut child = Command::new(assert_cmd::cargo::cargo_bin("poly"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn poly mcp");

        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        }
    }

    fn write_line(&mut self, v: &Value) {
        let line = serde_json::to_string(v).unwrap();
        writeln!(self.stdin, "{line}").expect("write mcp stdin");
        self.stdin.flush().expect("flush");
    }

    fn notify_initialized(&mut self) {
        self.write_line(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
    }

    fn request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.write_line(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }));
        id
    }

    fn initialize(&mut self) -> Value {
        let id = self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "poly-test", "version": "0.0.1" }
            }),
        );
        self.notify_initialized();
        self.wait_response(id)
    }

    fn wait_response(&mut self, id: u64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut line = String::new();

        while Instant::now() < deadline {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => panic!("MCP stdout EOF before response id={id}"),
                Ok(_) => {
                    let t = line.trim();
                    if t.is_empty() {
                        continue;
                    }
                    let v: Value = serde_json::from_str(t)
                        .unwrap_or_else(|e| panic!("invalid json from mcp: {e}: {t}"));
                    if let Some(rid) = v.get("id").and_then(|x| x.as_u64()) {
                        if rid == id {
                            return v;
                        }
                        // other ids — ignore (shouldn't happen in our sequential tests)
                    }
                }
                Err(e) => panic!("read mcp stdout: {e}"),
            }
        }
        panic!("timeout waiting for MCP response id={id}");
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Value {
        let id = self.request(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments
            }),
        );
        self.wait_response(id)
    }

    fn list_tools(&mut self) -> Value {
        let id = self.request("tools/list", json!({}));
        self.wait_response(id)
    }

    fn tool_text(result: &Value) -> String {
        if let Some(err) = result.get("error") {
            return format!("ERROR:{err}");
        }
        result["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    fn tool_error_message(result: &Value) -> Option<String> {
        result
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .map(|s| s.to_string())
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn mcp_initialize_and_list_tools_readonly() {
    let fx = WorkspaceFixture::basic();
    let cfg = fx.config();
    let mut s = McpSession::start(&["mcp", "--config", cfg.to_str().unwrap()]);

    let init = s.initialize();
    assert!(init.get("error").is_none(), "{init}");
    let result = &init["result"];
    assert_eq!(result["serverInfo"]["name"], "poly");
    let instructions = result["instructions"].as_str().unwrap_or("");
    assert!(
        instructions.to_lowercase().contains("disabled"),
        "readonly server should note exec disabled: {instructions}"
    );

    let listed = s.list_tools();
    assert!(listed.get("error").is_none(), "{listed}");
    let tools = listed["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    for required in [
        "list_repos",
        "status",
        "plan",
        "build_context",
        "repo_path",
        "workspace_root",
        "exec",
        "run",
        "commit",
    ] {
        assert!(
            names.contains(&required),
            "missing tool {required} in {names:?}"
        );
    }
}

#[test]
fn mcp_list_repos_and_plan_and_status() {
    let fx = WorkspaceFixture::basic();
    let cfg = fx.config();
    let mut s = McpSession::start(&["mcp", "--config", cfg.to_str().unwrap()]);
    s.initialize();

    let text = McpSession::tool_text(&s.call_tool("list_repos", json!({})));
    let v: Value = serde_json::from_str(&text).expect(&text);
    assert_eq!(v["workspace"], "fixture");
    assert_eq!(v["repos"].as_array().unwrap().len(), 2);

    let text = McpSession::tool_text(&s.call_tool(
        "plan",
        json!({ "query": "oauth", "format": "json", "no_status": true }),
    ));
    let v: Value = serde_json::from_str(&text).expect(&text);
    let steps = v["steps"].as_array().unwrap();
    assert!(!steps.is_empty());
    let ids: Vec<&str> = steps.iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"api") || ids.contains(&"web"), "{ids:?}");

    let text = McpSession::tool_text(&s.call_tool("status", json!({ "repos": "api,web" })));
    let v: Value = serde_json::from_str(&text).expect(&text);
    assert_eq!(v["repos"].as_array().unwrap().len(), 2);

    let text = McpSession::tool_text(&s.call_tool("workspace_root", json!({})));
    assert!(
        text.contains(fx.path().file_name().unwrap().to_str().unwrap())
            || std::path::Path::new(text.trim()).exists(),
        "unexpected root: {text}"
    );

    let text = McpSession::tool_text(&s.call_tool("repo_path", json!({ "repo": "web" })));
    assert!(text.contains("web"), "{text}");
}

#[test]
fn mcp_build_context_prompt() {
    let fx = WorkspaceFixture::basic();
    let cfg = fx.config();
    let mut s = McpSession::start(&["mcp", "--config", cfg.to_str().unwrap()]);
    s.initialize();

    let text = McpSession::tool_text(&s.call_tool(
        "build_context",
        json!({
            "query": "oauth",
            "format": "prompt",
            "no_status": true,
            "max_chars": 8000
        }),
    ));
    assert!(
        text.contains("Poly workspace") || text.contains("fixture") || text.contains("Selected"),
        "{text}"
    );
    assert!(
        text.contains("PLATFORM") || text.contains("Shared always-doc") || text.contains("always"),
        "expected always-doc content in: {text}"
    );
}

#[test]
fn mcp_exec_disabled_without_flag() {
    let fx = WorkspaceFixture::basic();
    let cfg = fx.config();
    let mut s = McpSession::start(&["mcp", "--config", cfg.to_str().unwrap()]);
    s.initialize();

    let resp = s.call_tool(
        "exec",
        json!({
            "repo": "api",
            "command": ["git", "rev-parse", "--abbrev-ref", "HEAD"]
        }),
    );
    let msg = McpSession::tool_error_message(&resp).unwrap_or_else(|| McpSession::tool_text(&resp));
    assert!(
        msg.to_lowercase().contains("disabled") || msg.contains("allow-exec"),
        "{msg}"
    );
}

#[test]
fn mcp_exec_and_run_with_allowlist() {
    let fx = WorkspaceFixture::basic();
    let cfg = fx.config();
    let mut s = McpSession::start(&[
        "mcp",
        "--config",
        cfg.to_str().unwrap(),
        "--allow-exec",
        "--exec-repos",
        "api,web",
    ]);
    let init = s.initialize();
    let instructions = init["result"]["instructions"].as_str().unwrap_or("");
    assert!(
        instructions.to_lowercase().contains("enabled"),
        "{instructions}"
    );

    let text = McpSession::tool_text(&s.call_tool(
        "exec",
        json!({
            "repo": "api",
            "command": ["git", "rev-parse", "--abbrev-ref", "HEAD"]
        }),
    ));
    let v: Value = serde_json::from_str(&text).expect(&text);
    assert_eq!(v["success"], true);
    assert!(v["stdout"].as_str().unwrap_or("").contains("main"));

    let text = McpSession::tool_text(&s.call_tool(
        "run",
        json!({
            "repos": "api,web",
            "command": ["git", "rev-parse", "--is-inside-work-tree"]
        }),
    ));
    let v: Value = serde_json::from_str(&text).expect(&text);
    assert_eq!(v["summary"], "ok");
    assert_eq!(v["results"].as_array().unwrap().len(), 2);

    drop(s);
    let mut s = McpSession::start(&[
        "mcp",
        "--config",
        cfg.to_str().unwrap(),
        "--allow-exec",
        "--exec-repos",
        "api",
    ]);
    s.initialize();
    let resp = s.call_tool(
        "run",
        json!({
            "repos": "api,web",
            "command": ["true"]
        }),
    );
    let msg = McpSession::tool_error_message(&resp).unwrap_or_else(|| McpSession::tool_text(&resp));
    assert!(
        msg.contains("allowlist") || msg.contains("web"),
        "expected allowlist error, got: {msg}"
    );
}

#[test]
fn mcp_shell_requires_allow_shell() {
    let fx = WorkspaceFixture::basic();
    let cfg = fx.config();

    let mut s = McpSession::start(&[
        "mcp",
        "--config",
        cfg.to_str().unwrap(),
        "--allow-exec",
        "--exec-repos",
        "api",
    ]);
    s.initialize();
    let resp = s.call_tool(
        "exec",
        json!({
            "repo": "api",
            "command": ["echo hi"],
            "shell": true
        }),
    );
    let msg = McpSession::tool_error_message(&resp).unwrap_or_else(|| McpSession::tool_text(&resp));
    assert!(
        msg.contains("shell") || msg.contains("allow-shell"),
        "{msg}"
    );
    drop(s);

    let mut s = McpSession::start(&[
        "mcp",
        "--config",
        cfg.to_str().unwrap(),
        "--allow-exec",
        "--allow-shell",
        "--exec-repos",
        "api",
    ]);
    s.initialize();
    let text = McpSession::tool_text(&s.call_tool(
        "exec",
        json!({
            "repo": "api",
            "command": ["echo shell-ok"],
            "shell": true
        }),
    ));
    let v: Value = serde_json::from_str(&text).expect(&text);
    assert_eq!(v["success"], true);
    assert!(
        v["stdout"].as_str().unwrap_or("").contains("shell-ok"),
        "{v}"
    );
}

#[test]
fn mcp_commit_tool() {
    let fx = WorkspaceFixture::basic();
    fx.dirty_web();
    let cfg = fx.config();
    let mut s = McpSession::start(&[
        "mcp",
        "--config",
        cfg.to_str().unwrap(),
        "--allow-exec",
        "--exec-repos",
        "web",
    ]);
    s.initialize();

    let text = McpSession::tool_text(&s.call_tool(
        "commit",
        json!({
            "repo": "web",
            "message": "test: mcp commit",
            "all": true
        }),
    ));
    let v: Value = serde_json::from_str(&text).expect(&text);
    assert_eq!(v["success"], true, "{v}");
    assert!(v["commit_sha"].as_str().is_some());
}
