#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    kestrel_core::bench::init_process_start();
    kestrel_app::entry();
}
