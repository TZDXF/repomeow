// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // CLI 模式:首参数命中 git/wiki/sem/project/report 时执行命令并退出,不启动桌面窗口。
    if repomeow_lib::cli::is_cli_mode() {
        std::process::exit(repomeow_lib::cli::run_cli_blocking());
    }
    repomeow_lib::run()
}
