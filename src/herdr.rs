use std::path::Path;

use serde::{Deserialize};

#[derive(Deserialize)]
pub(crate) struct AgentSession {
    #[serde(default)]
    pub(crate) agent: String,
    #[serde(default)]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) value: String
}

#[derive(Deserialize)]
pub struct AgentInfo {
    #[serde(default)]
    pub(crate) agent: String,
    #[serde(default)]
    pub(crate) agent_status: String,
    #[serde(default)]
    pub(crate) pane_id: String,
    pub(crate) agent_session: Option<AgentSession>
}

#[derive(Deserialize)]
struct AgentListResult {
    agents: Vec<AgentInfo>
}

#[derive(Deserialize)]
struct AgentListResponse {
    result: AgentListResult
}

#[derive(Deserialize)]
struct ProcessInfoResponse {
    result: ProcessInfoResult
}

#[derive(Deserialize)]
struct ProcessInfoResult {
    process_info: ProcessInfo
}

#[derive(Deserialize)]
struct ProcessInfo {
    foreground_processes: Vec<ForegroundProcess>
}

#[derive(Deserialize)]
struct ForegroundProcess {
    #[serde(default)]
    name: String,
    #[serde(default)]
    argv0: Option<String>,
    #[serde(default)]
    argv: Vec<String>
}

pub async fn run_in_pane(herdr_path: &str, pane_id: &str, command: &str) -> Result<(), String> {
    match tokio::process::Command::new(herdr_path).args(["pane", "run", pane_id, command]).output().await {
        Ok(output) => {
            if output.status.success() {
                Ok(())
            } else {
                let std_error = String::from_utf8_lossy(&output.stderr);
                let std_out = String::from_utf8_lossy(&output.stdout);

                let error_message = format!("std_err: {std_error} - std_out: {std_out}");
                let error_str = format!("Error in executing command: {}: on pane: {} - error: {}", command, pane_id, error_message);
                Err(error_str)
            }
        },
        Err(error) => {
            let error_str = format!("Error in executing command: {}: on pane: {} - error: {}", command, pane_id, error);
            Err(error_str)
        }
    }

}

pub async fn get_agent_list(herdr_path: &str) -> Result<Vec<AgentInfo>, String> {
    match tokio::process::Command::new(herdr_path).args(["agent", "list"]).output().await {
        Ok(output) => {
            if output.status.success() {
                let output_str = String::from_utf8_lossy(&output.stdout);

                let json_result: Result<AgentListResponse, serde_json::Error> =
                    serde_json::from_str(&output_str);

                match json_result {
                    Ok(value) => {
                        let agents = value.result.agents;
                        Ok(agents)
                    },
                    Err(error) => {
                        let error_text = format!("Error with parsing agents json: {}", error);
                        Err(error_text)
                    }
                }
            } else {
                let error_str = String::from_utf8_lossy(&output.stderr).to_string();
                let error_text = format!("Herdr agent list output error: {}", error_str);
                Err(error_text)
            }

        },
        Err(error) => {
            let error_str = error.to_string();
            let error_text = format!("Failed to run herdr agent list: {}", error_str);
            Err(error_text)
        }
    }
}

async fn get_pane_process_info(herdr_path: &str, pane_id: &str) -> Result<Vec<ForegroundProcess>, String> {
    match tokio::process::Command::new(herdr_path).args(["pane", "process-info", "--pane", pane_id]).output().await {
        Ok(output) => {
            if output.status.success() {
                let output_str = String::from_utf8_lossy(&output.stdout);

                let json_result: Result<ProcessInfoResponse, serde_json::Error>  = serde_json::from_str(&output_str);

                match json_result {
                    Ok(value) => {
                        let process_info = value.result.process_info.foreground_processes;
                        Ok(process_info)
                    },
                    Err(error) => {
                        let error_text = format!("Error with parsing processes json: {}", error);
                        Err(error_text)
                    }
                }
            } else {
                let error_str = String::from_utf8_lossy(&output.stderr).to_string();
                let error_text = format!("Herdr process output error: {}", error_str);
                Err(error_text)
            }

        },
        Err(error) => {
            let error_str = error.to_string();
            let error_text = format!("Failed to run herdr pane process info: {}", error_str);
            Err(error_text)
        }
    }
}

pub(crate) enum PanePiStatus {
    RunningPi,
    NonPi
}

pub async fn is_pi_running_in_pane(herdr_path: &str, pane_id: &str) -> Result<PanePiStatus, String> {
    match get_pane_process_info(herdr_path, pane_id).await {
        Ok(process_info) => {
            for p in process_info {
                if p.name == "pi" {
                    return Ok(PanePiStatus::RunningPi);
                }
                if p.argv0
                    .as_ref()
                    .or_else(|| p.argv.first())
                    .and_then(|arg| Path::new(arg).file_name())
                    .is_some_and(|name| name == "pi")
                {
                    return Ok(PanePiStatus::RunningPi);
                }
            }
            Ok(PanePiStatus::NonPi)
        },
        Err(error) => {
            let error_str = format!("Failed to check if pi is running in pane_id: {}, error: {}",pane_id, error);
            Err(error_str)
        }
    }
}
