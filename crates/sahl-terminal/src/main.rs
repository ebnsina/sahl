// Windows: no console window behind the till UI in a release build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    sahl_terminal_lib::run();
}
