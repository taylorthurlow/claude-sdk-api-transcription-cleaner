//! Runs the server as a launchd agent, so it starts at login.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::thread;
use std::time::Duration;

const LABEL: &str = concat!("local.", env!("CARGO_PKG_NAME"));

/// Environment variables copied from the current shell into the launchd agent.
const PASSED_ENVIRONMENT: &[&str] = &["PATH", "PORT", "IDLE_TIMEOUT_SECONDS"];

/// Installs and loads the launchd agent, reloading it if it's already loaded.
pub fn start() -> Result<(), String> {
    let plist_path = plist_path()?;
    let plist = plist()?;
    fs::create_dir_all(plist_path.parent().expect("plist path has a parent"))
        .and_then(|()| fs::write(&plist_path, plist))
        .map_err(|error| format!("could not write {}: {error}", plist_path.display()))?;

    if is_loaded()? {
        launchctl(&["bootout", &service_target()?])?;
        wait_until_unloaded()?;
    }
    launchctl(&["bootstrap", &domain()?, &plist_path.to_string_lossy()])?;

    println!("Started. It will also start at login.");
    println!("Log: {}", log_path()?.display());
    Ok(())
}

/// Unloads the launchd agent and removes it, so it doesn't start at login.
pub fn stop() -> Result<(), String> {
    let was_loaded = is_loaded()?;
    if was_loaded {
        launchctl(&["bootout", &service_target()?])?;
    }

    let plist_path = plist_path()?;
    if plist_path.exists() {
        fs::remove_file(&plist_path)
            .map_err(|error| format!("could not remove {}: {error}", plist_path.display()))?;
    }

    if was_loaded {
        println!("Stopped. It won't start at login until you run `start` again.");
    } else {
        println!("Not running.");
    }
    Ok(())
}

/// Prints whether the server is running, and returns true if it is.
pub fn status() -> Result<bool, String> {
    let output = run_launchctl(&["print", &service_target()?])?;
    if !output.status.success() {
        println!("Not running.");
        return Ok(false);
    }

    let description = String::from_utf8_lossy(&output.stdout);
    let field = |name: &str| {
        description
            .lines()
            .find_map(|line| line.trim().strip_prefix(name)?.strip_prefix(" = "))
            .map(str::to_string)
    };

    let running = field("state").as_deref() == Some("running");
    if running {
        println!("Running (pid {}).", field("pid").unwrap_or_default());
    } else {
        let exit = field("last exit code").unwrap_or_else(|| "unknown".into());
        println!("Installed but not running. Last exit code: {exit}.");
    }
    println!("Log: {}", log_path()?.display());
    Ok(running)
}

fn plist() -> Result<String, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not find this executable's path: {error}"))?;
    let environment: String = PASSED_ENVIRONMENT
        .iter()
        .filter_map(|name| Some((name, std::env::var(name).ok()?)))
        .map(|(name, value)| {
            format!(
                "        <key>{}</key>\n        <string>{}</string>\n",
                escape(name),
                escape(&value)
            )
        })
        .collect();
    let log = escape(&log_path()?.to_string_lossy());

    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{executable}</string>
        <string>serve</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
{environment}    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
        label = LABEL,
        executable = escape(&executable.to_string_lossy()),
    ))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn is_loaded() -> Result<bool, String> {
    Ok(run_launchctl(&["print", &service_target()?])?
        .status
        .success())
}

/// Waits for launchd to finish unloading the agent, which happens in the
/// background after `bootout` returns.
fn wait_until_unloaded() -> Result<(), String> {
    for _ in 0..50 {
        if !is_loaded()? {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err("timed out waiting for the running server to stop".into())
}

/// Runs launchctl and returns an error if it fails.
fn launchctl(args: &[&str]) -> Result<(), String> {
    let output = run_launchctl(args)?;
    if output.status.success() {
        return Ok(());
    }
    let error_output = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "launchctl {} failed: {}",
        args.join(" "),
        error_output.trim()
    ))
}

fn run_launchctl(args: &[&str]) -> Result<Output, String> {
    Command::new("launchctl")
        .args(args)
        .output()
        .map_err(|error| format!("could not run launchctl: {error}"))
}

/// Returns the launchd domain for the logged-in user's session, like `gui/501`.
fn domain() -> Result<String, String> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .map_err(|error| format!("could not get user ID: {error}"))?;
    Ok(format!(
        "gui/{}",
        String::from_utf8_lossy(&output.stdout).trim()
    ))
}

fn service_target() -> Result<String, String> {
    Ok(format!("{}/{LABEL}", domain()?))
}

fn plist_path() -> Result<PathBuf, String> {
    Ok(home()?.join(format!("Library/LaunchAgents/{LABEL}.plist")))
}

fn log_path() -> Result<PathBuf, String> {
    Ok(home()?.join(concat!("Library/Logs/", env!("CARGO_PKG_NAME"), ".log")))
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".into())
}
