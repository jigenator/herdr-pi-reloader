mod guard;
mod herdr;
mod pi;
mod progress;
mod render;
mod tui;

use std::env;

use crate::herdr::get_agent_list;
use crate::pi::{reload_all_pi, reset_all_pi};
use crate::tui::run;

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.is_empty() {
        println!("Usage: herdr-pi-reloader reload / reset / tui / status");
        return;
    }

    let command = &args[0];

    if command == "status" {
        let status = progress::status_line();
        if !status.is_empty() {
            println!("{status} · ");
        }
        return;
    }

    if command != "reload" && command != "reset" && command != "tui" {
        println!("Error: Unknown command");
        return;
    }

    if command == "tui" {
        let tui_result = run().await;

        match tui_result {
            Ok(_) => (),
            Err(error) => {
                eprintln!("Error: {}", error);
            }
        }

        return;
    }

    let herdr_path_result = env::var("HERDR_BIN_PATH");

    let herdr_path = match herdr_path_result {
        Ok(path) => path,
        Err(_error) => String::from("herdr"),
    };

    let agents = match get_agent_list(&herdr_path).await {
        Ok(value) => value,
        Err(error) => {
            println!("{}", error);
            return;
        }
    };

    if command == "reload" {
        let reload_summary = reload_all_pi(&herdr_path, &agents).await;
        println!("{:#?}", reload_summary);
        return;
    }

    if command == "reset" {
        match reset_all_pi(&herdr_path, &agents).await {
            Ok(summary) => println!("{summary:#?}"),
            Err(error) => {
                eprintln!("Reset not started: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
}
