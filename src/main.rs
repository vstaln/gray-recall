//! `gray-recall`, sidecar and CLI in one binary:
//!
//! - `gray-recall manifest`: the manifest on stdout (the install probe).
//! - `gray-recall <args>`: the `gray recall` CLI (the host forwards here).
//! - No arguments, stdin not a terminal: the wire v1.1 NDJSON loop.

use gray_recall::cli;
use serde_json::{Value, json};
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;

/// One request's result, or `None` for methods this plugin does not claim.
fn handle(home: &Path, method: &str, params: &Value) -> Option<Value> {
    Some(match method {
        "plugin/manifest" => cli::manifest(),
        "tool/call" => match params["name"].as_str().unwrap_or("") {
            cli::TOOL_NAME => cli::tool_call(home, &params["args"], &cli::wire_context(params)),
            other => {
                json!({"content": format!("recall: unknown tool '{other}'"), "is_error": true})
            }
        },
        "command/run" => {
            let argv: Vec<String> = params["argv"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            json!({"text": cli::run_argv(home, &argv, &cli::wire_context(params)).text})
        }
        _ => return None,
    })
}

fn sidecar_loop(home: &Path) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = v["method"].as_str().unwrap_or("");
        if method == "plugin/shutdown" {
            break;
        }
        // Notifications carry no id and get no reply.
        let id = &v["id"];
        if id.is_null() {
            continue;
        }
        let Some(result) = handle(home, method, &v["params"]) else {
            continue;
        };
        let mut out = stdout.lock();
        writeln!(out, "{}", json!({"id": id, "result": result}))?;
        out.flush()?;
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["manifest"] {
        println!("{}", cli::manifest());
        return;
    }
    let home = match gray_recall::gray_home() {
        Ok(home) => home,
        Err(e) => {
            eprintln!("recall: {e:#}");
            std::process::exit(1);
        }
    };
    if args.is_empty() && !std::io::stdin().is_terminal() {
        if let Err(e) = sidecar_loop(&home) {
            eprintln!("recall: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    let args = if args.is_empty() {
        vec!["--help".to_string()]
    } else {
        args
    };
    let out = cli::run_argv(&home, &args, &cli::cli_context());
    if out.code == 0 {
        println!("{}", out.text);
    } else {
        eprintln!("{}", out.text);
    }
    std::process::exit(out.code);
}
