#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if vex_launcher_lib::run_server_supervisor_from_args() {
        return;
    }
    vex_launcher_lib::run();
}
