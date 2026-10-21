// Evita abrir um console extra no Windows em release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().any(|a| a == "--dump") {
        porteiro_lib::dump();
        return;
    }
    porteiro_lib::run()
}
