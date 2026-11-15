// Release builds are GUI apps on Windows: no console window behind them.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    upscale4k_lib::run()
}
