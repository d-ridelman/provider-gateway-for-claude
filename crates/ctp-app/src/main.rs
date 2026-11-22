#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Claude запускает этот же exe как локальный MCP-сервер поиска: без окна и без Tauri.
    let args: Vec<String> = std::env::args().collect();
    if ctp_app::mcp_search::wanted(&args) {
        std::process::exit(ctp_app::mcp_search::run(
            ctp_app::mcp_search::port_from_args(&args),
        ));
    }
    ctp_app::run();
}
