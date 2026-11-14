use scriptura::{app, connect, maintenance};
use std::process::ExitCode;

fn print(value: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // Claude Code の Stop hook から呼ばれる。stdin の JSON を読み、結果を stdout の JSON で返す。
        Some("hook") => {
            maintenance::run();
            ExitCode::SUCCESS
        }
        Some("install") => match app::install() {
            Ok(v) => {
                print(&v);
                ExitCode::SUCCESS
            }
            Err(e) => fail(&e),
        },
        Some("connect") => match connect::run(&args[1..]) {
            Ok(v) => {
                print(&v);
                ExitCode::SUCCESS
            }
            Err(e) => fail(&e),
        },
        Some("status") => {
            print(&app::status());
            ExitCode::SUCCESS
        }
        Some("--version") | Some("-V") => {
            println!("scriptura {}", app::VERSION);
            ExitCode::SUCCESS
        }
        _ => {
            println!(
                "Scriptura {}\n\nUsage: scriptura <command>\n  hook                              Run the Claude Code Stop hook (reads JSON on stdin)\n  install                           Install this executable per user\n  connect claude-code [--apply]     Register the Stop hook (preview unless --apply)\n  status                            Show install, state and worker settings",
                app::VERSION
            );
            ExitCode::SUCCESS
        }
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::from(1)
}
