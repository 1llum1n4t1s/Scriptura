use scriptura::{app, app_update, cli, connect, maintenance};
use std::process::ExitCode;

fn print(value: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

fn main() -> ExitCode {
    // 全サブコマンド共通で、引数の解析より前に Velopack を処理する（app_update::startup）。
    // - 起動時の自動適用は切る（多プロセス型。適用はログオン時の update-apply だけが行う）。
    // - Setup の install / updated / uninstall などのフック引数は、ここで処理して必ず終了コード 0 で終わる。
    // - 管理外のビルド（開発・プラグイン同梱・単独 install）では何もせずに戻る。
    let startup = app_update::startup();
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
        // Claude Code の hook（git push 直後の PostToolUse とセッション開始時の SessionStart。旧登録の Stop も受け付ける）から呼ばれる。stdin の JSON を読み、結果を stdout の JSON で返す。
        Some("hook") => {
            maintenance::run();
            // Setup 版だけ、本処理の後に条件を満たせば更新を確認・取得する（適用はしない。stdout へは何も書かない）。
            app_update::after_hook();
            ExitCode::SUCCESS
        }
        // ログオン時に起動する（HKCU Run・LaunchAgent・XDG autostart）。保留中の更新を、導入先で他の
        // Scriptura が動いていないときだけ、署名者を検証してから再起動なしで適用する。結果は JSON 1 行。
        // 適用を依頼しなかったときは、設定（Setup 版の `ui --autostart on`）に従って管理画面の常駐を起動する。
        Some("update-apply") => match app_update::update_apply() {
            Ok(outcome) => {
                if !matches!(outcome, app_update::ApplyOutcome::Applied(_)) {
                    app_update::start_resident_after_logon_apply();
                }
                println!("{}", outcome.to_json());
                ExitCode::from(outcome.exit_code())
            }
            Err(e) => {
                app_update::start_resident_after_logon_apply();
                println!("{}", serde_json::json!({"status":"error","error":e}));
                ExitCode::from(1)
            }
        },
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
        // 管理画面。既定では常駐せず、Ctrl+C・画面の「終了」・60 分無操作で終わる。`--tray` を付けると
        // タスクトレイ（Windows）・メニューバー（macOS）に常駐する。
        // cli::run は「処理してから出力をまとめて返す」形なので、待ち続けるサーバーはここで直接起動する。
        Some("ui") => ui_command(&args[1..]),
        Some("--version") | Some("-V") => {
            println!("scriptura {}", app::VERSION);
            ExitCode::SUCCESS
        }
        // 引数なしの起動（Setup 直後の初回起動・スタートメニュー・AppImage の直接起動）。Setup 版なら登録を
        // 同期で確かめて案内を出し、対話の起動なら管理画面を常駐させて開く。管理外なら従来の使い方を表示する。
        None if app_update::welcome(startup.first_run) => ExitCode::SUCCESS,
        _ => {
            println!("{}", scriptura::t!("cli.help", version = app::VERSION));
            ExitCode::SUCCESS
        }
    }
}

/// `scriptura ui`。feature "ui" を外したビルドでは、管理画面が含まれないことを伝えて終了 1。
#[cfg(feature = "ui")]
fn ui_command(args: &[String]) -> ExitCode {
    ExitCode::from(scriptura::ui::run(args).clamp(0, 255) as u8)
}

#[cfg(not(feature = "ui"))]
fn ui_command(_args: &[String]) -> ExitCode {
    fail(scriptura::t!("cli.ui.not_included"))
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::from(1)
}
