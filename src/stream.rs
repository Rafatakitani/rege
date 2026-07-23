//! Parser for Claude's `--output-format stream-json` events, ported from
//! `legacy/lib/regente/stream.rb`. Translates one parsed JSON line into zero
//! or more small, UI-friendly Events for the TUI chat to render.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Ready { session_id: String },
    Text(String),
    Tool { name: String, input: String },
    ToolResult(String),
    Done { cost: Option<f64> },
}

/// Maps one parsed stream-json line to zero or more Events (an "assistant"
/// hash may carry several content blocks, each becoming its own Event).
pub fn parse_line(json: &Value) -> Vec<Event> {
    match json.get("type").and_then(Value::as_str) {
        Some("system") => {
            if json.get("subtype").and_then(Value::as_str) == Some("init") {
                match json.get("session_id").and_then(Value::as_str) {
                    Some(id) => vec![Event::Ready { session_id: id.to_string() }],
                    None => vec![],
                }
            } else {
                vec![]
            }
        }
        Some("assistant") => blocks(json.pointer("/message/content")),
        Some("user") => tool_results(json.pointer("/message/content")),
        Some("result") => {
            let cost = json.get("total_cost_usd").and_then(Value::as_f64);
            vec![Event::Done { cost }]
        }
        _ => vec![],
    }
}

fn blocks(content: Option<&Value>) -> Vec<Event> {
    let Some(arr) = content.and_then(Value::as_array) else { return vec![] };
    arr.iter()
        .filter_map(|b| match b.get("type").and_then(Value::as_str) {
            Some("text") => {
                let text = b.get("text").and_then(Value::as_str).unwrap_or("").to_string();
                Some(Event::Text(text))
            }
            Some("tool_use") => {
                let name = b.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let input = b
                    .get("input")
                    .map(|v| serde_json::to_string(v).unwrap_or_default())
                    .unwrap_or_default();
                Some(Event::Tool { name, input })
            }
            _ => None,
        })
        .collect()
}

fn tool_results(content: Option<&Value>) -> Vec<Event> {
    let Some(arr) = content.and_then(Value::as_array) else { return vec![] };
    arr.iter()
        .filter_map(|b| {
            if b.get("type").and_then(Value::as_str) != Some("tool_result") {
                return None;
            }
            let text = match b.get("content") {
                Some(Value::Array(parts)) => parts
                    .iter()
                    .filter_map(|c| c.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(""),
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            Some(Event::ToolResult(text))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Vec<Event> {
        parse_line(&serde_json::from_str(line).unwrap())
    }

    #[test]
    fn system_init_yields_ready() {
        let line = r#"{"type":"system","subtype":"init","session_id":"sess-123","tools":[]}"#;
        assert_eq!(parse(line), vec![Event::Ready { session_id: "sess-123".into() }]);
    }

    #[test]
    fn system_non_init_yields_nothing() {
        let line = r#"{"type":"system","subtype":"other"}"#;
        assert_eq!(parse(line), vec![]);
    }

    #[test]
    fn assistant_text_block() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"oi"}]}}"#;
        assert_eq!(parse(line), vec![Event::Text("oi".into())]);
    }

    #[test]
    fn assistant_tool_use_block() {
        let line = r#"{"type":"assistant","message":{"content":[
            {"type":"tool_use","name":"spawn_agent","input":{"role":"worker","task":"x"}}
        ]}}"#;
        let events = parse(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::Tool { name, input } => {
                assert_eq!(name, "spawn_agent");
                let v: serde_json::Value = serde_json::from_str(input).unwrap();
                assert_eq!(v["role"], "worker");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn assistant_multiple_blocks() {
        let line = r#"{"type":"assistant","message":{"content":[
            {"type":"text","text":"pensando..."},
            {"type":"tool_use","name":"list_agents","input":{}}
        ]}}"#;
        let events = parse(line);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], Event::Text("pensando...".into()));
        assert!(matches!(&events[1], Event::Tool { name, .. } if name == "list_agents"));
    }

    #[test]
    fn user_tool_result_string_content() {
        let line = r#"{"type":"user","message":{"content":[
            {"type":"tool_result","tool_use_id":"abc","content":"ok feito"}
        ]}}"#;
        assert_eq!(parse(line), vec![Event::ToolResult("ok feito".into())]);
    }

    #[test]
    fn user_tool_result_array_content() {
        let line = r#"{"type":"user","message":{"content":[
            {"type":"tool_result","tool_use_id":"abc","content":[
                {"type":"text","text":"linha1"},
                {"type":"text","text":"linha2"}
            ]}
        ]}}"#;
        assert_eq!(parse(line), vec![Event::ToolResult("linha1linha2".into())]);
    }

    #[test]
    fn result_yields_done_with_cost() {
        let line = r#"{"type":"result","subtype":"success","total_cost_usd":0.0234,"session_id":"sess-123","result":"done"}"#;
        assert_eq!(parse(line), vec![Event::Done { cost: Some(0.0234) }]);
    }

    #[test]
    fn result_without_cost_yields_done_none() {
        let line = r#"{"type":"result","subtype":"error"}"#;
        assert_eq!(parse(line), vec![Event::Done { cost: None }]);
    }

    #[test]
    fn unknown_type_yields_nothing() {
        let line = r#"{"type":"ping"}"#;
        assert_eq!(parse(line), vec![]);
    }
}
