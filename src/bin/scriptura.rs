use scriptura::{app, cli, connect, maintenance};
use std::process::ExitCode;

fn print(value: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str);
    // 利用者向けコマンド（enable / status / doctor など）は cli が出力と終了コードを決める。
    if let Some(name) = command.filter(|c| cli::is_command(c)) {
        let reply = cli::run(name, &args[1..]);
        if !reply.stdout.is_empty() {
            println!("{}", reply.stdout);
        }
        if !reply.stderr.is_empty() {
            eprintln!("{}", reply.stderr);
        }
        return ExitCode::from(reply.code as u8);
    }
    match command {
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
            Err(e) if args.iter().any(|a| a == "--json") => {
                print(&serde_json::json!({"ok": false, "error": e}));
                ExitCode::from(1)
            }
            Err(e) => fail(&e),
        },
        Some("--version") | Some("-V") => {
            println!("scriptura {}", app::VERSION);
            ExitCode::SUCCESS
        }
        _ => {
            println!(
                "Scriptura {}\n\n\
使い方: scriptura <command> [--json]\n\
\n\
設定・操作:\n\
  enable --repo <path> --accept-consent <版> [--mode propose|push|off] [--branches a,b] [--documents AGENTS.md,README.md]\n\
                                    リポジトリで文書保守を有効にする（既定は propose）\n\
  disable --repo <path>             登録を削除する\n\
  pause [--repo <path>]             一時停止する\n\
  resume [--repo <path>]            再開する\n\
  status                            設定・登録・更新案の件数を表示する\n\
  doctor [--verify]                 環境と設定を診断する\n\
  run-now [--repo <path>]           いま 1 回保守を実行する\n\
\n\
更新案（propose モード）:\n\
  review [sha|latest] [--repo <path>]   更新案を確認する\n\
  apply [sha|latest] [--repo <path>]    更新案を作業ツリーへ適用する（stage はしない）\n\
  discard [sha|latest] [--repo <path>]  更新案を破棄する\n\
\n\
導入・連携:\n\
  hook                              Claude Code の Stop hook として動く（stdin の JSON を読む）\n\
  install                           この実行ファイルを利用者ごとの導入先へ置く\n\
  connect claude-code [--apply]     settings.json へ Stop hook を登録する（--apply まではプレビュー）\n\
  connect claude-code --remove [--apply]  Scriptura の Stop hook だけを取り除く\n\
\n\
--json を付けると機械可読の JSON を 1 つ出力します。",
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
