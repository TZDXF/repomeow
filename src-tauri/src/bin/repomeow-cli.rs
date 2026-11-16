// Intentionally a console-subsystem executable, including release builds.
fn main() {
    std::process::exit(repomeow_lib::cli::run_cli_blocking());
}
