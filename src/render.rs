use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::pi::{ReloadSummary, ResetSummary};

pub(crate) fn render_menu(frame: &mut Frame, main_area: Rect, footer_area: Rect, selected: usize) {
    let menu_text_options = ["Reload all Pi", "Reset all Pi"];

    let menu_text = menu_text_options
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if i == selected {
                Span::styled(
                    format!("> {}", line),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
                .into()
            } else {
                Span::styled(format!("  {}", line), Style::default()).into()
            }
        })
        .collect::<Vec<Line<'_>>>();

    let main = Paragraph::new(menu_text).block(Block::bordered().title("Herdr Pi Reloader"));
    let footer = Paragraph::new(if selected == 1 {
        "Reset moves focus. Don't type until the summary."
    } else {
        "↑/k ↓/j select • Enter run • q/Esc quit"
    });

    frame.render_widget(main, main_area);
    frame.render_widget(footer, footer_area);
}

pub(crate) fn render_running_reload(frame: &mut Frame, main_area: Rect, footer_area: Rect) {
    let main = Paragraph::new("Reloading Pi instances...")
        .block(Block::bordered().title("Herdr Pi Reloader"));
    let footer = Paragraph::new("Operation in progress...");

    frame.render_widget(main, main_area);
    frame.render_widget(footer, footer_area);
}

pub(crate) fn render_reload_result(
    frame: &mut Frame,
    main_area: Rect,
    footer_area: Rect,
    result: &ReloadSummary,
) {
    let failed = result.failed + result.skipped_invalid_agent_data;

    let (heading_text, heading_color) = if failed > 0 {
        ("Completed with errors", Color::Red)
    } else if result.reloaded > 0 {
        ("Success", Color::Green)
    } else {
        ("Nothing to do", Color::Yellow)
    };

    let heading_line = Line::from(Span::styled(
        heading_text,
        Style::default()
            .fg(heading_color)
            .add_modifier(Modifier::BOLD),
    ));

    let mut lines = vec![
        heading_line,
        Line::from(""),
        Line::from(format!("Reloaded: {}", result.reloaded)),
        Line::from(format!("Skipped (busy): {}", result.skipped_unsafe_status)),
        Line::from(format!("Skipped (not Pi): {}", result.skipped_non_pi)),
        Line::from(format!("Failed: {}", failed)),
    ];

    if !result.errors.is_empty() {
        let max_errors = 5;
        let total_errors = result.errors.len();
        let hidden_errors = total_errors.saturating_sub(max_errors);

        lines.push(Line::from(""));
        lines.push(Line::from("Errors:"));
        lines.push(Line::from(""));

        let error_lines = result
            .errors
            .iter()
            .take(max_errors)
            .enumerate()
            .map(|(i, value)| Line::from(format!("Error {}: {}", i + 1, value)))
            .collect::<Vec<Line<'_>>>();

        lines.extend(error_lines);

        if hidden_errors > 0 {
            lines.push(Line::from(""));
            lines.push(Line::from(format!("...and {} more errors", hidden_errors)));
        }
    }

    let main = Paragraph::new(lines).block(Block::bordered().title("Herdr Pi Reloader"));
    let footer = Paragraph::new("Enter/q/Esc quit");

    frame.render_widget(main, main_area);
    frame.render_widget(footer, footer_area);
}

pub(crate) fn render_error(frame: &mut Frame, main_area: Rect, footer_area: Rect, error: &str) {
    let main = Paragraph::new(error).block(Block::bordered().title("Herdr Pi Reloader"));
    let footer = Paragraph::new("Enter/q/Esc quit");

    frame.render_widget(main, main_area);
    frame.render_widget(footer, footer_area);
}

pub(crate) fn render_running_reset(
    frame: &mut Frame,
    main_area: Rect,
    footer_area: Rect,
    spinner_frame: &usize,
) {
    let frames = ["-", "\\", "|", "/"];

    let current_frame = frames[spinner_frame % frames.len()];

    let status = crate::progress::status_line();
    let main = Paragraph::new(format!("{current_frame} Resetting Pi instances...\n\n{status}\n\nDo not type or switch panes.\nInput is NOT blocked outside this tab."))
        .block(Block::bordered().title("Herdr Pi Reloader"));
    let footer = Paragraph::new("Wait for the summary before using Herdr.");

    frame.render_widget(main, main_area);
    frame.render_widget(footer, footer_area);
}

pub(crate) fn render_reset_result(
    frame: &mut Frame,
    main_area: Rect,
    footer_area: Rect,
    result: &ResetSummary,
) {
    let failed = result.failed;

    let (heading_text, heading_color) = if failed > 0 {
        ("Completed with errors", Color::Red)
    } else if result.reset > 0 {
        ("Success", Color::Green)
    } else {
        ("Nothing to do", Color::Yellow)
    };

    let heading_line = Line::from(Span::styled(
        heading_text,
        Style::default()
            .fg(heading_color)
            .add_modifier(Modifier::BOLD),
    ));

    let mut lines = vec![
        heading_line,
        Line::from(""),
        Line::from(format!("Reset: {}", result.reset)),
        Line::from(format!("Visited: {}", result.visited)),
        Line::from(format!("Skipped (busy): {}", result.skipped_unsafe_status)),
        Line::from(format!("Skipped (not Pi): {}", result.skipped_non_pi)),
        Line::from(format!("Failed: {}", failed)),
    ];
    if result.errors.is_empty() {
        lines.push(Line::from(""));
        lines.extend(
            result
                .outcomes
                .iter()
                .take(5)
                .map(|text| Line::from(text.as_str())),
        );
        if result.outcomes.len() > 5 {
            lines.push(Line::from(format!(
                "...and {} more panes",
                result.outcomes.len() - 5
            )));
        }
    }

    if !result.errors.is_empty() {
        let max_errors = 5;
        let total_errors = result.errors.len();
        let hidden_errors = total_errors.saturating_sub(max_errors);

        lines.push(Line::from(""));
        lines.push(Line::from("Errors:"));
        lines.push(Line::from(""));

        let error_lines = result
            .errors
            .iter()
            .take(max_errors)
            .enumerate()
            .map(|(i, value)| Line::from(format!("Error {}: {}", i + 1, value)))
            .collect::<Vec<Line<'_>>>();

        lines.extend(error_lines);

        if hidden_errors > 0 {
            lines.push(Line::from(""));
            lines.push(Line::from(format!("...and {} more errors", hidden_errors)));
        }
    }

    let main = Paragraph::new(lines).block(Block::bordered().title("Herdr Pi Reloader"));
    let footer = Paragraph::new("Enter/q/Esc quit");

    frame.render_widget(main, main_area);
    frame.render_widget(footer, footer_area);
}
