use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::log;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug)]
pub struct ClaudeError(String);

impl fmt::Display for ClaudeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<io::Error> for ClaudeError {
    fn from(error: io::Error) -> Self {
        ClaudeError(format!("could not talk to claude: {error}"))
    }
}

impl From<serde_json::Error> for ClaudeError {
    fn from(error: serde_json::Error) -> Self {
        ClaudeError(format!("unexpected output from claude: {error}"))
    }
}

/// A `claude -p` process that waits for a prompt on standard input.
///
/// The process is killed when this is dropped.
pub struct ClaudeProcess {
    child: Child,
}

impl ClaudeProcess {
    pub fn start(model: &str, system_prompt: &str) -> Result<Self, ClaudeError> {
        let child = Command::new("claude")
            .args(["-p", "--model", model, "--system-prompt", system_prompt])
            // Plain text input gives up if nothing arrives within a few seconds.
            // This mode waits.
            .args([
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
            ])
            .args(["--verbose", "--tools", "", "--strict-mcp-config"])
            .args(["--setting-sources", "", "--disable-slash-commands"])
            .arg("--no-session-persistence")
            .current_dir(working_directory())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| ClaudeError(format!("could not start claude: {error}")))?;
        Ok(ClaudeProcess { child })
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Sends the prompt and returns Claude's reply, then kills the process.
    pub fn ask(mut self, prompt: &str) -> Result<String, ClaudeError> {
        let mut stdin = self.child.stdin.take().expect("stdin is piped");
        let stdout = self.child.stdout.take().expect("stdout is piped");
        let stderr = self.child.stderr.take().expect("stderr is piped");

        let message = json!({ "type": "user", "message": { "role": "user", "content": prompt } });
        writeln!(stdin, "{message}")?;
        stdin.flush()?;

        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || sender.send(read_reply(stdout, stderr)));
        match receiver.recv_timeout(REQUEST_TIMEOUT) {
            Ok(reply) => reply,
            Err(RecvTimeoutError::Timeout) => Err(ClaudeError(format!(
                "claude did not reply within {} seconds",
                REQUEST_TIMEOUT.as_secs()
            ))),
            Err(RecvTimeoutError::Disconnected) => {
                Err(ClaudeError("claude's output reader stopped".into()))
            }
        }
    }
}

impl Drop for ClaudeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Returns the text of the `result` event from `claude`'s stream-json output.
fn read_reply(stdout: ChildStdout, mut stderr: ChildStderr) -> Result<String, ClaudeError> {
    for line in BufReader::new(stdout).lines() {
        let event: Value = serde_json::from_str(&line?)?;
        if event["type"] != "result" {
            continue;
        }
        let text = event["result"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        if event["is_error"] == true {
            return Err(ClaudeError(if text.is_empty() {
                "claude reported an error".into()
            } else {
                text
            }));
        }
        return Ok(text);
    }

    let mut error_output = String::new();
    stderr.read_to_string(&mut error_output)?;
    let error_output = error_output.trim();
    Err(ClaudeError(if error_output.is_empty() {
        "claude exited without replying".into()
    } else {
        error_output.into()
    }))
}

/// Returns an empty directory to run `claude` in, so it finds no project files.
fn working_directory() -> &'static Path {
    static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
    DIRECTORY.get_or_init(|| {
        let directory = std::env::temp_dir().join(env!("CARGO_PKG_NAME"));
        let _ = std::fs::create_dir_all(&directory);
        directory
    })
}

/// Keeps a `claude` process started ahead of time, so a request doesn't wait
/// for it to start up.
///
/// The process uses the model and system prompt of the most recent successful
/// request. It is ended after `idle_timeout` without a request.
pub struct Standby {
    idle_timeout: Duration,
    state: Mutex<StandbyState>,
}

struct StandbyState {
    process: Option<ClaudeProcess>,
    model: String,
    system_prompt: String,
    last_used_at: Instant,
}

impl Standby {
    pub fn new(model: &str, system_prompt: &str, idle_timeout: Duration) -> Arc<Self> {
        Arc::new(Standby {
            idle_timeout,
            state: Mutex::new(StandbyState {
                process: None,
                model: model.into(),
                system_prompt: system_prompt.into(),
                last_used_at: Instant::now(),
            }),
        })
    }

    /// Returns the waiting process, or `None` if there isn't a running one
    /// that uses this model and system prompt.
    pub fn take(&self, model: &str, system_prompt: &str) -> Option<ClaudeProcess> {
        let mut process = {
            let mut state = self.state.lock().unwrap();
            if state.model != model || state.system_prompt != system_prompt {
                return None;
            }
            state.process.take()?
        };
        if process.is_running() {
            Some(process)
        } else {
            log("standby process had exited");
            None
        }
    }

    /// Makes the waiting process use this model and system prompt, then
    /// refills. A waiting process that uses others is ended.
    pub fn refill_with(self: &Arc<Self>, model: &str, system_prompt: &str) {
        let replaced = {
            let mut state = self.state.lock().unwrap();
            if state.model == model && state.system_prompt == system_prompt {
                None
            } else {
                state.model = model.into();
                state.system_prompt = system_prompt.into();
                log(&format!(
                    "standby  now uses model={model} system_chars={}",
                    system_prompt.chars().count()
                ));
                state.process.take()
            }
        };
        drop(replaced);
        self.refill();
    }

    /// Starts a waiting process if there isn't one, and restarts the idle
    /// countdown.
    pub fn refill(self: &Arc<Self>) {
        {
            let mut state = self.state.lock().unwrap();
            if state.process.is_none() {
                match ClaudeProcess::start(&state.model, &state.system_prompt) {
                    Ok(process) => state.process = Some(process),
                    Err(error) => log(&format!("could not start standby process: {error}")),
                }
            }
            state.last_used_at = Instant::now();
        }

        let standby = Arc::clone(self);
        thread::spawn(move || {
            thread::sleep(standby.idle_timeout);
            standby.expire_if_idle();
        });
    }

    fn expire_if_idle(&self) {
        let process = {
            let mut state = self.state.lock().unwrap();
            if state.last_used_at.elapsed() < self.idle_timeout {
                return;
            }
            state.process.take()
        };
        if process.is_some() {
            drop(process);
            log("standby process ended after inactivity");
        }
    }
}
