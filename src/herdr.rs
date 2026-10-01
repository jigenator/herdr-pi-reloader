use std::{
    env,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub(crate) struct AgentSession {
    #[serde(default)]
    pub(crate) agent: String,
    #[serde(default)]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) value: String,
}

#[derive(Deserialize)]
pub struct AgentInfo {
    #[serde(default)]
    pub(crate) agent: String,
    #[serde(default)]
    pub(crate) agent_status: String,
    #[serde(default)]
    pub(crate) pane_id: String,
    pub(crate) agent_session: Option<AgentSession>,
}

#[derive(Deserialize)]
pub(crate) struct ProcessInfo {
    shell_pid: Option<u32>,
    #[serde(default)]
    foreground_processes: Vec<ForegroundProcess>,
}

#[derive(Deserialize)]
struct ForegroundProcess {
    pid: Option<u32>,
    #[serde(default)]
    name: String,
    argv0: Option<String>,
    argv: Option<Vec<String>>,
}

impl ProcessInfo {
    pub(crate) fn is_pi(&self) -> bool {
        self.foreground_processes.iter().any(|p| {
            p.name == "pi"
                || p.argv0
                    .as_ref()
                    .or_else(|| p.argv.as_ref().and_then(|args| args.first()))
                    .and_then(|arg| Path::new(arg).file_name())
                    .is_some_and(|name| name == "pi")
        })
    }

    pub(crate) fn is_shell(&self) -> bool {
        !self.is_pi()
            && self.shell_pid.is_some()
            && self.foreground_processes.len() == 1
            && self.foreground_processes[0].pid == self.shell_pid
    }
}

async fn command(herdr_path: &str, args: &[&str]) -> Result<Value, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(herdr_path)
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| format!("Herdr {args:?} timed out; command may have been delivered"))?
    .map_err(|error| format!("Herdr {args:?}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Herdr {args:?}: {} {}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        ));
    }
    // Mutating CLI commands such as `pane run` succeed without printing JSON.
    if output.stdout.is_empty() {
        return Ok(Value::Null);
    }
    let response: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Invalid Herdr JSON: {error}"))?;
    response_result(response)
}

fn response_result(response: Value) -> Result<Value, String> {
    if let Some(error) = response.get("error") {
        return Err(format!("Herdr: {error}"));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| "Herdr response has no result".into())
}

pub(crate) fn socket_path() -> Result<String, String> {
    env::var("HERDR_SOCKET_PATH")
        .ok()
        .filter(|path| !path.is_empty())
        .ok_or_else(|| "Reset/status requires HERDR_SOCKET_PATH; run from Herdr".into())
}

// `agent focus` does not switch native clients in Herdr 0.9. Use the public pane API.
pub(crate) fn api(method: &str, params: Value) -> Result<Value, String> {
    let call = || -> Result<Value, Box<dyn std::error::Error>> {
        let mut stream = UnixStream::connect(socket_path()?)?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        writeln!(
            stream,
            "{}",
            json!({"id": "pi-reloader", "method": method, "params": params})
        )?;
        let mut line = String::new();
        BufReader::new(stream.take(1024 * 1024)).read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    };
    response_result(call().map_err(|error| format!("{method}: {error}"))?)
}

pub(crate) fn original_pane() -> Result<String, String> {
    let caller = if let Ok(context) = env::var("HERDR_PLUGIN_CONTEXT_JSON") {
        let context: Value = serde_json::from_str(&context).map_err(|error| error.to_string())?;
        Some(
            context["focused_pane_id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or("Plugin context has no original pane")?
                .to_string(),
        )
    } else {
        env::var("HERDR_PANE_ID").ok()
    };
    let result = api("pane.current", json!({"caller_pane_id": caller}))?;
    result["pane"]["pane_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "Cannot determine original pane; refusing reset".into())
}

pub(crate) fn focus_pane(pane_id: &str) -> Result<(), String> {
    let result = api("pane.focus", json!({"pane_id": pane_id}))?;
    if result["pane"]["pane_id"] != pane_id {
        return Err(format!("Focus response did not confirm pane {pane_id}"));
    }
    Ok(())
}

pub async fn run_in_pane(herdr_path: &str, pane_id: &str, text: &str) -> Result<(), String> {
    command(herdr_path, &["pane", "run", pane_id, text])
        .await
        .map(|_| ())
}

pub async fn get_agent_list(herdr_path: &str) -> Result<Vec<AgentInfo>, String> {
    let result = command(herdr_path, &["agent", "list"]).await?;
    serde_json::from_value(result["agents"].clone())
        .map_err(|error| format!("Invalid agent list: {error}"))
}

pub(crate) async fn get_pane_process_info(
    herdr_path: &str,
    pane_id: &str,
) -> Result<ProcessInfo, String> {
    let result = command(herdr_path, &["pane", "process-info", "--pane", pane_id]).await?;
    serde_json::from_value(result["process_info"].clone())
        .map_err(|error| format!("Invalid process info: {error}"))
}

pub(crate) enum PanePiStatus {
    RunningPi,
    NonPi,
}

pub async fn is_pi_running_in_pane(
    herdr_path: &str,
    pane_id: &str,
) -> Result<PanePiStatus, String> {
    Ok(
        if get_pane_process_info(herdr_path, pane_id).await?.is_pi() {
            PanePiStatus::RunningPi
        } else {
            PanePiStatus::NonPi
        },
    )
}
