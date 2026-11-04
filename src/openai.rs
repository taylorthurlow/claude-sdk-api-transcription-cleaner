//! Request and response bodies in the OpenAI chat completions format.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// Returns the system prompt, if the messages include one, and the prompt text.
pub fn split_messages(messages: &Value) -> (Option<String>, String) {
    let messages = messages.as_array().map(Vec::as_slice).unwrap_or_default();
    let is_system =
        |message: &&Value| matches!(message["role"].as_str(), Some("system" | "developer"));
    let (system_messages, others): (Vec<&Value>, Vec<&Value>) =
        messages.iter().partition(is_system);

    let system_prompt = system_messages
        .iter()
        .map(|message| text_of(&message["content"]))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let prompt = match others.as_slice() {
        [only] => text_of(&only["content"]),
        _ => others
            .iter()
            .map(|message| {
                let role = message["role"].as_str().unwrap_or("user");
                format!("{role}: {}", text_of(&message["content"]))
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
    };

    ((!system_prompt.is_empty()).then_some(system_prompt), prompt)
}

/// Returns the text of a message's content, which is a string or a list of parts.
fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str())
            .collect(),
        _ => String::new(),
    }
}

pub fn model_list(models: &[&str]) -> Value {
    let data: Vec<Value> = models
        .iter()
        .map(|id| json!({ "id": id, "object": "model", "created": 0, "owned_by": "anthropic" }))
        .collect();
    json!({ "object": "list", "data": data })
}

pub fn completion(model: &str, reply: &str) -> Value {
    json!({
        "id": completion_id(),
        "object": "chat.completion",
        "created": now(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": reply },
            "finish_reason": "stop",
        }],
        "usage": { "prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0 },
    })
}

/// Returns a server-sent events body with the whole reply in a single chunk.
pub fn completion_stream(model: &str, reply: &str) -> String {
    let id = completion_id();
    let created = now();
    let chunk = |delta: Value, finish_reason: Value| {
        json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish_reason }],
        })
    };

    [
        chunk(
            json!({ "role": "assistant", "content": reply }),
            Value::Null,
        ),
        chunk(json!({}), json!("stop")),
    ]
    .iter()
    .map(|event| format!("data: {event}\n\n"))
    .chain(["data: [DONE]\n\n".to_string()])
    .collect()
}

pub fn error(message: &str) -> Value {
    json!({ "error": { "message": message } })
}

fn completion_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("chatcmpl-{nanos:x}")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
