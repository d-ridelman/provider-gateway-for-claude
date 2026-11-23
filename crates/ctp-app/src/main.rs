#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Повышенный экземпляр для TUN (UAC) только присматривает за ядром mihomo: до окна, иначе
    // single-instance передал бы его запуск в уже открытое окно.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--mihomo-supervisor") {
        std::process::exit(ctp_app::mihomo::supervise(&args[2..]));
    }
    ctp_app::run();
}
