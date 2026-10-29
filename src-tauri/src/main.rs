// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "windows")]
    if eco_paste_lib::run_clipboard_capture_worker_if_requested() {
        return;
    }
    eco_paste_lib::run()
}
