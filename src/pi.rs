use std::time::Duration;

use shell_escape::unix::escape;
use tokio::time::{self, timeout};

use crate::herdr::{
    AgentInfo, PanePiStatus, focus_pane, get_agent_list, get_pane_process_info,
    is_pi_running_in_pane, original_pane, run_in_pane,
};
use crate::progress::Progress;

#[derive(Debug, PartialEq)]
pub struct ReloadSummary {
    pub(crate) reloaded: usize,
    pub(crate) skipped_non_pi: usize,
    pub(crate) skipped_unsafe_status: usize,
    pub(crate) skipped_invalid_agent_data: usize,
    pub(crate) failed: usize,
    pub(crate) errors: Vec<String>,
}

#[derive(Debug)]
pub struct ResetSummary {
    pub(crate) reset: usize,
    pub(crate) visited: usize,
    pub(crate) skipped_non_pi: usize,
    pub(crate) skipped_unsafe_status: usize,
    pub(crate) failed: usize,
    pub(crate) errors: Vec<String>,
    pub(crate) outcomes: Vec<String>,
}

#[derive(Debug)]
pub struct ResetCandidatesSummary {
    candidates: usize,
    skipped_non_pi: usize,
    skipped_unsafe_status: usize,
    skipped_invalid_agent_data: usize,
    skipped_missing_session: usize,
    skipped_invalid_session: usize,
    candidate_errors: Vec<String>,
}

#[derive(Debug)]
pub struct ResetCandidate {
    pane_id: String,
    session_path: String,
    was_idle: bool,
}

impl ResetCandidate {
    fn matches_session(&self, agent: &AgentInfo) -> bool {
        agent.agent == "pi"
            && agent.agent_session.as_ref().is_some_and(|session| {
                session.agent == "pi"
                    && session.kind == "path"
                    && session.value == self.session_path
            })
    }
}

pub async fn get_reset_candidates(
    herdr_path: &str,
    agent_list: &[AgentInfo],
) -> (Vec<ResetCandidate>, ResetCandidatesSummary) {
    let mut reset_candidates_summary = ResetCandidatesSummary {
        candidates: 0,
        skipped_non_pi: 0,
        skipped_unsafe_status: 0,
        skipped_invalid_agent_data: 0,
        skipped_missing_session: 0,
        skipped_invalid_session: 0,
        candidate_errors: Vec::new(),
    };

    let mut reset_candidates: Vec<ResetCandidate> = Vec::new();

    for value in agent_list {
        if value.agent != "pi" {
            reset_candidates_summary.skipped_non_pi += 1;
            continue;
        }

        if value.pane_id.is_empty() {
            reset_candidates_summary.skipped_invalid_agent_data += 1;
            continue;
        }

        if value.agent_status.is_empty() {
            reset_candidates_summary.skipped_invalid_agent_data += 1;
            continue;
        }

        if value.agent_status != "done" && value.agent_status != "idle" {
            reset_candidates_summary.skipped_unsafe_status += 1;
            continue;
        }

        match is_pi_running_in_pane(herdr_path, &value.pane_id).await {
            Ok(pane_status) => {
                if matches!(pane_status, PanePiStatus::NonPi) {
                    reset_candidates_summary.skipped_non_pi += 1;
                    continue;
                }
            }
            Err(error) => {
                reset_candidates_summary.candidate_errors.push(error);
                continue;
            }
        }

        match &value.agent_session {
            Some(session) => {
                if &session.agent != "pi" {
                    reset_candidates_summary.skipped_invalid_session += 1;
                    continue;
                }

                if &session.kind != "path" {
                    reset_candidates_summary.skipped_invalid_session += 1;
                    continue;
                }

                if session.value.is_empty() || session.value.chars().any(char::is_control) {
                    reset_candidates_summary.skipped_invalid_session += 1;
                    continue;
                }

                let reset_candidate = ResetCandidate {
                    pane_id: value.pane_id.clone(),
                    session_path: session.value.clone(),
                    was_idle: value.agent_status == "idle",
                };

                reset_candidates.push(reset_candidate);
                reset_candidates_summary.candidates += 1;
            }
            None => {
                reset_candidates_summary.skipped_missing_session += 1;
                continue;
            }
        }
    }

    (reset_candidates, reset_candidates_summary)
}

async fn wait_until_pi_exits(herdr_path: &str, pane_id: &str) -> Result<(), String> {
    timeout(Duration::from_secs(15), async {
        loop {
            // Don't paste a shell command into an unrelated foreground process, or
            // accept stale lifecycle data from the old Pi as the resumed session.
            if get_pane_process_info(herdr_path, pane_id).await?.is_shell()
                && !get_agent_list(herdr_path)
                    .await?
                    .iter()
                    .any(|a| a.pane_id == pane_id && a.agent == "pi")
            {
                return Ok(());
            }
            time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .map_err(|_| format!("{pane_id}: shell not ready after /quit; no start command sent"))?
}

async fn wait_until_pi_ready(herdr_path: &str, candidate: &ResetCandidate) -> Result<(), String> {
    timeout(Duration::from_secs(30), async {
        loop {
            let agents = get_agent_list(herdr_path).await?;
            if let Some(agent) = agents.iter().find(|a| a.pane_id == candidate.pane_id)
                && candidate.matches_session(agent)
                && matches!(agent.agent_status.as_str(), "idle" | "done")
                && get_pane_process_info(herdr_path, &candidate.pane_id)
                    .await?
                    .is_pi()
            {
                return Ok(());
            }
            time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .map_err(|_| {
        format!(
            "{}: resumed Pi/session not ready after 30s; not focused",
            candidate.pane_id
        )
    })?
}

async fn reset_one_candidate(
    herdr_path: &str,
    candidate: &ResetCandidate,
    progress: &Progress,
    completed: usize,
    total: usize,
) -> Result<(), String> {
    progress.update(completed, total, &format!("{} stopping", candidate.pane_id))?;
    run_in_pane(herdr_path, &candidate.pane_id, "/quit").await?;
    wait_until_pi_exits(herdr_path, &candidate.pane_id).await?;
    progress.update(completed, total, &format!("{} starting", candidate.pane_id))?;
    let safe_session_path = escape(std::borrow::Cow::Borrowed(&candidate.session_path));
    run_in_pane(
        herdr_path,
        &candidate.pane_id,
        &format!("pi --session {safe_session_path}"),
    )
    .await?;
    progress.update(completed, total, &format!("{} waiting", candidate.pane_id))?;
    wait_until_pi_ready(herdr_path, candidate).await
}

pub(crate) async fn reset_all_pi(
    herdr_path: &str,
    agents: &[AgentInfo],
) -> Result<ResetSummary, String> {
    let progress = Progress::start()?;
    let (candidates, counts) = get_reset_candidates(herdr_path, agents).await;
    let mut summary = ResetSummary {
        reset: 0,
        visited: 0,
        skipped_non_pi: counts.skipped_non_pi,
        skipped_unsafe_status: counts.skipped_unsafe_status,
        failed: counts.skipped_invalid_agent_data
            + counts.skipped_missing_session
            + counts.skipped_invalid_session
            + counts.candidate_errors.len(),
        errors: counts.candidate_errors,
        outcomes: Vec::new(),
    };
    for (count, reason) in [
        (counts.skipped_invalid_agent_data, "invalid agent data"),
        (counts.skipped_missing_session, "no session data"),
        (counts.skipped_invalid_session, "invalid session data"),
    ] {
        if count > 0 {
            summary
                .errors
                .push(format!("{count} reset candidate(s) had {reason}"));
        }
    }
    if candidates.is_empty() {
        return Ok(summary);
    }
    let origin = original_pane()?;
    let total = candidates.len();
    progress.update(0, total, "starting; don't switch panes")?;
    // Let the one-second status-bar poll display the warning before any mutation.
    time::sleep(Duration::from_millis(1100)).await;
    let mut focus_attempted = false;
    let mut processed = 0;
    let work = async {
        for (completed, candidate) in candidates.iter().enumerate() {
            processed = completed + 1;
            progress.update(completed, total, &format!("{} checking", candidate.pane_id))?;
            // A later pane may have become busy or changed sessions while earlier panes reset.
            let current = get_agent_list(herdr_path).await?;
            let agent = current.iter().find(|a| a.pane_id == candidate.pane_id);
            let Some(agent) = agent.filter(|a| a.agent == "pi") else {
                summary.skipped_non_pi += 1;
                summary
                    .outcomes
                    .push(format!("{} skipped (no longer Pi)", candidate.pane_id));
                continue;
            };
            if !matches!(agent.agent_status.as_str(), "idle" | "done") {
                summary.skipped_unsafe_status += 1;
                summary
                    .outcomes
                    .push(format!("{} skipped (now busy)", candidate.pane_id));
                continue;
            }
            if !candidate.matches_session(agent) {
                summary.failed += 1;
                summary
                    .errors
                    .push(format!("{} changed session; not reset", candidate.pane_id));
                continue;
            }
            if !get_pane_process_info(herdr_path, &candidate.pane_id)
                .await?
                .is_pi()
            {
                summary.skipped_non_pi += 1;
                summary
                    .outcomes
                    .push(format!("{} skipped (not Pi)", candidate.pane_id));
                continue;
            }
            match reset_one_candidate(herdr_path, candidate, &progress, completed, total).await {
                Ok(()) => {
                    summary.reset += 1;
                    summary
                        .outcomes
                        .push(format!("{} restarted", candidate.pane_id));
                    if candidate.was_idle {
                        progress.update(
                            completed,
                            total,
                            &format!("{} viewing", candidate.pane_id),
                        )?;
                        focus_attempted = true;
                        focus_pane(&candidate.pane_id)?;
                        // ponytail: no public client acknowledgement receipt. Allow one
                        // second for a focused local client to render; not a remote/unfocused guarantee.
                        time::sleep(Duration::from_secs(1)).await;
                        summary.visited += 1;
                    }
                }
                Err(error) => {
                    summary.failed += 1;
                    summary
                        .errors
                        .push(format!("{}: {error}", candidate.pane_id));
                    summary
                        .outcomes
                        .push(format!("{} failed", candidate.pane_id));
                }
            }
            progress.update(completed + 1, total, "continuing")?;
        }
        Ok::<(), String>(())
    }
    .await;
    if let Err(error) = work {
        summary.failed += 1;
        summary.errors.push(error);
    }
    // Restore even after a failed/ambiguous focus or a later reset error. Never pick
    // some other pane if the original disappeared; report that restoration failed.
    if focus_attempted {
        let _ = progress.update(processed, total, "returning to original pane");
        if let Err(error) = focus_pane(&origin) {
            summary.failed += 1;
            summary
                .errors
                .push(format!("Could not restore original pane {origin}: {error}"));
        } else {
            time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(summary)
}

pub async fn reload_all_pi(herdr_path: &str, agents: &[AgentInfo]) -> ReloadSummary {
    let mut reload_summary = ReloadSummary {
        reloaded: 0,
        skipped_non_pi: 0,
        skipped_unsafe_status: 0,
        skipped_invalid_agent_data: 0,
        failed: 0,
        errors: Vec::new(),
    };

    for agent in agents {
        let pane_id = agent.pane_id.as_str();
        let agent_name = agent.agent.as_str();
        let agent_status = agent.agent_status.as_str();

        if agent_name != "pi" {
            reload_summary.skipped_non_pi += 1;
            continue;
        }

        let required_fields = [("pane_id", pane_id), ("agent_status", agent_status)];

        let mut invalid_agent_data = false;
        for (name, value) in required_fields {
            if value.is_empty() {
                invalid_agent_data = true;
                reload_summary
                    .errors
                    .push(format!("Pi agent is missing {}", name));
            }
        }

        if invalid_agent_data {
            reload_summary.skipped_invalid_agent_data += 1;
            continue;
        }

        match is_pi_running_in_pane(herdr_path, pane_id).await {
            Ok(pane_status) => {
                if matches!(pane_status, PanePiStatus::NonPi) {
                    reload_summary.skipped_non_pi += 1;
                    continue;
                }
            }
            Err(error) => {
                reload_summary.failed += 1;
                reload_summary.errors.push(error);
                continue;
            }
        }

        if agent_status == "done" || agent_status == "idle" {
            let reload_pane_status = run_in_pane(herdr_path, pane_id, "/reload").await;

            match reload_pane_status {
                Ok(_) => reload_summary.reloaded += 1,
                Err(error) => {
                    reload_summary.failed += 1;
                    reload_summary.errors.push(error);
                }
            }
        } else {
            reload_summary.skipped_unsafe_status += 1;
            continue;
        }
    }

    reload_summary
}
