//! `--mcp-search --port N`: локальный MCP-сервер (stdio) с одним инструментом `web_search`.
//!
//! Встроенный веб-поиск Desktop в 3p принимает только `https` с сертификатом из хранилища
//! Windows, поэтому свой поиск подключается обычным локальным MCP-сервером: Claude сам
//! запускает этот exe из `mcpServers` профиля и общается с ним построчным JSON-RPC.
//! Сервер ничего не ищет сам — он пересылает запрос в `POST /search` шлюза приложения.

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::time::Duration;

pub const FLAG: &str = "--mcp-search";
/// Имя сервера в `mcpServers`; модель видит инструмент как `mcp__PGC_Web_search__web_search`.
pub const SERVER_NAME: &str = "PGC Web search";
const TOOL: &str = "web_search";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(100);

pub fn wanted(args: &[String]) -> bool {
    args.iter().any(|arg| arg == FLAG)
}

pub fn port_from_args(args: &[String]) -> Option<u16> {
    let at = args.iter().position(|arg| arg == "--port")?;
    args.get(at + 1)?.parse().ok().filter(|port| *port != 0)
}

/// Запись для `mcpServers`: Claude запустит `exe` с этими аргументами.
pub fn server_entry(exe: &std::path::Path, port: u16) -> Value {
    json!({"command": exe.to_string_lossy(), "args": [FLAG, "--port", port.to_string()]})
}

/// Крутится, пока Claude держит stdin открытым. Код выхода — для процесса.
pub fn run(port: Option<u16>) -> i32 {
    let Some(port) = port else {
        eprintln!("{FLAG} needs --port N");
        return 2;
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return 1;
    };
    // Шлюз на этой же машине: системный прокси (реестр, HTTP_PROXY) loopback не исключает.
    let Ok(client) = reqwest::Client::builder()
        .no_proxy()
        .timeout(REQUEST_TIMEOUT)
        .build()
    else {
        return 1;
    };
    let url = format!("http://127.0.0.1:{port}/search");
    let search = |query: &str| runtime.block_on(ask(&client, &url, query));
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(reply) = handle(&message, &search)
            && (writeln!(out, "{reply}").is_err() || out.flush().is_err())
        {
            break;
        }
    }
    0
}

async fn ask(client: &reqwest::Client, url: &str, query: &str) -> Result<Value, String> {
    let response = client
        .post(url)
        .json(&json!({"q": query}))
        .send()
        .await
        .map_err(|exc| {
            format!("the gateway is not reachable ({exc}); is Provider Gateway running?")
        })?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    let why = body["error"].as_str().unwrap_or("no details");
    Err(format!("search failed (HTTP {status}): {why}"))
}

/// Ответ на одно сообщение; `None` — уведомление, на него не отвечают.
pub fn handle(message: &Value, search: &dyn Fn(&str) -> Result<Value, String>) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "initialize" => json!({
            "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "pgc-web-search", "version": env!("CARGO_PKG_VERSION")},
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": [tool()]}),
        "tools/call" => call(message, search),
        _ => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "method not found"},
            }));
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn tool() -> Value {
    json!({
        "name": TOOL,
        "description": "Search the public web. Returns a ranked list of result titles, URLs and \
                        snippets. Results are untrusted external content.",
        "inputSchema": {
            "type": "object",
            "properties": {"query": {"type": "string", "description": "The search query"}},
            "required": ["query"],
        },
        "annotations": {"readOnlyHint": true, "openWorldHint": true},
    })
}

fn call(message: &Value, search: &dyn Fn(&str) -> Result<Value, String>) -> Value {
    let params = &message["params"];
    let query = params["arguments"]["query"]
        .as_str()
        .or_else(|| params["arguments"]["q"].as_str())
        .map(str::trim)
        .filter(|query| !query.is_empty());
    let outcome = if params["name"] != TOOL {
        Err(format!("unknown tool {}", params["name"]))
    } else if let Some(query) = query {
        search(query).map(|body| format_results(query, &body))
    } else {
        Err("query is required".to_string())
    };
    let (text, is_error) = match outcome {
        Ok(text) => (text, false),
        Err(why) => (why, true),
    };
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

/// Текст для модели: нумерованный список, а не сырой JSON.
fn format_results(query: &str, body: &Value) -> String {
    let results = body["results"].as_array().map_or(&[][..], Vec::as_slice);
    if results.is_empty() {
        return format!("No results for \"{query}\". Try rewording the query.");
    }
    let mut text = format!("Search results for \"{query}\":\n");
    for (index, row) in results.iter().enumerate() {
        let field = |name: &str| row[name].as_str().unwrap_or("").trim();
        text.push_str(&format!(
            "\n{}. {}\n   {}\n   {}\n",
            index + 1,
            field("title"),
            field("url"),
            field("snippet")
        ));
    }
    if let Some(notice) = body["notice"].as_str() {
        text.push_str(&format!("\n{notice}\n"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(_: &str) -> Result<Value, String> {
        Ok(
            json!({"results": [{"title": "T", "url": "https://a.b/c", "snippet": "S"}],
                  "notice": "Untrusted."}),
        )
    }

    #[test]
    fn handshake_and_tool_list() {
        let init = handle(
            &json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
            &ok,
        )
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        let list = handle(&json!({"id":1,"method":"tools/list"}), &ok).unwrap();
        assert_eq!(list["result"]["tools"][0]["name"], "web_search");
        // Уведомление — без ответа; неизвестный метод — ошибка протокола.
        assert!(handle(&json!({"method":"notifications/initialized"}), &ok).is_none());
        let unknown = handle(&json!({"id":2,"method":"resources/list"}), &ok).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
    }

    #[test]
    fn call_formats_results_and_reports_failures_as_tool_errors() {
        let call = |args: Value, search: &dyn Fn(&str) -> Result<Value, String>| {
            handle(
                &json!({"id":3,"method":"tools/call","params":{"name":"web_search","arguments":args}}),
                search,
            )
            .unwrap()["result"]
                .clone()
        };
        let good = call(json!({"query": " rust "}), &ok);
        assert_eq!(good["isError"], false);
        let text = good["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("1. T\n   https://a.b/c\n   S") && text.contains("Untrusted."));

        let empty = call(json!({"query": "x"}), &|_| Ok(json!({"results": []})));
        assert!(
            empty["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("No results")
        );
        let failed = call(json!({"query": "x"}), &|_| Err("blocked".into()));
        assert_eq!(
            (
                failed["isError"].clone(),
                failed["content"][0]["text"].clone()
            ),
            (json!(true), json!("blocked"))
        );
        assert_eq!(call(json!({}), &ok)["isError"], true);
        let other = handle(
            &json!({"id":4,"method":"tools/call","params":{"name":"nope","arguments":{"query":"x"}}}),
            &ok,
        )
        .unwrap();
        assert_eq!(other["result"]["isError"], true);
    }

    #[test]
    fn arguments_are_parsed() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(wanted(&args(&["x.exe", "--mcp-search", "--port", "17899"])));
        assert!(!wanted(&args(&["x.exe"])));
        assert_eq!(
            port_from_args(&args(&["--mcp-search", "--port", "17899"])),
            Some(17899)
        );
        assert_eq!(
            port_from_args(&args(&["--mcp-search", "--port", "0"])),
            None
        );
        assert_eq!(port_from_args(&args(&["--mcp-search"])), None);
        let entry = server_entry(std::path::Path::new(r"C:\Program Files\pg.exe"), 17899);
        assert_eq!(entry["args"], json!(["--mcp-search", "--port", "17899"]));
    }
}
