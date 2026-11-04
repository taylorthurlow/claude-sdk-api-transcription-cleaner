//! An OpenAI-compatible chat completions server that answers with `claude -p`.

mod claude;
mod openai;
mod service;
mod transcript;

use std::fmt;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::claude::{ClaudeError, ClaudeProcess, Standby};

const MODELS: &[&str] = &["haiku", "sonnet", "opus"];
const DEFAULT_MODEL: &str = "haiku";

// Without --system-prompt, `claude -p` uses the full Claude Code agent prompt.
const DEFAULT_SYSTEM_PROMPT: &str =
    "Follow the user's instructions. Reply with only the requested text.";

const USAGE: &str = "\
Usage: claude-sdk-api-transcription-cleaner <command>

Commands:
  start   Start the server in the background, and at every login
  stop    Stop the server, and stop it starting at login
  status  Show whether the server is running
  serve   Run the server in this terminal";

fn main() -> ExitCode {
    let result = match std::env::args().nth(1).as_deref() {
        Some("serve") => serve(),
        Some("start") => service::start(),
        Some("stop") => service::stop(),
        Some("status") => match service::status() {
            Ok(true) => Ok(()),
            // 3 is the conventional exit code for "not running".
            Ok(false) => return ExitCode::from(3),
            Err(error) => Err(error),
        },
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn serve() -> Result<(), String> {
    let port: u16 = env_number("PORT", 8765);
    let idle_timeout = Duration::from_secs(env_number("IDLE_TIMEOUT_SECONDS", 10 * 60));

    let server = Server::http(("127.0.0.1", port))
        .map_err(|error| format!("could not listen on port {port}: {error}"))?;
    let standby = Standby::new(DEFAULT_MODEL, DEFAULT_SYSTEM_PROMPT, idle_timeout);

    log(&format!("Listening on http://127.0.0.1:{port}/v1"));
    standby.refill();

    for request in server.incoming_requests() {
        let standby = Arc::clone(&standby);
        thread::spawn(move || handle(request, &standby));
    }
    Ok(())
}

fn handle(mut request: Request, standby: &Arc<Standby>) {
    let method = request.method().clone();
    let url = request.url().to_string();

    let (status, content_type, body) = match (&method, url.trim_end_matches('/')) {
        (Method::Get, "/v1/models") => json(200, openai::model_list(MODELS)),
        (Method::Post, "/v1/chat/completions") => chat_completion(&mut request, standby),
        _ => json(404, openai::error(&format!("Unknown path {url}"))),
    };

    let header = Header::from_bytes("Content-Type", content_type).expect("valid header");
    let response = Response::from_string(body)
        .with_status_code(status)
        .with_header(header);
    if let Err(error) = request.respond(response) {
        log(&format!("could not send response: {error}"));
    }
    log(&format!("\"{method} {url}\" {status}"));
}

fn chat_completion(request: &mut Request, standby: &Arc<Standby>) -> (u16, &'static str, String) {
    let mut body = String::new();
    if let Err(error) = request.as_reader().read_to_string(&mut body) {
        return json(400, openai::error(&format!("Could not read body: {error}")));
    }
    let body: Value = match serde_json::from_str(&body) {
        Ok(body) => body,
        Err(error) => return json(400, openai::error(&format!("Invalid JSON: {error}"))),
    };

    let model = body["model"].as_str().unwrap_or(DEFAULT_MODEL);
    let (system_prompt, prompt) = openai::split_messages(&body["messages"]);
    let system_prompt = system_prompt.as_deref().unwrap_or(DEFAULT_SYSTEM_PROMPT);

    log(&format!(
        "request  model={model} prompt_chars={} system_chars={}",
        prompt.chars().count(),
        system_prompt.chars().count()
    ));
    let started = Instant::now();
    let (reply, source) = match answer(standby, model, system_prompt, &prompt) {
        Ok(answer) => answer,
        Err(error) => {
            log(&format!("error    {error}"));
            return json(502, openai::error(&error.to_string()));
        }
    };
    log(&format!(
        "response {:.2}s source={source} reply_chars={}",
        started.elapsed().as_secs_f64(),
        reply.chars().count()
    ));

    let reply = match transcript::extract(&prompt).or_else(|| transcript::extract(system_prompt)) {
        Some(transcript) => checked_reply(transcript, reply),
        None => {
            log("check    skipped, no <transcript> tags");
            reply
        }
    };

    if body["stream"] == true {
        (
            200,
            "text/event-stream",
            openai::completion_stream(model, &reply),
        )
    } else {
        json(200, openai::completion(model, &reply))
    }
}

/// Returns the reply, or the transcript unchanged if the reply doesn't look
/// like a cleaned-up copy of it.
fn checked_reply(transcript: &str, reply: String) -> String {
    let comparison = transcript::Comparison::new(transcript, &reply);
    match comparison.problem() {
        None => {
            log(&format!("check    passed {comparison}"));
            reply
        }
        Some(problem) => {
            log(&format!(
                "check    failed, replying with the transcript: {problem} {comparison}"
            ));
            transcript.to_string()
        }
    }
}

enum Source {
    Standby,
    Fresh,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Source::Standby => "standby",
            Source::Fresh => "fresh",
        })
    }
}

/// Returns Claude's reply and which kind of process gave it.
fn answer(
    standby: &Arc<Standby>,
    model: &str,
    system_prompt: &str,
    prompt: &str,
) -> Result<(String, Source), ClaudeError> {
    let fresh = || {
        Ok((
            ClaudeProcess::start(model, system_prompt)?.ask(prompt)?,
            Source::Fresh,
        ))
    };

    if !standby.matches(model, system_prompt) {
        return fresh();
    }

    let result = match standby.take() {
        Some(process) => process
            .ask(prompt)
            .map(|reply| (reply, Source::Standby))
            .or_else(|error| {
                log(&format!(
                    "standby process failed, retrying with a fresh one: {error}"
                ));
                fresh()
            }),
        None => fresh(),
    };
    standby.refill();
    result
}

fn json(status: u16, body: Value) -> (u16, &'static str, String) {
    (status, "application/json", body.to_string())
}

fn env_number<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

pub fn log(message: &str) {
    eprintln!("{message}");
}
