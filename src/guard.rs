use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::UnixStream,
    },
    path::PathBuf,
    time::Duration,
};

use serde::Deserialize;
use serde_json::json;
use tokio::time::{self, timeout};

use crate::herdr::{AgentInfo, get_pane_process_info, socket_path};

#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    Completed,
    Draft,
    Busy,
}

#[derive(Deserialize)]
struct Reply {
    protocol: u8,
    pid: u32,
    pane_id: String,
    server: String,
    session_path: Option<String>,
    generation: String,
    status: String,
    error: Option<String>,
}

struct Guard {
    path: PathBuf,
    pid: u32,
    pane_id: String,
    server: String,
    session_path: Option<String>,
}

impl Guard {
    async fn new(herdr_path: &str, agent: &AgentInfo) -> Result<Self, String> {
        let pid = get_pane_process_info(herdr_path, &agent.pane_id)
            .await?
            .pi_pid()
            .ok_or("Cannot identify one foreground Pi PID; no action taken")?;
        let server = socket_path()?;
        let uid = fs::metadata(&server)
            .map_err(|error| error.to_string())?
            .uid();
        let directory = PathBuf::from(format!("/tmp/herdr-pi-reloader-{uid}"));
        let path = directory.join(format!("{pid}.sock"));
        let check = || -> Result<(), Box<dyn std::error::Error>> {
            let dir = fs::symlink_metadata(&directory)?;
            let socket = fs::symlink_metadata(&path)?;
            if !dir.is_dir()
                || !socket.file_type().is_socket()
                || dir.uid() != uid
                || socket.uid() != uid
                || (dir.mode() | socket.mode()) & 0o077 != 0
            {
                return Err(
                    "Guard directory/socket is not private and owned by the Herdr user".into(),
                );
            }
            Ok(())
        };
        check().map_err(|error| {
            format!(
                "{}: guard unavailable; manual /reload from an empty editor first ({error})",
                agent.pane_id
            )
        })?;
        Ok(Self {
            path,
            pid,
            server,
            pane_id: agent.pane_id.clone(),
            session_path: agent
                .agent_session
                .as_ref()
                .filter(|s| s.agent == "pi" && s.kind == "path")
                .map(|s| s.value.clone()),
        })
    }

    fn call(&self, action: &str, generation: Option<&str>) -> Result<Reply, String> {
        let call = || -> Result<Reply, Box<dyn std::error::Error>> {
            let mut stream = UnixStream::connect(&self.path)?;
            stream.set_read_timeout(Some(Duration::from_secs(3)))?;
            stream.set_write_timeout(Some(Duration::from_secs(3)))?;
            writeln!(
                stream,
                "{}",
                json!({
                    "protocol": 1, "pid": self.pid, "pane_id": self.pane_id,
                    "server": self.server, "session_path": self.session_path,
                    "action": action, "generation": generation,
                })
            )?;
            let mut line = String::new();
            BufReader::new(stream.take(8192)).read_line(&mut line)?;
            Ok(serde_json::from_str(&line)?)
        };
        let reply = call().map_err(|error| {
            format!(
                "{}: guard {action} failed ({error}); no terminal-input fallback",
                self.pane_id
            )
        })?;
        if reply.protocol != 1
            || reply.pid != self.pid
            || reply.pane_id != self.pane_id
            || reply.server != self.server
            || reply.session_path != self.session_path
            || reply.generation.is_empty()
            || generation.is_some_and(|g| g != reply.generation)
        {
            return Err(format!(
                "{}: guard identity/session/runtime mismatch",
                self.pane_id
            ));
        }
        if reply.status == "error" {
            return Err(format!(
                "{}: {}",
                self.pane_id,
                reply.error.unwrap_or("Guard refused action".into())
            ));
        }
        Ok(reply)
    }
}

pub(crate) async fn control(
    herdr_path: &str,
    agent: &AgentInfo,
    action: &str,
) -> Result<Outcome, String> {
    let guard = Guard::new(herdr_path, agent).await?;
    let before = guard.call("probe", None)?;
    match before.status.as_str() {
        "draft" => return Ok(Outcome::Draft),
        "busy" => return Ok(Outcome::Busy),
        "ready" => {}
        _ => return Err("Unexpected guard probe response; no action taken".into()),
    }
    let reply = guard.call(action, Some(&before.generation))?;
    match reply.status.as_str() {
        "draft" => return Ok(Outcome::Draft),
        "busy" => return Ok(Outcome::Busy),
        "accepted" => {}
        _ => return Err("Guard did not accept action; no terminal-input fallback".into()),
    }
    if action == "reload" {
        // Acceptance is not completion. A replacement extension runtime must answer.
        timeout(Duration::from_secs(30), async {
            loop {
                if let Ok(after) = guard.call("probe", None)
                    && after.generation != before.generation
                    && matches!(after.status.as_str(), "ready" | "draft" | "busy")
                {
                    break;
                }
                time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| {
            format!(
                "{}: reload did not confirm a new guard runtime",
                agent.pane_id
            )
        })?;
    }
    Ok(Outcome::Completed)
}
