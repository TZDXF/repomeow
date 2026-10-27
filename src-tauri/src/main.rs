// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // CLI 模式:命中内置子命令时执行并退出;terminal 通过内部参数与桌面进程握手。
    if repomeow_lib::cli::is_cli_mode() {
        std::process::exit(repomeow_lib::cli::run_cli_blocking());
    }
    repomeow_lib::run()
}
