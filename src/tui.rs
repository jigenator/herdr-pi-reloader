use crossterm::event::KeyCode::{Char, Down, Enter, Esc, Up};
use crossterm::event::{self, Event, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::{DefaultTerminal, Frame};
use std::time::Duration;
use std::{env, io};
use tokio::task::JoinHandle;

use crate::herdr::get_agent_list;
use crate::pi::{ReloadSummary, ResetSummary, reload_all_pi, reset_all_pi};
use crate::render::{
    render_error, render_menu, render_reload_result, render_reset_result, render_running_reload,
    render_running_reset,
};

pub async fn run() -> io::Result<()> {
    let mut terminal = ratatui::try_init()?;

    let app_result = app(&mut terminal).await;

    let restore_result = ratatui::try_restore();

    match restore_result {
        Ok(()) => app_result,
        Err(error) => Err(error),
    }
}

enum Screen {
    Menu,
    RunningReload,
    RunningReset,
    ReloadResult(ReloadSummary),
    ResetResult(ResetSummary),
    Error(String),
}

struct AppState {
    selected: usize,
    screen: Screen,
    reset_task: Option<JoinHandle<Result<ResetSummary, String>>>,
    spinner_frame: usize,
}

async fn app(terminal: &mut DefaultTerminal) -> std::io::Result<()> {
    let mut state = AppState {
        selected: 0,
        screen: Screen::Menu,
        reset_task: None,
        spinner_frame: 0,
    };

    loop {
        let reset_task_finished = match &state.reset_task {
            Some(task) => task.is_finished(),
            None => false,
        };

        if reset_task_finished {
            let task = state.reset_task.take().unwrap();

            match task.await {
                Ok(join_result) => match join_result {
                    Ok(reset_result) => {
                        state.screen = Screen::ResetResult(reset_result);
                    }
                    Err(error) => {
                        state.screen = Screen::Error(error);
                    }
                },
                Err(error) => {
                    state.screen = Screen::Error(format!("Join handle error: {}", error));
                }
            }
        }

        if matches!(state.screen, Screen::RunningReset) {
            state.spinner_frame += 1;
        }

        terminal.draw(|frame| render(frame, &state))?;

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            if !matches!(state.screen, Screen::RunningReset) {
                match key.code {
                    Char('q') | Esc => break Ok(()),
                    Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        break Ok(());
                    }
                    _ => {}
                }
            }

            match &state.screen {
                Screen::Menu => match key.code {
                    Char('j') | Down => state.selected = 1,
                    Char('k') | Up => state.selected = 0,
                    Enter => {
                        if state.selected == 0 {
                            state.screen = Screen::RunningReload;
                            terminal.draw(|frame| render(frame, &state))?;

                            let herdr_path_result = env::var("HERDR_BIN_PATH");

                            let herdr_path = match herdr_path_result {
                                Ok(path) => path,
                                Err(_error) => String::from("herdr"),
                            };

                            match get_agent_list(&herdr_path).await {
                                Ok(agents) => {
                                    let reload_summary = reload_all_pi(&herdr_path, &agents).await;

                                    state.screen = Screen::ReloadResult(reload_summary);
                                }
                                Err(error) => {
                                    state.screen = Screen::Error(format!(
                                        "Failed to get agent list: {}",
                                        error
                                    ));
                                }
                            };
                        } else {
                            state.screen = Screen::RunningReset;
                            state.spinner_frame = 0;

                            let handler: JoinHandle<Result<ResetSummary, String>> =
                                tokio::spawn(async {
                                    let herdr_path_result = env::var("HERDR_BIN_PATH");
                                    let herdr_path = match herdr_path_result {
                                        Ok(path) => path,
                                        Err(_error) => String::from("herdr"),
                                    };

                                    let agents = match get_agent_list(&herdr_path).await {
                                        Ok(value) => value,
                                        Err(error) => {
                                            return Err(error);
                                        }
                                    };

                                    reset_all_pi(&herdr_path, &agents).await
                                });

                            state.reset_task = Some(handler);
                        }
                    }
                    _ => {}
                },
                Screen::ReloadResult(_) | Screen::Error(_) | Screen::ResetResult(_) => {
                    if key.code == Enter {
                        break Ok(());
                    }
                }
                Screen::RunningReload | Screen::RunningReset => {}
            }
        }
    }
}

fn render(frame: &mut Frame, state: &AppState) {
    let [main_area, footer_area] = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(vec![Constraint::Min(0), Constraint::Length(1)])
        .areas(frame.area());

    match &state.screen {
        Screen::Menu => render_menu(frame, main_area, footer_area, state.selected),
        Screen::RunningReload => render_running_reload(frame, main_area, footer_area),
        Screen::ReloadResult(result) => render_reload_result(frame, main_area, footer_area, result),
        Screen::Error(error) => render_error(frame, main_area, footer_area, error),
        Screen::RunningReset => {
            render_running_reset(frame, main_area, footer_area, &state.spinner_frame)
        }
        Screen::ResetResult(result) => render_reset_result(frame, main_area, footer_area, result),
    }
}
