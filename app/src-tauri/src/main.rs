#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(exit_code) = kosh_lib::run_recovery_cli_if_requested() {
        std::process::exit(exit_code);
    }
    kosh_lib::run()
}
