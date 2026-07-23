//! Tool catalog exposed to the master over MCP, plus a JSON-RPC 2.0
//! newline-delimited server over stdio. Porta
//! `legacy/lib/regente/mcp/tools.rb` + `legacy/lib/regente/mcp/server.rb`.

use crate::session::Session;
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

pub const PROTOCOL_VERSION: &str = "2024-11-05";

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

/// MCP tools/list payload: name, description, JSON schema of every tool.
pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "spawn_agent",
            description: "Dispara um worker (CLI de IA) isolado num git worktree pra uma tarefa.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "cli": { "type": "string", "description": "claude|codex|gemini|opencode" },
                    "task": { "type": "string" },
                    "model": { "type": "string" },
                    "role": { "type": "string" }
                },
                "required": ["cli", "task"]
            }),
        },
        ToolDef {
            name: "list_agents",
            description: "Lista os agentes e seus estados.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "agent_status",
            description: "Estado atual de um agente.",
            input_schema: json!({
                "type": "object",
                "properties": { "agent_id": { "type": "string" } },
                "required": ["agent_id"]
            }),
        },
        ToolDef {
            name: "wait_agent",
            description: "Bloqueia ate o agente terminar (ou timeout em s) e commita o trabalho. Use apos spawn_agent.",
            input_schema: json!({
                "type": "object",
                "properties": { "agent_id": { "type": "string" }, "timeout": { "type": "integer" } },
                "required": ["agent_id"]
            }),
        },
        ToolDef {
            name: "read_output",
            description: "Saida acumulada de um agente.",
            input_schema: json!({
                "type": "object",
                "properties": { "agent_id": { "type": "string" } },
                "required": ["agent_id"]
            }),
        },
        ToolDef {
            name: "send_message",
            description: "Injeta texto na sessao de um agente (redirecionar / cochichar / takeover).",
            input_schema: json!({
                "type": "object",
                "properties": { "agent_id": { "type": "string" }, "text": { "type": "string" } },
                "required": ["agent_id", "text"]
            }),
        },
        ToolDef {
            name: "kill_agent",
            description: "Mata a sessao de um agente.",
            input_schema: json!({
                "type": "object",
                "properties": { "agent_id": { "type": "string" } },
                "required": ["agent_id"]
            }),
        },
        ToolDef {
            name: "diff_agent",
            description: "Diff da branch de um agente.",
            input_schema: json!({
                "type": "object",
                "properties": { "agent_id": { "type": "string" } },
                "required": ["agent_id"]
            }),
        },
        ToolDef {
            name: "review",
            description: "Monta o contexto de revisao: diffs das branches dos agentes dados.",
            input_schema: json!({
                "type": "object",
                "properties": { "agent_ids": { "type": "array", "items": { "type": "string" } } },
                "required": ["agent_ids"]
            }),
        },
        ToolDef {
            name: "run_tests",
            description: "Roda o comando de verify no worktree do agente (se configurado).",
            input_schema: json!({
                "type": "object",
                "properties": { "agent_id": { "type": "string" } },
                "required": ["agent_id"]
            }),
        },
        ToolDef {
            name: "consult",
            description: "Pergunta pontual a um modelo mais forte (ex: opus) sem spawnar worker. Escalacao de raciocinio.",
            input_schema: json!({
                "type": "object",
                "properties": { "question": { "type": "string" }, "model": { "type": "string" } },
                "required": ["question"]
            }),
        },
        ToolDef {
            name: "open_pr",
            description: "Abre um PR a partir de uma branch (nunca faz merge). Fallback: patch local.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "branch": { "type": "string" },
                    "title": { "type": "string" },
                    "body": { "type": "string" }
                },
                "required": ["branch", "title", "body"]
            }),
        },
    ]
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    arg_str(args, key).ok_or_else(|| anyhow::anyhow!("argumento obrigatorio ausente: {key}"))
}

fn arg_strs(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Dispatch a tool call to the session. Returns the session's result value.
/// Errs on unknown tool or missing required arguments.
pub fn call(name: &str, arguments: &Value, session: &mut Session) -> Result<Value> {
    let empty = json!({});
    let args = if arguments.is_null() { &empty } else { arguments };

    Ok(match name {
        "spawn_agent" => session.spawn_agent(
            require_str(args, "cli")?,
            require_str(args, "task")?,
            arg_str(args, "model"),
            arg_str(args, "role"),
            None,
        )?,
        "list_agents" => session.list_agents(),
        "agent_status" => session.agent_status(require_str(args, "agent_id")?),
        "wait_agent" => session.wait_agent(
            require_str(args, "agent_id")?,
            args.get("timeout").and_then(Value::as_u64),
        ),
        "read_output" => session.read_output(require_str(args, "agent_id")?),
        "send_message" => {
            session.send_message(require_str(args, "agent_id")?, require_str(args, "text")?)
        }
        "kill_agent" => session.kill_agent(require_str(args, "agent_id")?),
        "diff_agent" => session.diff_agent(require_str(args, "agent_id")?),
        "review" => session.review(&arg_strs(args, "agent_ids")),
        "run_tests" => session.run_tests(require_str(args, "agent_id")?),
        "consult" => session.consult(require_str(args, "question")?, arg_str(args, "model"), None)?,
        "open_pr" => session.open_pr(
            require_str(args, "branch")?,
            require_str(args, "title")?,
            require_str(args, "body")?,
        )?,
        other => bail!("tool desconhecida: {other}"),
    })
}

/// Minimal MCP server over stdio using newline-delimited JSON-RPC 2.0.
pub struct Server<'a, R, W> {
    session: Session<'a>,
    input: R,
    output: W,
}

impl<'a, R: BufRead, W: Write> Server<'a, R, W> {
    pub fn new(session: Session<'a>, input: R, output: W) -> Self {
        Server { session, input, output }
    }

    pub fn run(&mut self) -> Result<()> {
        loop {
            let mut line = String::new();
            let n = self.input.read_line(&mut line)?;
            if n == 0 {
                break;
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(msg) = serde_json::from_str::<Value>(line) else { continue };
            if let Some(reply) = self.handle(&msg) {
                self.write(&reply)?;
            }
        }
        Ok(())
    }

    /// Handle one message; returns a response value, or None for notifications.
    fn handle(&mut self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));

        // notifications carry no id and get no response
        if msg.get("id").is_none() && method.starts_with("notifications/") {
            return None;
        }

        Some(match method {
            "initialize" => ok(id, initialize_result()),
            "ping" => ok(id, json!({})),
            "tools/list" => ok(id, json!({ "tools": tools_list_json() })),
            "tools/call" => ok(id, self.call_tool(&params)),
            other => err(id, -32_601, &format!("method not found: {other}")),
        })
    }

    fn call_tool(&mut self, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let empty = json!({});
        let args = params.get("arguments").unwrap_or(&empty);
        match call(name, args, &mut self.session) {
            Ok(result) => {
                let is_error = result.get("error").map(|e| !e.is_null()).unwrap_or(false);
                json!({
                    "content": [{ "type": "text", "text": serde_json::to_string(&result).unwrap() }],
                    "isError": is_error
                })
            }
            Err(e) => json!({ "content": [{ "type": "text", "text": e.to_string() }], "isError": true }),
        }
    }

    fn write(&mut self, obj: &Value) -> Result<()> {
        writeln!(self.output, "{}", serde_json::to_string(obj)?)?;
        self.output.flush()?;
        Ok(())
    }
}

fn tools_list_json() -> Vec<Value> {
    definitions()
        .into_iter()
        .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.input_schema }))
        .collect()
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "regente", "version": env!("CARGO_PKG_VERSION") }
    })
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::fs;
    use std::io::Cursor;
    use std::path::PathBuf;

    fn init_repo(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("regente-mcp-test-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        run(&d, &["init", "-q"]);
        run(&d, &["config", "user.email", "test@example.com"]);
        run(&d, &["config", "user.name", "Test"]);
        fs::write(d.join("README.md"), "hello\n").unwrap();
        run(&d, &["add", "-A"]);
        run(&d, &["commit", "-q", "-m", "initial"]);
        d
    }

    fn run(dir: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git").arg("-C").arg(dir).args(args).status().unwrap();
        assert!(status.success(), "git {:?} falhou", args);
    }

    fn roundtrip(messages: &[Value], session: Session) -> Vec<Value> {
        let body = messages.iter().map(|m| serde_json::to_string(m).unwrap()).collect::<Vec<_>>().join("\n");
        let input = Cursor::new(body.into_bytes());
        let mut output = Vec::new();
        Server::new(session, input, &mut output).run().unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn definitions_cover_catalog() {
        let defs = definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name).collect();
        assert!(names.contains(&"spawn_agent"));
        assert!(names.contains(&"open_pr"));
        assert_eq!(defs.len(), 12);
    }

    #[test]
    fn call_dispatches_to_session() {
        let repo = init_repo("dispatch");
        let config = Config::default();
        let mut session = Session::new(&repo, &config);
        let result = call("list_agents", &json!({}), &mut session).unwrap();
        assert_eq!(result, json!({ "agents": [] }));
    }

    #[test]
    fn call_unknown_tool_errs() {
        let repo = init_repo("unknown");
        let config = Config::default();
        let mut session = Session::new(&repo, &config);
        assert!(call("nope", &json!({}), &mut session).is_err());
    }

    #[test]
    fn call_missing_required_arg_errs() {
        let repo = init_repo("missing-arg");
        let config = Config::default();
        let mut session = Session::new(&repo, &config);
        assert!(call("agent_status", &json!({}), &mut session).is_err());
    }

    #[test]
    fn initialize_returns_server_info() {
        let repo = init_repo("init");
        let config = Config::default();
        let session = Session::new(&repo, &config);
        let replies = roundtrip(&[json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" })], session);
        assert_eq!(replies[0]["result"]["serverInfo"]["name"], json!("regente"));
        assert!(replies[0]["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_includes_spawn_agent() {
        let repo = init_repo("tools-list");
        let config = Config::default();
        let session = Session::new(&repo, &config);
        let replies = roundtrip(&[json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })], session);
        let names: Vec<String> = replies[0]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"spawn_agent".to_string()));
        assert!(!replies[0]["result"]["tools"].as_array().unwrap().is_empty());
    }

    #[test]
    fn tools_call_list_agents_empty() {
        let repo = init_repo("call-list-agents");
        let config = Config::default();
        let session = Session::new(&repo, &config);
        let msg = json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "list_agents", "arguments": {} }
        });
        let replies = roundtrip(&[msg], session);
        let text = replies[0]["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed, json!({ "agents": [] }));
        assert_eq!(replies[0]["result"]["isError"], json!(false));
    }

    #[test]
    fn notification_produces_no_reply() {
        let repo = init_repo("notification");
        let config = Config::default();
        let session = Session::new(&repo, &config);
        let replies = roundtrip(&[json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })], session);
        assert!(replies.is_empty());
    }

    #[test]
    fn unknown_method_errors() {
        let repo = init_repo("unknown-method");
        let config = Config::default();
        let session = Session::new(&repo, &config);
        let replies = roundtrip(&[json!({ "jsonrpc": "2.0", "id": 9, "method": "bogus/thing" })], session);
        assert_eq!(replies[0]["error"]["code"], json!(-32_601));
    }
}
