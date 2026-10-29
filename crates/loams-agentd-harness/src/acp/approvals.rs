//! ACP agents in a supervised thread (plan SD1 Task 13): a tool permission's
//! `session/request_permission` becomes an approval, and the person's
//! decision maps to one of the options the agent offered.

use serde_json::{Value, json};

use crate::ApprovalAsk;

/// The approval a tool permission asks for. The tool is the call's `kind`
/// (`edit`, `delete`, `execute`, …), else its title; the call id is the
/// call's `toolCallId`, else the request's own id. The arguments are the
/// tool call as the agent described it (title, kind, raw input, locations).
pub(crate) fn ask(id: &Value, params: &Value) -> ApprovalAsk {
    let call = params.get("toolCall").cloned().unwrap_or(Value::Null);
    // ACP's catch-all kind `other` names no tool (goose sends it for every
    // call), so the title stands in; goose titles read `<tool> · <summary>`,
    // and only the tool part names what "always" remembers.
    let tool = call
        .get("kind")
        .and_then(Value::as_str)
        .filter(|kind| !kind.is_empty() && *kind != "other")
        .or_else(|| {
            call.get("title")
                .and_then(Value::as_str)
                .map(|title| title.split(" · ").next().unwrap_or(title).trim())
        })
        .filter(|tool| !tool.is_empty())
        .unwrap_or("permission")
        .to_owned();
    let call_id = call
        .get("toolCallId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| match id {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        });
    let mut args = serde_json::Map::new();
    for key in ["title", "kind", "rawInput", "locations"] {
        if let Some(value) = call.get(key) {
            args.insert(key.into(), value.clone());
        }
    }
    let args = Value::Object(args);
    let risk = crate::approvals::tool_risk(&tool, &args);
    ApprovalAsk {
        call_id,
        tool,
        args,
        risk,
    }
}

/// The answer to the permission request: an allow maps to the agent's
/// allow-once option (else its allow-always one, when that is all it
/// offers), a denial to its reject-once option (else reject-always); with no
/// such option the request is cancelled. "always" is the daemon's to
/// remember per thread and tool, so the agent is never told to remember.
pub(crate) fn outcome(options: &[Value], allowed: bool) -> Value {
    let kinds: &[&str] = if allowed {
        &["allow_once", "allow_always"]
    } else {
        &["reject_once", "reject_always"]
    };
    let option = kinds.iter().find_map(|kind| {
        options
            .iter()
            .find(|o| o.get("kind").and_then(Value::as_str) == Some(kind))
            .and_then(|o| o.get("optionId").and_then(Value::as_str))
    });
    match option {
        Some(option_id) => json!({ "outcome": { "outcome": "selected", "optionId": option_id } }),
        None => json!({ "outcome": { "outcome": "cancelled" } }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_kind_falls_back_to_the_tool_in_the_title() {
        let params = json!({"toolCall": {
            "toolCallId": "call_x",
            "kind": "other",
            "title": "shell · echo hi",
            "rawInput": {"command": "echo hi"},
        }});
        let ask = ask(&json!("625d36a4"), &params);
        assert_eq!(ask.tool, "shell");
        assert_eq!(ask.call_id, "call_x");
        let params = json!({"toolCall": {"kind": "delete", "title": "Delete build"}});
        assert_eq!(super::ask(&json!(1), &params).tool, "delete");
    }

    #[test]
    fn decisions_map_to_offered_options() {
        let options = vec![
            json!({"optionId": "always", "kind": "allow_always"}),
            json!({"optionId": "once", "kind": "allow_once"}),
            json!({"optionId": "never", "kind": "reject_always"}),
        ];
        assert_eq!(outcome(&options, true)["outcome"]["optionId"], "once");
        assert_eq!(outcome(&options, false)["outcome"]["optionId"], "never");
        assert_eq!(
            outcome(&options[..1], false)["outcome"]["outcome"],
            "cancelled"
        );
    }
}
