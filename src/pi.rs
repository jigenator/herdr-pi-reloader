use std::{time::Duration};
use tokio::task::JoinHandle;

use shell_escape::unix::escape;
use tokio::time::{self, timeout};

use crate::herdr::{AgentInfo, PanePiStatus, is_pi_running_in_pane, run_in_pane};

#[derive(Debug, PartialEq)]
pub struct ReloadSummary {
    pub(crate) reloaded: usize,
    pub(crate) skipped_non_pi: usize,
    pub(crate) skipped_unsafe_status: usize,
    pub(crate) skipped_invalid_agent_data: usize,
    pub(crate) failed: usize,
    pub(crate) errors: Vec<String>
}

#[derive(Debug)]
pub struct ResetSummary {
    pub(crate) reset: usize,
    pub(crate) skipped_non_pi: usize,
    pub(crate) skipped_unsafe_status: usize,
    pub(crate) failed: usize,
    pub(crate) errors: Vec<String>
}

#[derive(Debug)]
pub struct ResetCandidatesSummary {
   candidates: usize,
   skipped_non_pi: usize,
   skipped_unsafe_status: usize,
   skipped_invalid_agent_data: usize,
   skipped_missing_session: usize,
   skipped_invalid_session: usize,
   candidate_errors: Vec<String>
}

#[derive(Debug)]
pub struct ResetCandidate {
    pane_id: String,
    session_path: String
}

pub async fn get_reset_candidates(herdr_path: &str, agent_list: &[AgentInfo]) -> (Vec<ResetCandidate>, ResetCandidatesSummary) {
    let mut reset_candidates_summary = ResetCandidatesSummary {
        candidates: 0,
        skipped_non_pi: 0,
        skipped_unsafe_status: 0,
        skipped_invalid_agent_data: 0,
        skipped_missing_session: 0,
        skipped_invalid_session: 0,
        candidate_errors: Vec::new()
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
            },
            Err(error) => {
                reset_candidates_summary.candidate_errors.push(error);
                continue;
            }
        }

        match &value.agent_session {
            Some(session) =>{
                if &session.agent != "pi" {
                    reset_candidates_summary.skipped_invalid_session += 1;
                    continue;
                }

                if &session.kind != "path" {
                    reset_candidates_summary.skipped_invalid_session += 1;
                    continue;
                }

                if session.value.is_empty() {
                    reset_candidates_summary.skipped_invalid_session += 1;
                    continue;
                }

                let reset_candidate = ResetCandidate {
                    pane_id: value.pane_id.clone(),
                    session_path: session.value.clone()
                };

                reset_candidates.push(reset_candidate);
                reset_candidates_summary.candidates += 1;
            },
            None => {
                reset_candidates_summary.skipped_missing_session += 1;
                continue;
            }
        }

    }

    (reset_candidates, reset_candidates_summary)
}

async fn wait_until_pi_exits(herdr_path: &str, pane_id: &str) -> Result<(), String> {
    let res = timeout(Duration::from_secs(15), async {
        loop {
            let pi_running_result = is_pi_running_in_pane(herdr_path, pane_id).await;

            match pi_running_result {
                Ok(p) => {
                    if matches!(p, PanePiStatus::RunningPi) {
                        let sleep_time = time::Duration::from_millis(500);
                        tokio::time::sleep(sleep_time).await;
                    } else {
                        return Ok(());
                    }

                },
                Err(error) => {
                    return Err(error);
                }
            }
        }
    }).await;

    match res {
        Ok(wait_result) => wait_result,
        Err(error) => {
            let error_str = format!("Error, timeout reached for pane_id: {} - error: {}", pane_id, error);
            Err(error_str)
        },
    }
}

pub async fn reset_one_candidate(herdr_path: &str, candidate: &ResetCandidate) -> Result<(), String> {

    let quit_result = run_in_pane(herdr_path, candidate.pane_id.as_str(), "/quit").await;

    match quit_result {
        Ok(_) => (),
        Err(error) => {
            let error_str = format!("Error with resetting pane: {} - error: {}", candidate.pane_id, error);
            return Err(error_str);
        }
    }

    let wait_result = wait_until_pi_exits(herdr_path, &candidate.pane_id).await;

    match wait_result {
        Ok(_) => (),
        Err(error) => {
            let error_str = format!("Error with exiting pi on pane: {} - error: {}", candidate.pane_id, error);
            return Err(error_str);
        }
    }

    let safe_session_path = escape(std::borrow::Cow::Borrowed(&candidate.session_path));
    let start_command = format!("pi --session {}", safe_session_path);

    let start_result = run_in_pane(herdr_path, candidate.pane_id.as_str(), &start_command).await;

    match start_result {
        Ok(_) => Ok(()),
        Err(error) => {
            let error_str = format!("Error with starting pi on pane: {} - error: {}", candidate.pane_id, error);
            Err(error_str)
        }
    }
}

pub(crate) async fn reset_all_pi(herdr_path: &str, agents: &[AgentInfo]) -> ResetSummary {
    let (candidates, candidates_summary) = get_reset_candidates(herdr_path, agents).await;

    let mut reset_summary = ResetSummary {
        reset: 0,
        skipped_non_pi: candidates_summary.skipped_non_pi,
        skipped_unsafe_status: candidates_summary.skipped_unsafe_status,
        failed: candidates_summary.skipped_invalid_agent_data + candidates_summary.skipped_missing_session + candidates_summary.skipped_invalid_session,
        errors: Vec::new()
    };

    if candidates_summary.skipped_invalid_agent_data > 0 {
        reset_summary.errors.push(format!(
            "{} reset candidate(s) had invalid agent data",
            candidates_summary.skipped_invalid_agent_data
        ));
    }

    if candidates_summary.skipped_missing_session > 0 {
        reset_summary.errors.push(format!(
            "{} reset candidate(s) had no session data",
            candidates_summary.skipped_missing_session
        ));
    }

    if candidates_summary.skipped_invalid_session > 0 {
        reset_summary.errors.push(format!(
            "{} reset candidate(s) had invalid session data",
            candidates_summary.skipped_invalid_session
        ));
    }

    if !candidates_summary.candidate_errors.is_empty() {
        reset_summary.failed += candidates_summary.candidate_errors.len();
        reset_summary.errors.extend(candidates_summary.candidate_errors);
    }

    let reset_tasks: Vec<JoinHandle<Result<(), String>>> = candidates.into_iter().map(|candidate| {
        let herdr_path_c = herdr_path.to_string();
        tokio::spawn(async move {
            let res = reset_one_candidate(&herdr_path_c, &candidate).await;
            return res;
        })
    }).collect();

    for task in reset_tasks {
        let task_res = task.await;
        match task_res {
            Ok(res) => match res {
                Ok(_) => reset_summary.reset += 1,
                Err(error) => {
                    reset_summary.failed += 1;
                    reset_summary.errors.push(error);
                },
            },
            Err(join_error) => {
                let error_str = format!("Join error: {}", join_error);
                reset_summary.failed += 1;
                reset_summary.errors.push(error_str);
            },
        }
    }

    return reset_summary;
}

pub async fn reload_all_pi(herdr_path: &str, agents: &[AgentInfo]) -> ReloadSummary {
    let mut reload_summary = ReloadSummary {
        reloaded: 0,
        skipped_non_pi: 0,
        skipped_unsafe_status: 0,
        skipped_invalid_agent_data: 0,
        failed: 0,
        errors: Vec::new()
    };

    for (_index, agent) in agents.iter().enumerate() {

        let pane_id = agent.pane_id.as_str();
        let agent_name = agent.agent.as_str();
        let agent_status = agent.agent_status.as_str();

        if agent_name != "pi" {
            reload_summary.skipped_non_pi += 1;
            continue;
        }

        let required_fields = [
            ("pane_id", pane_id),
            ("agent_status", agent_status),
        ];

        let mut invalid_agent_data = false;
        for (name, value) in required_fields {
            if value.is_empty() {
                invalid_agent_data = true;
                reload_summary.errors.push(format!("Pi agent is missing {}", name));
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
            },
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

    return reload_summary;
}
