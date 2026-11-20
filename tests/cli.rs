//! 利用者向け CLI（src/cli.rs・src/doctor.rs・connect の --remove）の契約テスト。
//!
//! 隔離: すべてのコマンドは子プロセスとして実行し、環境変数 `SCRIPTURA_HOME` / `SCRIPTURA_CONFIG` /
//! `SCRIPTURA_DB` / `CLAUDE_CONFIG_DIR` / `LOCALAPPDATA` を一時フォルダーへ向ける。実環境の設定・状態・
//! `~/.claude` は読み書きしない。`claude` / `gh` は既存のスタブ（本物の Claude は呼ばない）。
//! 環境変数は各子プロセスにだけ渡すので、テスト同士は干渉しない（プロセス全体の環境は変えない）。
//! fixture の置き場は `SCRIPTURA_TEST_BASE`（未指定なら既存テストと同じ `validation/`）。
#![cfg(feature = "test-support")]
use scriptura::config::{self, Config, Loaded, MigratedFrom};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);
const EXE: &str = env!("CARGO_BIN_EXE_scriptura");

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "JSON ではありません（{e}）:\n{}\n{}",
                self.stdout, self.stderr
            )
        })
    }
}

struct Fx {
    base: PathBuf,
    repo: PathBuf,
}

fn same_path(a: impl AsRef<Path>, b: impl AsRef<Path>) -> bool {
    fs::canonicalize(a).unwrap() == fs::canonicalize(b).unwrap()
}

impl Fx {
    /// 空の一時環境と、push 済みの Git リポジトリ（main が upstream を持つ）を作る。
    fn new() -> Self {
        let parent = env::var_os("SCRIPTURA_TEST_BASE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("validation"));
        let base = parent.join(format!(
            "cli-{}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for dir in ["bin", "home", "claude", "localappdata", "state"] {
            fs::create_dir_all(base.join(dir)).unwrap();
        }
        let stub = if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        };
        fs::copy(
            env!("CARGO_BIN_EXE_hook-test-stub"),
            base.join("bin").join(stub),
        )
        .unwrap();
        let repo = base.join("repo");
        let fx = Self { base, repo };
        let bare = fx.base.join("remote.git");
        fx.make_repo(&fx.repo);
        git_global(&["init", "--bare", "-b", "main", bare.to_str().unwrap()]);
        fx.git(&["remote", "add", "origin", bare.to_str().unwrap()]);
        fx.git(&["push", "-u", "origin", "main"]);
        fx
    }

    /// remote の無い Git リポジトリ（1 コミット）を作る。
    fn make_repo(&self, path: &Path) {
        fs::create_dir_all(path).unwrap();
        let g = |args: &[&str]| git_in(path, args);
        g(&["init", "-b", "main"]);
        g(&["config", "user.email", "test@example.invalid"]);
        g(&["config", "user.name", "CLI fixture"]);
        g(&["config", "core.autocrlf", "false"]);
        fs::write(path.join("source.txt"), "initial").unwrap();
        fs::write(path.join("README.md"), "readme").unwrap();
        g(&["add", "."]);
        g(&["commit", "-m", "initial"]);
    }

    /// 専用の bare remote を持ち、main が upstream を持つ Git リポジトリ（enable できる状態）を作る。
    fn make_pushed_repo(&self, path: &Path) {
        self.make_repo(path);
        let bare = path.with_extension("remote.git");
        git_global(&["init", "--bare", "-b", "main", bare.to_str().unwrap()]);
        git_in(path, &["remote", "add", "origin", bare.to_str().unwrap()]);
        git_in(path, &["push", "-u", "origin", "main"]);
    }

    fn git(&self, args: &[&str]) -> String {
        git_in(&self.repo, args)
    }

    fn config_path(&self) -> PathBuf {
        self.base.join("config.toml")
    }

    fn settings_path(&self) -> PathBuf {
        self.base.join("claude").join("settings.json")
    }

    fn state_db(&self) -> PathBuf {
        self.base.join("state").join("state.sqlite3")
    }

    /// 子プロセスとして scriptura を実行する。
    fn exec(
        &self,
        args: &[&str],
        extra: &[(&str, &str)],
        stdin: Option<&str>,
        cwd: Option<&Path>,
    ) -> Out {
        self.exec_program(Path::new(EXE), args, extra, stdin, cwd)
    }

    /// 任意の実行ファイルを、exec と同じ隔離環境（PATH・LOCALAPPDATA・設定・状態）で実行する。
    fn exec_program(
        &self,
        program: &Path,
        args: &[&str],
        extra: &[(&str, &str)],
        stdin: Option<&str>,
        cwd: Option<&Path>,
    ) -> Out {
        let mut c = Command::new(program);
        c.args(args)
            .current_dir(cwd.unwrap_or(&self.base))
            .env("SCRIPTURA_HOME", self.base.join("home"))
            .env("SCRIPTURA_CONFIG", self.config_path())
            .env("SCRIPTURA_DB", self.state_db())
            .env("CLAUDE_CONFIG_DIR", self.base.join("claude"))
            .env("LOCALAPPDATA", self.base.join("localappdata"))
            .env("HOOK_STUB_PROMPT", self.base.join("prompt.json"))
            .env(
                "PATH",
                env::join_paths(
                    std::iter::once(self.base.join("bin"))
                        .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
                )
                .unwrap(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // 実行環境（Claude セッション内など）の値がテストへ混ざらないよう、関係する変数を外す。
        for key in [
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_SIMPLE",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "HOOK_STUB_WRITES",
            "HOOK_STUB_EXIT",
            "HOOK_STUB_AUTH_JSON",
            "HOOK_STUB_AUTH_EXIT",
            "HOOK_STUB_AUTH_STDERR",
            "HOOK_STUB_AUTH_LOG",
        ] {
            c.env_remove(key);
        }
        for (k, v) in extra {
            c.env(k, v);
        }
        let mut child = c.spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        if let Some(text) = stdin {
            input.write_all(text.as_bytes()).unwrap();
        }
        drop(input);
        let out = child.wait_with_output().unwrap();
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn run(&self, args: &[&str]) -> Out {
        self.exec(args, &[], None, None)
    }

    /// 既定のリポジトリで enable を実行する（JSON 出力）。
    fn enable(&self, extra: &[&str]) -> Out {
        self.enable_repo(&self.repo, extra)
    }

    fn enable_repo(&self, repo: &Path, extra: &[&str]) -> Out {
        let mut args = vec!["enable", "--repo", repo.to_str().unwrap(), "--json"];
        args.extend_from_slice(extra);
        self.run(&args)
    }

    fn config(&self) -> Config {
        match config::load_from(&self.config_path()) {
            Loaded::Ok(c) => c,
            other => panic!("設定を読めません: {other:?}"),
        }
    }

    /// source.txt を変更して commit・push する。
    fn push_change(&self, body: &str) {
        fs::write(self.repo.join("source.txt"), body).unwrap();
        self.git(&["add", "source.txt"]);
        self.git(&["commit", "-m", body]);
        self.git(&["push"]);
    }

    fn hook(&self, extra: &[(&str, &str)]) -> Out {
        let payload = json!({"hook_event_name": "Stop", "cwd": self.repo}).to_string();
        self.exec(&["hook"], extra, Some(&payload), None)
    }

    fn write_settings(&self, value: &Value) {
        fs::write(
            self.settings_path(),
            serde_json::to_vec_pretty(value).unwrap(),
        )
        .unwrap();
    }
}

fn git_global(args: &[&str]) {
    let r = Command::new("git").args(args).output().unwrap();
    assert!(
        r.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&r.stderr)
    );
}

fn git_in(path: &Path, args: &[&str]) -> String {
    let r = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    String::from_utf8(r.stdout).unwrap()
}

fn item<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == id)
        .unwrap_or_else(|| panic!("診断項目 {id} がありません: {report}"))
}

// ---------------------------------------------------------------------------
// enable
// ---------------------------------------------------------------------------

#[test]
fn removed_enable_options_and_commands_are_refused_with_a_dedicated_error() {
    let fx = Fx::new();
    let repo = fx.repo.to_str().unwrap();
    // 廃止した --mode と --accept-consent は、値の有無に関わらず専用のエラーで拒否し、何も書かない。
    for extra in [
        vec!["--mode", "push"],
        vec!["--mode=propose"],
        vec!["--accept-consent", "1"],
        vec!["--accept-consent=1"],
    ] {
        let mut args = vec!["enable", "--repo", repo, "--json"];
        args.extend(extra.iter());
        let out = fx.run(&args);
        assert_eq!(out.code, 1, "{extra:?}");
        let message = out.json()["error"].as_str().unwrap().to_string();
        assert!(
            message.contains("廃止")
                && message.contains(extra[0].trim_start_matches("--").split('=').next().unwrap()),
            "{message}"
        );
        assert!(!message.contains("不明なオプション"), "{message}");
    }
    assert!(!fx.config_path().exists());
    assert!(!fx.state_db().exists());
    // --json なしは標準エラーへ出す。
    let out = fx.run(&["enable", "--repo", repo, "--mode", "push"]);
    assert_eq!(out.code, 1);
    assert!(out.stdout.is_empty() && out.stderr.contains("廃止"));
    // review / apply / discard は専用のエラー（引数があっても同じ）。残ったパッチは git apply へ案内する。
    for command in ["review", "apply", "discard"] {
        for extra in [vec![], vec!["--repo", repo], vec!["latest", "x"]] {
            let mut args = vec![command, "--json"];
            args.extend(extra.iter());
            let out = fx.run(&args);
            assert_eq!(out.code, 1, "{command} {extra:?}");
            let message = out.json()["error"].as_str().unwrap().to_string();
            assert!(
                message.contains("廃止")
                    && message.contains("git apply")
                    && message.contains(command),
                "{message}"
            );
        }
    }
    assert!(!fx.config_path().exists());
    // 通常の未知のオプションは従来どおりのエラー。
    let out = fx.run(&["enable", "--repo", repo, "--bogus", "x", "--json"]);
    assert!(
        out.json()["error"]
            .as_str()
            .unwrap()
            .contains("不明なオプション")
    );
}

#[test]
fn enable_registers_push_on_the_current_branch_and_creates_a_safe_config() {
    let fx = Fx::new();
    let out = fx.enable(&[]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let v = out.json();
    assert_eq!(v["ok"], true);
    // mode・同意は出力に無い。
    assert!(
        v.get("mode").is_none() && v.get("consentVersion").is_none(),
        "{v}"
    );
    assert_eq!(v["paused"], false);
    assert_eq!(v["migratedFrom"], Value::Null);
    // 既定のブランチは現在のブランチだけ。
    assert_eq!(v["branches"], json!(["main"]));
    assert_eq!(
        v["documents"],
        json!(["AGENTS.md", "DESIGN.md", "README.md"])
    );
    assert_eq!(v["baseline"], true);
    assert!(same_path(v["root"].as_str().unwrap(), &fx.repo));
    assert!(same_path(
        v["configPath"].as_str().unwrap(),
        fx.config_path()
    ));
    let config = fx.config();
    assert_eq!(config.enrollment.policy, config::Policy::Explicit);
    assert_eq!(config.schema, 3);
    assert!(
        fs::read_to_string(fx.config_path())
            .unwrap()
            .contains("schema = 3")
    );
    assert_eq!(config.repository.len(), 1);
    assert_eq!(config.repository[0].branches, vec!["main".to_string()]);
    // 人が読む形式にも mode は出ない。
    let human = fx.run(&["enable", "--repo", fx.repo.to_str().unwrap()]);
    assert_eq!(human.code, 0, "{}", human.stderr);
    assert!(
        human.stdout.contains("ブランチ") && !human.stdout.contains("モード"),
        "{}",
        human.stdout
    );
    // 明示したブランチはそのまま使う。
    let v = fx.enable(&["--branches", "main, release/1"]).json();
    assert_eq!(v["branches"], json!(["main", "release/1"]));
    assert_eq!(fx.enable(&["--branches", ","]).code, 1);
    // サブフォルダーを指定しても Git ルートへ正規化される。
    fs::create_dir_all(fx.repo.join("sub")).unwrap();
    let out = fx.enable_repo(&fx.repo.join("sub"), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(fx.config().repository.len(), 1);
}

#[test]
fn enable_keeps_other_registrations_and_settings_and_leaves_no_temp_files() {
    let fx = Fx::new();
    let other = fx.base.join("other-repo");
    let mut seed = Config::default();
    seed.worker.timeout_secs = 300;
    seed.defaults.exclude = vec!["private/**".into()];
    seed.repository.push(config::Repository {
        root: other.to_string_lossy().into_owned(),
        remote: None,
        branches: vec!["release".into()],
        documents: None,
        exclude: vec![],
        paused: true,
        migrated_from: None,
    });
    config::save_to(&fx.config_path(), &seed).unwrap();
    let out = fx.enable(&["--documents", "README.md,agents.md"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let config = fx.config();
    assert_eq!(config.worker.timeout_secs, 300);
    assert_eq!(config.defaults.exclude, vec!["private/**".to_string()]);
    assert_eq!(config.repository.len(), 2);
    assert_eq!(config.repository[0].root, other.to_string_lossy());
    assert!(config.repository[0].paused);
    assert_eq!(config.repository[0].branches, vec!["release".to_string()]);
    assert_eq!(
        config.repository[1].documents,
        Some(vec![config::Document::Readme, config::Document::Agents])
    );
    // 再実行は同じ登録を更新する（重複しない）。一時ファイルも残らない。
    assert_eq!(fx.enable(&["--branches", "release"]).code, 0);
    let config = fx.config();
    assert_eq!(config.repository.len(), 2);
    assert_eq!(config.repository[1].branches, vec!["release".to_string()]);
    let leftovers: Vec<_> = fs::read_dir(&fx.base)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    // 未対応の文書名・設定に定義の無いカスタム種類は拒否する（設定は書き換えない）。
    let before = fs::read_to_string(&fx.config_path()).unwrap();
    for name in ["FOO.md", "architecture"] {
        let out = fx.enable(&["--documents", name]);
        assert_eq!(out.code, 1, "{name}");
        assert!(
            out.json()["error"].as_str().unwrap().contains(name),
            "{name}"
        );
    }
    assert_eq!(fs::read_to_string(&fx.config_path()).unwrap(), before);
    // CHANGELOG.md は組み込みの種類として受け付ける（大文字小文字は区別しない）。
    let out = fx.enable(&["--documents", "readme.md,Changelog.md"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert_eq!(
        out.json()["documents"],
        json!(["README.md", "CHANGELOG.md"])
    );
    assert_eq!(
        fx.config().repository[1].documents,
        Some(vec![config::Document::Readme, config::Document::Changelog])
    );
    assert!(
        fs::read_to_string(&fx.config_path())
            .unwrap()
            .contains("schema = 3")
    );
}

#[test]
fn enable_records_history_and_restarts_a_registration_stopped_by_the_migration() {
    // 旧形式（同意なし・propose）の設定は、enable で読まれるときに変換され、止めた登録は enable が解除する。
    let fx = Fx::new();
    let repo = fx.repo.to_string_lossy().replace('\\', "/");
    let old = format!("[defaults]\nmode = \"propose\"\n[[repository]]\nroot = \"{repo}\"\n");
    fs::write(fx.config_path(), &old).unwrap();
    let out = fx.enable(&[]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert_eq!(out.json()["created"], false);
    assert_eq!(out.json()["migratedFrom"], Value::Null);
    let config = fx.config();
    assert_eq!(config.repository.len(), 1);
    assert!(!config.repository[0].paused);
    assert_eq!(config.repository[0].migrated_from, None);
    assert_eq!(config.schema, 3);
    // CLI の変更も設定ロックと変更履歴を通る（状態フォルダーに直前の内容が退避される）。
    let history = fx.base.join("state").join("config-history");
    let saved: Vec<_> = fs::read_dir(&history)
        .unwrap()
        .flatten()
        .map(|e| fs::read(e.path()).unwrap())
        .collect();
    assert_eq!(saved, vec![old.into_bytes()]);
}

#[test]
fn enable_without_remote_or_upstream_is_refused() {
    let fx = Fx::new();
    // remote が無い。
    let bare_less = fx.base.join("no-remote");
    fx.make_repo(&bare_less);
    let out = fx.enable_repo(&bare_less, &[]);
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("remote"));
    // remote はあるが現在のブランチに upstream が無い。
    let no_upstream = fx.base.join("no-upstream");
    fx.make_repo(&no_upstream);
    git_in(
        &no_upstream,
        &[
            "remote",
            "add",
            "origin",
            fx.base.join("remote.git").to_str().unwrap(),
        ],
    );
    let out = fx.enable_repo(&no_upstream, &[]);
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("upstream"));
    // --branches を明示しても upstream は必須。
    let out = fx.enable_repo(&no_upstream, &["--branches", "main"]);
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("upstream"));
    // detached HEAD も受け付けない。
    git_in(&no_upstream, &["checkout", "--detach"]);
    let out = fx.enable_repo(&no_upstream, &[]);
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("HEAD"));
    assert!(!fx.config_path().exists(), "拒否したときは設定を作らない");
}

#[test]
fn enable_records_a_normalized_remote_when_the_push_url_is_recognizable() {
    let fx = Fx::new();
    fx.git(&[
        "remote",
        "set-url",
        "--push",
        "origin",
        "git@GitHub.com:Acme/Widget.git",
    ]);
    assert_eq!(fx.enable(&[]).code, 0);
    assert_eq!(
        fx.config().repository[0].remote.as_deref(),
        Some("github.com/acme/widget")
    );
}

#[test]
fn enable_refuses_to_rewrite_a_broken_config() {
    let fx = Fx::new();
    let broken = "this = is = broken\n";
    fs::write(fx.config_path(), broken).unwrap();
    let out = fx.enable(&[]);
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("壊れている"));
    assert_eq!(fs::read_to_string(fx.config_path()).unwrap(), broken);
    // 他の書き込み系も同様。
    for cmd in ["pause", "resume"] {
        let out = fx.exec(
            &[cmd, "--repo", fx.repo.to_str().unwrap(), "--json"],
            &[],
            None,
            None,
        );
        assert_eq!(out.code, 1, "{cmd}");
    }
    assert_eq!(fs::read_to_string(fx.config_path()).unwrap(), broken);
    assert!(!fx.state_db().exists(), "壊れた設定のまま起点を記録しない");
}

#[test]
fn enable_records_a_baseline_so_earlier_pushes_are_not_processed() {
    let fx = Fx::new();
    // 有効化の前に push 済みの変更がある。
    fx.push_change("before enable");
    assert_eq!(fx.enable(&[]).code, 0);
    // 起点が状態 DB に記録されている。
    let db = rusqlite::Connection::open(fx.state_db()).unwrap();
    let rows: i64 = db
        .query_row("SELECT count(*) FROM poll_cursors", [], |r| r.get(0))
        .unwrap();
    assert!(rows >= 1);
    drop(db);
    // 以前の push は処理されない（worker が呼ばれない）。
    let prompt = fx.base.join("prompt.json");
    let out = fx.hook(&[]);
    assert_eq!(out.code, 0);
    assert!(!prompt.exists(), "有効化前の push を処理してはいけない");
    // 有効化後の新しい push は処理される。
    fx.push_change("after enable");
    let out = fx.hook(&[]);
    assert_eq!(out.code, 0);
    assert!(
        prompt.exists(),
        "有効化後の push は worker が呼ばれる: {}",
        out.stdout
    );
}

// ---------------------------------------------------------------------------
// disable / pause / resume
// ---------------------------------------------------------------------------

#[test]
fn disable_pause_and_resume_update_the_registration() {
    let fx = Fx::new();
    let repo = fx.repo.to_str().unwrap();
    // 未登録（設定なし）は拒否。
    for cmd in ["pause", "resume", "disable"] {
        let out = fx.run(&[cmd, "--repo", repo, "--json"]);
        assert_eq!(out.code, 1, "{cmd}");
        assert_eq!(out.json()["ok"], false);
    }
    assert_eq!(fx.enable(&[]).code, 0);
    let out = fx.run(&["pause", "--repo", repo, "--json"]);
    assert_eq!(out.code, 0);
    assert_eq!(out.json()["paused"], true);
    assert_eq!(out.json()["changed"], true);
    assert!(fx.config().repository[0].paused);
    assert_eq!(
        fx.run(&["pause", "--repo", repo, "--json"]).json()["changed"],
        false
    );
    // --repo 省略時は cwd のリポジトリ。
    let out = fx.exec(&["resume", "--json"], &[], None, Some(&fx.repo));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.json()["paused"], false);
    assert!(!fx.config().repository[0].paused);
    // disable は --repo が必須。
    assert_eq!(
        fx.exec(&["disable", "--json"], &[], None, Some(&fx.repo))
            .code,
        1
    );
    let out = fx.run(&["disable", "--repo", repo, "--json"]);
    assert_eq!(out.code, 0);
    assert_eq!(out.json()["removed"], true);
    assert!(fx.config().repository.is_empty());
    assert_eq!(fx.run(&["disable", "--repo", repo, "--json"]).code, 1);
    // 登録だけが消え、設定は schema 3 のまま残る。
    assert_eq!(fx.config().schema, 3);
}

#[test]
fn migrated_registrations_cannot_be_resumed_from_the_cli() {
    // 旧形式からの移行で止めた登録は resume では再開できず、enable で有効化し直す。
    let fx = Fx::new();
    let repo = fx.repo.to_str().unwrap();
    let root = fx.repo.to_string_lossy().replace('\\', "/");
    fs::write(
        fx.config_path(),
        format!("[[repository]]\nroot = \"{root}\"\n"),
    )
    .unwrap();
    // 読み取り（status）は変換した結果を示し、ファイルは書き換えない。
    let before = fs::read(fx.config_path()).unwrap();
    let v = fx.run(&["status", "--json"]).json();
    assert_eq!(v["legacy"]["fromSchema"], 1);
    assert_eq!(v["legacy"]["stopped"], 1);
    assert_eq!(v["repositories"][0]["paused"], true);
    assert_eq!(v["repositories"][0]["migratedFrom"], "unconsented");
    let human = fx.run(&["status"]).stdout;
    assert!(
        human.contains("unconsented") && human.contains("旧形式"),
        "{human}"
    );
    assert_eq!(fs::read(fx.config_path()).unwrap(), before);
    let out = fx.run(&["resume", "--repo", repo, "--json"]);
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("enable"));
    // 拒否では変換結果を保存しない。enable で解除すると新形式で保存される。
    assert_eq!(fs::read(fx.config_path()).unwrap(), before);
    assert_eq!(fx.enable(&[]).code, 0);
    let config = fx.config();
    assert_eq!(config.repository[0].migrated_from, None);
    assert!(!config.repository[0].paused);
    let v = fx.run(&["status", "--json"]).json();
    assert_eq!(v["legacy"], Value::Null);
    assert_eq!(v["repositories"][0]["migratedFrom"], Value::Null);
    let _ = MigratedFrom::Off;
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

#[test]
fn status_json_reports_config_repositories_and_worker_without_secrets() {
    let fx = Fx::new();
    let v = fx.run(&["status", "--json"]).json();
    assert_eq!(v["ok"], true);
    assert_eq!(v["config"]["state"], "missing");
    assert_eq!(v["repositories"], json!([]));
    assert!(
        v.get("consent").is_none() && v.get("proposals").is_none(),
        "{v}"
    );
    assert_eq!(v["legacy"], Value::Null);
    assert!(v["version"].is_string() && v["installed"].is_boolean());
    // 導入の形（既存の installed は単独 install の有無のまま。新しいキー installForm が実行中の形を表す）。
    // テストのビルドは Velopack 管理下でも単独導入でもないので other。
    assert_eq!(v["installForm"], "other");
    assert_eq!(v["installed"], false);
    let human = fx.run(&["status"]);
    assert!(!human.stdout.contains("未導入"), "{}", human.stdout);
    assert!(
        human.stdout.contains("プラグイン／開発ビルド"),
        "{}",
        human.stdout
    );

    assert_eq!(fx.enable(&["--branches", "main"]).code, 0);
    let v = fx.run(&["status", "--json"]).json();
    assert_eq!(v["config"]["state"], "ok");
    assert!(same_path(
        v["config"]["path"].as_str().unwrap(),
        fx.config_path()
    ));
    assert_eq!(v["policy"], "explicit");
    assert!(v["defaults"].get("mode").is_none(), "{v}");
    let repo = &v["repositories"][0];
    assert!(
        repo.get("mode").is_none() && repo.get("proposals").is_none(),
        "{repo}"
    );
    assert_eq!(repo["paused"], false);
    assert_eq!(repo["migratedFrom"], Value::Null);
    assert_eq!(repo["branches"], json!(["main"]));
    for key in ["model", "effort", "timeoutSecs", "permission", "claudePath"] {
        assert!(v["worker"].get(key).is_some(), "worker.{key}");
    }
    assert_eq!(v["worker"]["permission"], "restricted");
    assert!(v["stateDb"].is_string());

    // 壊れた設定も状態として報告し、失敗しない。
    fs::write(fx.config_path(), "broken = = =").unwrap();
    let out = fx.run(&["status", "--json"]);
    assert_eq!(out.code, 0);
    let v = out.json();
    assert_eq!(v["config"]["state"], "invalid");
    assert!(v["config"]["error"].is_string());
    // 人が読む形式は JSON ではない。
    let text = fx.run(&["status"]).stdout;
    assert!(text.contains("Scriptura") && !text.trim_start().starts_with('{'));
}

// ---------------------------------------------------------------------------
// doctor
// ---------------------------------------------------------------------------

#[test]
fn doctor_never_prints_secret_values_and_scrubs_host_env_for_auth_status() {
    let fx = Fx::new();
    assert_eq!(fx.enable(&[]).code, 0);
    let log = fx.base.join("auth-log.json");
    let out = fx.exec(
        &["doctor", "--json"],
        &[
            (
                "HOOK_STUB_AUTH_JSON",
                r#"{"loggedIn":true,"authMethod":"claude.ai","token":"sk-ant-SECRETVALUE123","email":"secret.person@example.invalid"}"#,
            ),
            ("HOOK_STUB_AUTH_STDERR", "refresh token sk-ant-STDERRSECRET456"),
            ("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat-ENVSECRET789"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_SIMPLE", "1"),
            ("CLAUDE_CODE_ENTRYPOINT", "desktop"),
            ("ANTHROPIC_BASE_URL", "https://relay.invalid"),
            ("HOOK_STUB_AUTH_LOG", log.to_str().unwrap()),
        ],
        None,
        None,
    );
    let all = format!("{}{}", out.stdout, out.stderr);
    for secret in [
        "SECRETVALUE123",
        "STDERRSECRET456",
        "ENVSECRET789",
        "secret.person",
        "relay.invalid",
    ] {
        assert!(
            !all.contains(secret),
            "{secret} が出力に含まれています:\n{all}"
        );
    }
    let v = out.json();
    let auth = item(&v, "claudeAuth");
    assert_eq!(auth["status"], "ok", "{v}");
    assert!(auth["detail"].as_str().unwrap().contains("claude.ai"));
    // worker と同じ環境変数除去がかかっている。
    let seen: Value = serde_json::from_str(&fs::read_to_string(&log).unwrap()).unwrap();
    assert_eq!(
        seen["inherited_host_env"],
        json!([]),
        "ホスト由来の環境変数が claude へ漏れています"
    );
    // 全項目の形。
    for i in v["items"].as_array().unwrap() {
        assert!(i["id"].is_string() && i["detail"].is_string());
        assert!(["ok", "warn", "fail", "info"].contains(&i["status"].as_str().unwrap()));
    }
}

#[test]
fn doctor_detects_not_logged_in_and_missing_claude() {
    let fx = Fx::new();
    assert_eq!(fx.enable(&[]).code, 0);
    let out = fx.exec(
        &["doctor", "--json"],
        &[
            ("HOOK_STUB_AUTH_EXIT", "1"),
            ("HOOK_STUB_AUTH_JSON", r#"{"loggedIn":false}"#),
        ],
        None,
        None,
    );
    assert_eq!(out.code, 1, "fail があれば終了コードは 1");
    let v = out.json();
    assert_eq!(v["ok"], false);
    let auth = item(&v, "claudeAuth");
    assert_eq!(auth["status"], "fail");
    assert!(
        auth["detail"]
            .as_str()
            .unwrap()
            .contains("claude auth login")
    );
    // ログイン済みなら ok・終了コード 0（git などが無い環境では他項目で落ちうるので claudeAuth だけ見る）。
    let v = fx.run(&["doctor", "--json"]).json();
    assert_eq!(item(&v, "claudeAuth")["status"], "ok");
    assert_eq!(item(&v, "claude")["status"], "ok");
    assert_eq!(item(&v, "config")["status"], "ok");
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["id"] != "consent")
    );
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["id"] != "configLegacy")
    );
    assert_eq!(item(&v, "stateDir")["status"], "ok");

    // claude が見つからない（設定の claude_path が存在しない）。認証確認は行わない。
    let mut config = fx.config();
    config.worker.claude_path = fx
        .base
        .join("nowhere")
        .join("claude.exe")
        .to_string_lossy()
        .into_owned();
    config::save_to(&fx.config_path(), &config).unwrap();
    let out = fx.run(&["doctor", "--json"]);
    assert_eq!(out.code, 1);
    let v = out.json();
    assert_eq!(item(&v, "claude")["status"], "fail");
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["id"] != "claudeAuth")
    );
}

#[test]
fn doctor_reports_config_problems_and_does_not_create_the_state_dir() {
    let fx = Fx::new();
    // 設定なし。状態フォルダーは作らない。
    fs::remove_dir(fx.base.join("state")).unwrap();
    let v = fx.run(&["doctor", "--json"]).json();
    assert_eq!(item(&v, "config")["status"], "warn");
    assert_eq!(item(&v, "stateDir")["status"], "ok");
    assert!(
        item(&v, "stateDir")["detail"]
            .as_str()
            .unwrap()
            .contains("未作成")
    );
    assert!(!fx.base.join("state").exists());
    // 壊れた設定。
    fs::write(fx.config_path(), "broken = = =").unwrap();
    let out = fx.run(&["doctor", "--json"]);
    assert_eq!(out.code, 1);
    assert_eq!(item(&out.json(), "config")["status"], "fail");
    // 旧形式（同意なし）の設定: 変換して読めるので config は ok、旧形式と移行で止めた登録の警告が出る。
    // doctor は設定を書き換えない。
    let root = fx.repo.to_string_lossy().replace('\\', "/");
    let old = format!("[defaults]\nmode = \"propose\"\n[[repository]]\nroot = \"{root}\"\n");
    fs::write(fx.config_path(), &old).unwrap();
    let v = fx.run(&["doctor", "--json"]).json();
    assert_eq!(item(&v, "config")["status"], "ok");
    assert_eq!(item(&v, "configLegacy")["status"], "warn");
    assert_eq!(item(&v, "migratedStopped")["status"], "warn");
    assert!(
        item(&v, "migratedStopped")["detail"]
            .as_str()
            .unwrap()
            .contains("enable")
    );
    assert_eq!(fs::read_to_string(fx.config_path()).unwrap(), old);
    // 新形式に保存し直されても、移行で止めた登録は警告し続ける（旧形式の警告は消える）。
    assert_eq!(
        fx.run(&["pause", "--repo", fx.repo.to_str().unwrap()]).code,
        0
    );
    let v = fx.run(&["doctor", "--json"]).json();
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["id"] != "configLegacy")
    );
    assert_eq!(item(&v, "migratedStopped")["status"], "warn");
}

#[test]
fn doctor_reports_leftover_proposal_patches_without_touching_them() {
    let fx = Fx::new();
    // 廃止した更新案が残した <state>/proposals/<リポジトリ>/*.patch。読みも消しもせず、件数だけを知らせる。
    let v = fx.run(&["doctor", "--json"]).json();
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["id"] != "legacyProposals")
    );
    let dir = fx.base.join("state").join("proposals").join("abcd");
    fs::create_dir_all(&dir).unwrap();
    for name in ["a.patch", "b.patch", "a.json"] {
        fs::write(dir.join(name), "x").unwrap();
    }
    let v = fx.run(&["doctor", "--json"]).json();
    let leftover = item(&v, "legacyProposals");
    assert_eq!(leftover["status"], "info");
    let detail = leftover["detail"].as_str().unwrap();
    assert!(
        detail.contains("2") && detail.contains("git apply") && detail.contains("proposals"),
        "{detail}"
    );
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 3);
}

#[test]
fn doctor_warns_about_duplicate_registration_policy_and_legacy_hooks() {
    let fx = Fx::new();
    // 単独登録だけ・プラグインだけでは警告しない。
    fx.write_settings(&json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "\"C:/x/Scriptura/bin/scriptura.exe\" hook"}]}]}}));
    let v = fx.run(&["doctor", "--json"]).json();
    assert_eq!(item(&v, "hookDuplicate")["status"], "ok");
    // 旧登録の Stop hook が残っていれば、再 connect を案内する（新しい登録では出ない）。
    let old = item(&v, "oldStopHook");
    assert_eq!(old["status"], "warn");
    assert!(
        old["detail"]
            .as_str()
            .unwrap()
            .contains("connect claude-code --apply")
    );
    fx.write_settings(&json!({"hooks": {"PostToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "\"C:/x/Scriptura/bin/scriptura.exe\" hook", "if": "Bash(* push*)"}]}]}}));
    let v = fx.run(&["doctor", "--json"]).json();
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["id"] != "oldStopHook")
    );
    fx.write_settings(&json!({"enabledPlugins": {"scriptura@scriptura": true}}));
    let v = fx.run(&["doctor", "--json"]).json();
    assert_eq!(item(&v, "hookDuplicate")["status"], "ok");
    // 両方あれば警告し、解除コマンドを案内する。
    fx.write_settings(&json!({
        "enabledPlugins": {"scriptura@scriptura": true},
        "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "\"C:/x/Scriptura/bin/scriptura.exe\" hook"}]}]}
    }));
    let v = fx.run(&["doctor", "--json"]).json();
    let dup = item(&v, "hookDuplicate");
    assert_eq!(dup["status"], "warn");
    assert!(
        dup["detail"]
            .as_str()
            .unwrap()
            .contains("connect claude-code --remove --apply")
    );
    // 無効化されたプラグインは数えない。
    fx.write_settings(&json!({
        "enabledPlugins": {"scriptura@scriptura": false},
        "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "\"C:/x/Scriptura/bin/scriptura.exe\" hook"}]}]}
    }));
    assert_eq!(
        item(&fx.run(&["doctor", "--json"]).json(), "hookDuplicate")["status"],
        "ok"
    );
    // 組織ポリシー・旧登録。
    fx.write_settings(&json!({
        "disableAllHooks": true,
        "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "bun C:/h/agents-maintenance.ts"}]}]}
    }));
    let v = fx.run(&["doctor", "--json"]).json();
    assert_eq!(item(&v, "hookPolicy")["status"], "warn");
    assert!(
        item(&v, "hookPolicy")["detail"]
            .as_str()
            .unwrap()
            .contains("disableAllHooks")
    );
    assert_eq!(item(&v, "legacyHook")["status"], "warn");
}

#[test]
fn doctor_verify_checks_the_plugin_bundle_hashes() {
    let fx = Fx::new();
    // プラグイン外の実行ファイルは対象外（info）。
    let v = fx.run(&["doctor", "--json", "--verify"]).json();
    assert_eq!(item(&v, "verify")["status"], "info");
    // --verify なしでは項目自体が出ない。
    assert!(
        fx.run(&["doctor", "--json"]).json()["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["id"] != "verify")
    );

    // <plugin>/bin/ へ置いた本体と SHA256SUMS を照合する。
    let plugin = fx.base.join("plugin");
    fs::create_dir_all(plugin.join("bin")).unwrap();
    fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
    fs::write(plugin.join(".claude-plugin").join("plugin.json"), "{}").unwrap();
    let name = if cfg!(windows) {
        "scriptura.exe"
    } else {
        "scriptura"
    };
    let bundled = plugin.join("bin").join(name);
    fs::copy(EXE, &bundled).unwrap();
    let hex: String = Sha256::digest(fs::read(&bundled).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    // 実環境の claude を呼ばないよう、通常の実行と同じ隔離環境（スタブの claude だけが見える）で動かす。
    let run_bundled = |fx: &Fx| -> Value {
        fx.exec_program(&bundled, &["doctor", "--json", "--verify"], &[], None, None)
            .json()
    };
    // SHA256SUMS が無いプラグインは配布事故（fail）。
    assert_eq!(item(&run_bundled(&fx), "verify")["status"], "fail");
    fs::write(plugin.join("SHA256SUMS"), format!("{hex}  bin/{name}\n")).unwrap();
    assert_eq!(item(&run_bundled(&fx), "verify")["status"], "ok");
    // ハッシュが違えば fail。
    fs::write(
        plugin.join("SHA256SUMS"),
        format!("{}  bin/{name}\n", "0".repeat(64)),
    )
    .unwrap();
    assert_eq!(item(&run_bundled(&fx), "verify")["status"], "fail");
    // 本体が一覧に載っていなければ fail。
    fs::write(plugin.join("SHA256SUMS"), format!("{hex}  bin/other\n")).unwrap();
    assert_eq!(item(&run_bundled(&fx), "verify")["status"], "fail");
    // 親ディレクトリへ出るパスは拒否する。
    fs::write(plugin.join("SHA256SUMS"), format!("{hex}  ../{name}\n")).unwrap();
    assert_eq!(item(&run_bundled(&fx), "verify")["status"], "fail");
}

// ---------------------------------------------------------------------------
// connect --remove
// ---------------------------------------------------------------------------

/// `connect --apply` は旧登録の Stop hook を外して新しい登録（PostToolUse・PostToolUseFailure・SessionStart）へ置き換え、
/// JSON の `events` に登録するイベントを返す。`--remove` の `events` は新旧すべて。
#[test]
fn connect_apply_migrates_a_stop_registration_and_reports_events() {
    let fx = Fx::new();
    let own = format!("\"{}\" hook", EXE.replace('\\', "/"));
    fx.write_settings(&json!({"hooks": {"Stop": [
        {"hooks": [{"type": "command", "command": own}]},
        {"hooks": [{"type": "command", "command": "other-tool stop"}]}
    ]}}));
    let preview = fx.run(&["connect", "claude-code", "--executable", EXE, "--json"]);
    assert_eq!(preview.code, 0, "{}", preview.stderr);
    let v = preview.json();
    assert_eq!(
        v["events"],
        json!(["PostToolUse", "PostToolUseFailure", "SessionStart"])
    );
    assert_eq!(
        (v["applied"].clone(), v["changed"].clone()),
        (json!(false), json!(true))
    );
    let out = fx.run(&["connect", "claude-code", "--executable", EXE, "--apply"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let after: Value = serde_json::from_slice(&fs::read(fx.settings_path()).unwrap()).unwrap();
    assert_eq!(after["hooks"]["Stop"].as_array().unwrap().len(), 1);
    assert_eq!(
        after["hooks"]["Stop"][0]["hooks"][0]["command"],
        "other-tool stop"
    );
    for event in ["PostToolUse", "PostToolUseFailure"] {
        let entries = after["hooks"][event].as_array().unwrap();
        assert_eq!(entries.len(), 2, "{event}");
        assert_eq!(entries[0]["matcher"], "Bash");
        assert_eq!(entries[0]["hooks"][0]["if"], "Bash(* push*)");
        assert_eq!(entries[1]["hooks"][0]["if"], "PowerShell(* push*)");
    }
    assert_eq!(
        after["hooks"]["SessionStart"][0]["matcher"],
        "startup|resume"
    );
    // 冪等。
    let again = fx
        .run(&["connect", "claude-code", "--executable", EXE, "--json"])
        .json();
    assert_eq!(again["changed"], false);
    // --remove は新旧すべてのイベントから外し、events に新旧を返す。
    let removed = fx
        .run(&["connect", "claude-code", "--remove", "--apply"])
        .json();
    assert_eq!(removed["removed"], 5);
    assert_eq!(
        removed["events"],
        json!(["Stop", "PostToolUse", "PostToolUseFailure", "SessionStart"])
    );
    let last: Value = serde_json::from_slice(&fs::read(fx.settings_path()).unwrap()).unwrap();
    assert_eq!(last["hooks"].as_object().unwrap().len(), 1);
}

/// Stop で呼ばれた hook は、ユーザー単位の settings.json の旧登録を新しい登録へ移行して 1 回だけ知らせる。
/// 設定も旧状態も無い新規利用者でも状態フォルダー・DB は作らず、2 回目は何もしない。
#[test]
fn stop_hook_migrates_the_old_registration_once_without_creating_state() {
    let fx = Fx::new();
    let own = format!("\"{}\" hook", EXE.replace('\\', "/"));
    let original = json!({"model": "keep", "hooks": {"Stop": [
        {"hooks": [{"type": "command", "command": "bun C:/h/agents-maintenance.ts"}]},
        {"hooks": [{"type": "command", "command": own, "async": true, "timeout": 1800}]}
    ]}});
    fx.write_settings(&original);
    let out = fx.hook(&[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let v = out.json();
    assert!(
        v["systemMessage"]
            .as_str()
            .unwrap()
            .contains("settings.json.scriptura-migration-backup")
    );
    let after: Value = serde_json::from_slice(&fs::read(fx.settings_path()).unwrap()).unwrap();
    assert_eq!(after["model"], "keep");
    assert_eq!(after["hooks"]["Stop"].as_array().unwrap().len(), 1);
    assert!(
        after["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("agents-maintenance")
    );
    assert_eq!(
        after["hooks"]["PostToolUse"][0]["hooks"][0]["if"],
        "Bash(* push*)"
    );
    assert_eq!(
        after["hooks"]["PostToolUseFailure"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        after["hooks"]["SessionStart"][0]["matcher"],
        "startup|resume"
    );
    let backup = fx
        .base
        .join("claude")
        .join("settings.json.scriptura-migration-backup");
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(backup).unwrap()).unwrap(),
        original
    );
    // 設定も旧状態も無い新規利用者: 状態フォルダー・DB を作らない。
    assert!(!fx.state_db().exists());
    assert_eq!(fs::read_dir(fx.base.join("state")).unwrap().count(), 0);
    // 2 回目は何も書かず、何も出さない。
    let written = fs::read(fx.settings_path()).unwrap();
    let again = fx.hook(&[]);
    assert_eq!(again.stdout.trim(), "");
    assert_eq!(fs::read(fx.settings_path()).unwrap(), written);
    // Stop 以外のイベントでは移行しない。
    fx.write_settings(&original);
    let payload = json!({"hook_event_name": "SessionStart", "cwd": fx.repo}).to_string();
    let other = fx.exec(&["hook"], &[], Some(&payload), None);
    assert_eq!(other.stdout.trim(), "");
    let untouched: Value = serde_json::from_slice(&fs::read(fx.settings_path()).unwrap()).unwrap();
    assert_eq!(untouched, original);
}

#[test]
fn connect_remove_drops_only_scriptura_entries_and_is_safe_and_idempotent() {
    let fx = Fx::new();
    let original = json!({
        "model": "keep-me",
        "enabledPlugins": {"scriptura@scriptura": true},
        "hooks": {
            "Stop": [
                {"hooks": [{"type": "command", "command": "\"C:/x/Scriptura/bin/scriptura.exe\" hook", "async": true}]},
                {"hooks": [{"type": "command", "command": "bun C:/h/agents-maintenance.ts"}]},
                {"hooks": [{"type": "command", "command": "other-tool stop"}]}
            ],
            "PostToolUse": [
                {"matcher": "Bash", "hooks": [{"type": "command", "command": "\"C:/x/Scriptura/bin/scriptura.exe\" hook", "if": "Bash(* push *)"}]},
                {"matcher": "Edit", "hooks": [{"type": "command", "command": "fmt"}]}
            ],
            "SessionStart": [
                {"matcher": "startup|resume", "hooks": [{"type": "command", "command": "\"C:/x/Scriptura/bin/scriptura.exe\" hook"}]}
            ],
            "Notification": [{"hooks": [{"type": "command", "command": "notify"}]}]
        }
    });
    fx.write_settings(&original);
    let before = fs::read(fx.settings_path()).unwrap();
    // 既定はプレビュー（書き込まない）。
    let out = fx.run(&["connect", "claude-code", "--remove", "--json"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let v = out.json();
    assert_eq!(v["ok"], true);
    assert_eq!(
        (
            v["applied"].clone(),
            v["changed"].clone(),
            v["removed"].clone()
        ),
        (json!(false), json!(true), json!(3))
    );
    assert_eq!(fs::read(fx.settings_path()).unwrap(), before);
    // --apply で Scriptura の項目だけ消え、退避が作られる。
    let out = fx.run(&["connect", "claude-code", "--remove", "--apply"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let backup = fx
        .base
        .join("claude")
        .join("settings.json.scriptura-backup");
    assert_eq!(
        fs::read(&backup).unwrap(),
        before,
        "最初の変更前の内容を退避する"
    );
    let after: Value = serde_json::from_slice(&fs::read(fx.settings_path()).unwrap()).unwrap();
    let stop = after["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 2);
    assert!(
        stop[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("agents-maintenance")
    );
    assert_eq!(stop[1]["hooks"][0]["command"], "other-tool stop");
    // 新しい登録（PostToolUse・SessionStart）の Scriptura の hook も外れ、他ツールの項目と空のイベントの整理は保たれる。
    assert_eq!(after["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
    assert_eq!(
        after["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
        "fmt"
    );
    assert!(after["hooks"].get("SessionStart").is_none());
    assert_eq!(after["model"], "keep-me");
    assert_eq!(after["enabledPlugins"], original["enabledPlugins"]);
    assert_eq!(
        after["hooks"]["Notification"],
        original["hooks"]["Notification"]
    );
    // 冪等: 2 回目は何も変えず、退避も上書きしない。
    let written = fs::read(fx.settings_path()).unwrap();
    let v = fx
        .run(&["connect", "claude-code", "--remove", "--apply"])
        .json();
    assert_eq!(
        (v["changed"].clone(), v["removed"].clone()),
        (json!(false), json!(0))
    );
    assert_eq!(fs::read(fx.settings_path()).unwrap(), written);
    assert_eq!(fs::read(&backup).unwrap(), before);
}

#[test]
fn connect_remove_does_not_rewrite_broken_settings_or_create_files_when_absent() {
    let fx = Fx::new();
    // settings.json が無い: 何も作らない。
    let out = fx.run(&["connect", "claude-code", "--remove", "--apply"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.json()["changed"], false);
    assert!(!fx.settings_path().exists());
    // 壊れた JSON: 書き換えず、退避も作らず、終了コード 1。
    fs::write(fx.settings_path(), "{ not json").unwrap();
    let out = fx.run(&["connect", "claude-code", "--remove", "--apply"]);
    assert_eq!(out.code, 1);
    assert_eq!(
        fs::read_to_string(fx.settings_path()).unwrap(),
        "{ not json"
    );
    assert!(
        !fx.base
            .join("claude")
            .join("settings.json.scriptura-backup")
            .exists()
    );
    // --json では失敗も JSON で返る。
    let out = fx.run(&["connect", "claude-code", "--remove", "--json"]);
    assert_eq!(out.code, 1);
    assert_eq!(out.json()["ok"], false);
    // --executable とは併用できない。
    let out = fx.run(&[
        "connect",
        "claude-code",
        "--remove",
        "--executable",
        fx.base.join("x").to_str().unwrap(),
    ]);
    assert_eq!(out.code, 1);
}

// ---------------------------------------------------------------------------
// run-now
// ---------------------------------------------------------------------------

#[test]
fn run_now_refuses_unregistered_repositories_and_missing_config() {
    let fx = Fx::new();
    // 設定なし。
    let out = fx.exec(&["run-now", "--json"], &[], None, Some(&fx.repo));
    assert_eq!(out.code, 1);
    assert_eq!(out.json()["ok"], false);
    // 別のリポジトリだけ登録 → こちらは未登録で拒否（worker は呼ばれない）。
    let other = fx.base.join("other");
    fx.make_pushed_repo(&other);
    assert_eq!(fx.enable_repo(&other, &[]).code, 0);
    let out = fx.exec(&["run-now", "--json"], &[], None, Some(&fx.repo));
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("登録"));
    assert!(!fx.base.join("prompt.json").exists());
    // 登録済みでも一時停止中なら拒否。
    assert_eq!(fx.enable(&[]).code, 0);
    assert_eq!(
        fx.run(&["pause", "--repo", fx.repo.to_str().unwrap()]).code,
        0
    );
    let out = fx.exec(&["run-now", "--json"], &[], None, Some(&fx.repo));
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("一時停止"));
    // 旧形式からの移行で止めた登録も拒否し、enable での有効化し直しを案内する（worker は呼ばれない）。
    assert_eq!(
        fx.run(&["resume", "--repo", fx.repo.to_str().unwrap()])
            .code,
        0
    );
    let mut config = fx.config();
    let mine = config.repository.last_mut().unwrap();
    mine.paused = true;
    mine.migrated_from = Some(MigratedFrom::Propose);
    config::save_to(&fx.config_path(), &config).unwrap();
    let out = fx.exec(&["run-now", "--json"], &[], None, Some(&fx.repo));
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("enable"));
    assert!(!fx.base.join("prompt.json").exists());
}

#[test]
fn run_now_commits_and_pushes_the_documents() {
    let fx = Fx::new();
    assert_eq!(fx.enable(&[]).code, 0);
    fx.push_change("a change worth documenting");
    let writes = json!({"README.md": "updated readme\n"}).to_string();
    let out = fx.exec(
        &["run-now", "--json"],
        &[("HOOK_STUB_WRITES", &writes)],
        None,
        Some(&fx.repo),
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let v = out.json();
    assert_eq!(v["ok"], true);
    assert_eq!(v["outcome"], "completed");
    assert!(
        v.get("mode").is_none() && v.get("sha").is_none() && v.get("patchPath").is_none(),
        "{v}"
    );
    // 文書は commit され、remote へ push 済み（更新案は作られない）。
    assert_eq!(
        fs::read_to_string(fx.repo.join("README.md"))
            .unwrap()
            .replace("\r\n", "\n"),
        "updated readme\n"
    );
    let head = fx.git(&["rev-parse", "HEAD"]).trim().to_string();
    assert_eq!(fx.git(&["rev-parse", "origin/main"]).trim(), head);
    assert!(fx.git(&["status", "--porcelain"]).trim().is_empty());
    assert!(!fx.base.join("state").join("proposals").exists());
    // 保守履歴: run-now は契機 run_now の committed として 1 行残る。
    {
        let db = rusqlite::Connection::open(fx.state_db()).unwrap();
        let rows = scriptura::runlog::load_from(&db, None, None, 10)
            .unwrap()
            .rows;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].run.origin, "run_now");
        assert_eq!(rows[0].run.outcome, "committed");
        assert_eq!(rows[0].run.result_commit, head);
    }
    // 人が読む形式。
    fx.push_change("one more change");
    let writes = json!({"README.md": "updated readme again\n"}).to_string();
    let out = fx.exec(
        &["run-now"],
        &[("HOOK_STUB_WRITES", &writes)],
        None,
        Some(&fx.repo),
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("push"), "{}", out.stdout);
}

// ---------------------------------------------------------------------------
// docs list / docs show・run-now --documents・Rejected・doctor の kinds（P5）
// ---------------------------------------------------------------------------

/// 設定（無ければ既定）へ種類の定義を書き足して保存する（保存は schema 2 になる）。
fn define_kinds(fx: &Fx, edit: impl FnOnce(&mut Config)) {
    let mut config = match config::load_from(&fx.config_path()) {
        Loaded::Ok(c) => c,
        Loaded::Missing => Config::default(),
        other => panic!("設定を読めません: {other:?}"),
    };
    edit(&mut config);
    config::save_to(&fx.config_path(), &config).unwrap();
}

/// カスタム種類 `architecture`（テンプレート・上限・タイミング・追加指示つき）の定義。
fn architecture_kind() -> config::KindSettings {
    config::KindSettings {
        paths: vec!["docs/ARCHITECTURE.md".into()],
        guidance: Some("システム構成と責務を書く".into()),
        template: Some("templates/architecture.md".into()),
        max_chars: Some(15000),
        instructions: Some("日本語で簡潔に".into()),
        timing: Some(config::Timing {
            when: Some(config::When::Push),
            paths: vec!["src/**".into()],
            min_interval_hours: Some(24),
        }),
        ..Default::default()
    }
}

fn write_architecture_template(fx: &Fx) {
    fs::create_dir_all(fx.base.join("templates")).unwrap();
    fs::write(
        fx.base.join("templates").join("architecture.md"),
        "# 構成\n\n## 責務\n",
    )
    .unwrap();
}

fn kind_of<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["id"] == id)
        .unwrap_or_else(|| panic!("種類 {id} がありません: {v}"))
}

#[test]
fn docs_list_and_show_report_resolved_kinds_read_only() {
    let fx = Fx::new();
    write_architecture_template(&fx);
    define_kinds(&fx, |c| {
        c.kinds.insert("architecture".into(), architecture_kind());
        c.kinds.insert(
            "readme".into(),
            config::KindSettings {
                max_chars: Some(8000),
                language: Some("ja".into()),
                required_headings: vec!["## 概要".into()],
                ..Default::default()
            },
        );
    });
    assert_eq!(
        fx.enable(&["--documents", "README.md,architecture"]).code,
        0
    );
    let before = fs::read(fx.config_path()).unwrap();

    let out = fx.exec(&["docs", "list", "--json"], &[], None, Some(&fx.repo));
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let v = out.json();
    assert_eq!(v["ok"], true);
    assert_eq!(v["registered"], true);
    assert!(v.get("mode").is_none(), "{v}");
    let ids: Vec<&str> = v["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["agents", "design", "readme", "changelog", "architecture"]
    );
    let readme = kind_of(&v, "readme");
    assert_eq!(readme["enabled"], true);
    assert_eq!(readme["builtin"], true);
    assert_eq!(readme["maxChars"], 8000);
    assert_eq!(readme["maxBytes"], Value::Null);
    // 上限を設定した種類は既定で docs/<id> へ分割できる（上限の無い種類・CHANGELOG は分割しない）。
    assert_eq!(readme["splitDir"], "docs/readme");
    assert_eq!(kind_of(&v, "agents")["splitDir"], Value::Null);
    assert_eq!(kind_of(&v, "changelog")["splitDir"], Value::Null);
    assert_eq!(
        readme["paths"],
        json!([{"path": "README.md", "exists": true}])
    );
    assert_eq!(readme["lastDoneAt"], Value::Null);
    assert_eq!(kind_of(&v, "changelog")["enabled"], false);
    assert_eq!(kind_of(&v, "changelog")["template"]["state"], "builtin");
    assert_eq!(kind_of(&v, "agents")["template"]["state"], "none");
    let arch = kind_of(&v, "architecture");
    assert_eq!(arch["enabled"], true);
    assert_eq!(arch["builtin"], false);
    assert_eq!(
        arch["paths"],
        json!([{"path": "docs/ARCHITECTURE.md", "exists": false}])
    );
    assert_eq!(arch["template"]["spec"], "templates/architecture.md");
    assert_eq!(arch["template"]["state"], "ok");
    assert_eq!(arch["maxChars"], 15000);
    assert_eq!(arch["splitDir"], "docs/architecture");
    assert_eq!(
        arch["timing"],
        json!({"when": "push", "paths": ["src/**"], "minIntervalHours": 24})
    );
    assert_eq!(arch["hasInstructions"], true);
    // list には追加指示の本文や指針を出さない（show だけ）。
    assert!(arch.get("instructions").is_none() && arch.get("guidance").is_none());

    // show: 詳細（指針・追加指示・必須見出し・言語）。組み込みは文書名でも大文字小文字違いでも引ける。
    let v = fx
        .exec(
            &["docs", "show", "architecture", "--json"],
            &[],
            None,
            Some(&fx.repo),
        )
        .json();
    let k = &v["kind"];
    assert_eq!(k["guidance"], "システム構成と責務を書く");
    assert_eq!(k["instructions"], "日本語で簡潔に");
    assert_eq!(k["changelog"], Value::Null);
    let v = fx
        .exec(
            &["docs", "show", "readme.MD", "--json"],
            &[],
            None,
            Some(&fx.repo),
        )
        .json();
    assert_eq!(v["kind"]["id"], "readme");
    assert_eq!(v["kind"]["language"], "ja");
    assert_eq!(v["kind"]["requiredHeadings"], json!(["## 概要"]));
    assert_eq!(v["kind"]["paths"][0]["chars"], 6); // "readme"
    assert_eq!(v["kind"]["paths"][0]["bytes"], 6);
    let v = fx
        .exec(
            &["docs", "show", "changelog", "--json"],
            &[],
            None,
            Some(&fx.repo),
        )
        .json();
    assert_eq!(v["kind"]["changelog"]["onRelease"], "promote");
    assert_eq!(v["kind"]["changelog"]["releaseTag"], "v*");
    assert_eq!(v["kind"]["changelog"]["backfillMaxReleases"], 20);
    // 人が読む形式。
    let out = fx.exec(&["docs", "list"], &[], None, Some(&fx.repo));
    assert_eq!(out.code, 0);
    assert!(
        out.stdout.contains("- architecture [有効]"),
        "{}",
        out.stdout
    );
    let out = fx.exec(&["docs", "show", "architecture"], &[], None, Some(&fx.repo));
    assert!(
        out.stdout.contains("指針: システム構成と責務を書く"),
        "{}",
        out.stdout
    );

    // テンプレートが無くなると state=error と理由が出る（保守が見送られる原因）。
    fs::remove_file(fx.base.join("templates").join("architecture.md")).unwrap();
    let v = fx
        .exec(&["docs", "list", "--json"], &[], None, Some(&fx.repo))
        .json();
    let t = &kind_of(&v, "architecture")["template"];
    assert_eq!(t["state"], "error");
    assert!(t["error"].as_str().unwrap().contains("architecture"));

    // 未定義の id・使い方の誤りは拒否。読み取り専用なので設定は変わらない。
    for args in [
        vec!["docs", "show", "nosuch", "--json"],
        vec!["docs", "show", "--json"],
        vec!["docs", "--json"],
        vec!["docs", "list", "extra", "--json"],
    ] {
        let out = fx.exec(&args, &[], None, Some(&fx.repo));
        assert_eq!(out.code, 1, "{args:?}");
        assert_eq!(out.json()["ok"], false);
    }
    assert_eq!(fs::read(fx.config_path()).unwrap(), before);
}

#[test]
fn docs_list_works_for_unregistered_repos_and_refuses_broken_config() {
    let fx = Fx::new();
    // 設定なし: 組み込み既定だけで、どれも無効。
    let v = fx
        .exec(&["docs", "list", "--json"], &[], None, Some(&fx.repo))
        .json();
    assert_eq!(v["configPresent"], false);
    assert_eq!(v["registered"], false);
    assert!(
        v["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|k| k["enabled"] == false)
    );
    assert_eq!(v["kinds"].as_array().unwrap().len(), 4);
    // 壊れた設定は書き換えず、理由を返して失敗する。
    fs::write(fx.config_path(), "schema = [").unwrap();
    let out = fx.exec(&["docs", "list", "--json"], &[], None, Some(&fx.repo));
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("読めません"));
    assert_eq!(fs::read_to_string(fx.config_path()).unwrap(), "schema = [");
}

#[test]
fn run_now_documents_narrows_the_kinds_and_refuses_unknown_or_disabled_ones() {
    let fx = Fx::new();
    assert_eq!(fx.enable(&["--documents", "README.md,DESIGN.md"]).code, 0);
    fx.push_change("narrowing");
    let prompt = fx.base.join("prompt.json");
    let run = |args: &[&str]| fx.exec(args, &[], None, Some(&fx.repo));

    // 拒否: 未定義のカスタム id・構文の誤り・このリポジトリで有効でない種類。worker は起動しない。
    let out = run(&["run-now", "--documents", "nosuch", "--json"]);
    assert_eq!(out.code, 1);
    assert!(
        out.json()["error"]
            .as_str()
            .unwrap()
            .contains("定義されていません")
    );
    let out = run(&["run-now", "--documents", "FOO.md", "--json"]);
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("未対応"));
    let out = run(&["run-now", "--documents", "CHANGELOG.md", "--json"]);
    assert_eq!(out.code, 1);
    assert!(
        out.json()["error"]
            .as_str()
            .unwrap()
            .contains("有効ではない")
    );
    assert!(!prompt.exists(), "拒否では worker を起動しない");

    // README だけに絞る。DESIGN.md は有効だが今回は処理しない。
    let writes = json!({"README.md": "narrowed readme\n"}).to_string();
    let out = fx.exec(
        &["run-now", "--documents", "readme", "--json"],
        &[("HOOK_STUB_WRITES", &writes)],
        None,
        Some(&fx.repo),
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let v = out.json();
    assert_eq!(v["outcome"], "completed");
    let sent = fs::read_to_string(&prompt).unwrap();
    assert!(sent.contains("README.md"));
    assert!(
        !sent.contains("DESIGN"),
        "絞り込んだ種類だけを worker へ渡す"
    );

    // 対照: 絞らなければ DESIGN.md も対象になる（別の環境で同じ手順）。
    let control = Fx::new();
    assert_eq!(
        control.enable(&["--documents", "README.md,DESIGN.md"]).code,
        0
    );
    control.push_change("narrowing");
    let out = control.exec(
        &["run-now", "--json"],
        &[("HOOK_STUB_WRITES", &writes)],
        None,
        Some(&control.repo),
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(
        fs::read_to_string(control.base.join("prompt.json"))
            .unwrap()
            .contains("DESIGN")
    );

    // 完了した種類の最終完了時刻が docs list に出る（絞った README だけ）。
    let v = fx
        .exec(&["docs", "list", "--json"], &[], None, Some(&fx.repo))
        .json();
    assert!(kind_of(&v, "readme")["lastDoneAt"].is_string());
    assert_eq!(kind_of(&v, "design")["lastDoneAt"], Value::Null);
}

#[test]
fn run_now_commits_with_a_warning_when_the_limits_are_still_violated_after_reruns() {
    let fx = Fx::new();
    // README の上限を 5 文字にする。worker が長い README を書き続けても、保守は止めず警告つきで完了する。
    define_kinds(&fx, |c| {
        c.kinds.insert(
            "readme".into(),
            config::KindSettings {
                max_chars: Some(5),
                ..Default::default()
            },
        );
    });
    assert_eq!(fx.enable(&["--documents", "README.md"]).code, 0);
    fx.push_change("rejected change");
    let head = fx.git(&["rev-parse", "HEAD"]).trim().to_string();
    let writes = json!({"README.md": "this readme is far too long
"})
    .to_string();
    let out = fx.exec(
        &["run-now", "--json"],
        &[("HOOK_STUB_WRITES", &writes)],
        None,
        Some(&fx.repo),
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let v = out.json();
    assert_eq!(v["ok"], true);
    let warning = v["warnings"][0].as_str().unwrap();
    assert!(
        warning.contains("README.md") && warning.contains("上限"),
        "{v}"
    );
    let now = fx.git(&["rev-parse", "HEAD"]).trim().to_string();
    assert_ne!(now, head, "警告つきで commit する");
    assert_eq!(fx.git(&["rev-parse", "origin/main"]).trim(), now);
}

#[test]
fn doctor_reports_kinds_templates_limits_and_changelog_notes() {
    let fx = Fx::new();
    // 既定の設定（種類の定義なし）: kinds 項目は ok で、カスタム 0 種。
    assert_eq!(fx.enable(&[]).code, 0);
    let v = fx.run(&["doctor", "--json"]).json();
    assert_eq!(item(&v, "kinds")["status"], "ok");
    assert!(
        item(&v, "kinds")["detail"]
            .as_str()
            .unwrap()
            .contains("カスタム 0 種")
    );

    // カスタム種類＋テンプレートが読める＋上限超過の既存文書。
    write_architecture_template(&fx);
    define_kinds(&fx, |c| {
        c.kinds.insert("architecture".into(), architecture_kind());
        c.kinds.insert(
            "readme".into(),
            config::KindSettings {
                max_chars: Some(3), // 既存の README（6 文字）より小さい
                ..Default::default()
            },
        );
        c.repository[0].documents = Some(vec![
            config::Document::Readme,
            config::Document::Changelog,
            config::Document::Custom("architecture".into()),
        ]);
    });
    let v = fx.run(&["doctor", "--json"]).json();
    let kinds = item(&v, "kinds");
    assert_eq!(kinds["status"], "ok");
    assert!(
        kinds["detail"]
            .as_str()
            .unwrap()
            .contains("カスタム 1 種（architecture）")
    );
    assert!(
        kinds["detail"]
            .as_str()
            .unwrap()
            .contains("読めるもの 1 件")
    );
    assert_eq!(item(&v, "kindsLimits")["status"], "info");
    assert!(
        item(&v, "kindsLimits")["detail"]
            .as_str()
            .unwrap()
            .contains("README.md")
    );
    // CHANGELOG を保守する有効な登録があるので、リリース作業との競合の注意（info）が出る。
    assert_eq!(item(&v, "kindsChangelog")["status"], "info");

    // テンプレートが無い種類が有効なら fail（保守が見送られ続ける）。
    fs::remove_file(fx.base.join("templates").join("architecture.md")).unwrap();
    let out = fx.run(&["doctor", "--json"]);
    assert_eq!(out.code, 1);
    let v = out.json();
    assert_eq!(item(&v, "kinds")["status"], "fail");
    assert!(
        item(&v, "kinds")["detail"]
            .as_str()
            .unwrap()
            .contains("architecture")
    );

    // どの登録でも有効でなければ warn。CHANGELOG を保守する有効な登録があれば注意（info）。
    define_kinds(&fx, |c| {
        c.repository[0].documents = Some(vec![config::Document::Changelog]);
    });
    let out = fx.run(&["doctor", "--json"]);
    let v = out.json();
    assert_eq!(item(&v, "kinds")["status"], "warn");
    assert_eq!(item(&v, "kindsChangelog")["status"], "info");
    // 診断はテンプレートの中身などを出力しない。
    assert!(!out.stdout.contains("責務"));
}

// ---------------------------------------------------------------------------
// Setup（Velopack）版の機能: 管理外のビルドでは何もしない
// ---------------------------------------------------------------------------

/// 状態フォルダー（SCRIPTURA_HOME/state）に、Setup 版の更新・登録のファイルがあるか。
fn setup_files(fx: &Fx) -> Vec<String> {
    let state = fx.base.join("home").join("state");
    fs::read_dir(&state)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("app-update") || n.starts_with("setup-registration"))
        .collect()
}

#[test]
fn setup_features_do_nothing_in_a_non_velopack_build() {
    // テストの実行ファイル（target/ 配下）は開発ビルドと同じく Velopack の管理下ではない。
    let fx = Fx::new();
    let settings = json!({
        "enabledPlugins": {"other@x": true},
        "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify"}]}]}
    });
    fx.write_settings(&settings);
    let before = fs::read(fx.settings_path()).unwrap();

    // ログオン時の適用は、管理外なら JSON 1 行で unmanaged を返して終了コード 0。
    let out = fx.run(&["update-apply"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.json(), json!({"status": "unmanaged"}));
    // 手動の確認・解除も何もしない。
    let v = fx.run(&["update", "--check", "--json"]).json();
    assert_eq!(v["status"], "unmanaged");
    assert_eq!(v["ok"], true);
    let v = fx.run(&["uninstall-setup", "--apply", "--json"]).json();
    assert_eq!(v["status"], "unmanaged");
    // ログオン時の管理画面の常駐（ui --autostart）は Setup 版だけの機能。管理外では断り、何も書かない
    // （Windows・macOS は終了コード 1、常駐に対応しない OS は 2）。
    let out = fx.run(&["ui", "--autostart", "on"]);
    assert_ne!(out.code, 0, "{}", out.stdout);
    assert!(out.stdout.is_empty(), "{}", out.stdout);
    assert!(!out.stderr.is_empty());
    // 引数なしの起動は従来の使い方を表示する（Setup の案内や登録はしない）。
    let out = fx.run(&[]);
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("使い方"), "{}", out.stdout);
    assert!(out.stdout.contains("update-apply"));
    // hook の後の確認・取得もしない（stdout は保守の結果だけ）。
    let out = fx.hook(&[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    // status・doctor は Setup 版でないことを報告する。
    let v = fx.run(&["status", "--json"]).json();
    assert_eq!(v["setup"]["managed"], false);
    assert_eq!(v["setup"]["autoUpdate"], true);
    assert_eq!(v["setup"]["trayAutostart"], false);
    assert_eq!(v["setup"]["registration"], Value::Null);
    let v = fx.run(&["doctor", "--json"]).json();
    let setup = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "setup")
        .expect("setup item");
    assert_eq!(setup["status"], "info");
    // どの操作でも、settings.json と状態フォルダーの Setup 用ファイルに触れていない。
    assert_eq!(fs::read(fx.settings_path()).unwrap(), before);
    assert!(setup_files(&fx).is_empty(), "{:?}", setup_files(&fx));
}

#[test]
fn update_auto_setting_is_saved_in_the_state_folder() {
    let fx = Fx::new();
    let v = fx.run(&["update", "--auto", "off", "--json"]).json();
    assert_eq!(v["ok"], true);
    assert_eq!(v["autoUpdate"], false);
    assert_eq!(v["changed"], true);
    let saved: Value = serde_json::from_slice(
        &fs::read(fx.base.join("home").join("state").join("app-update.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["auto_update"], false);
    let v = fx.run(&["update", "--json"]).json();
    assert_eq!(v["action"], "show");
    assert_eq!(v["autoUpdate"], false);
    assert_eq!(v["managed"], false);
    assert_eq!(
        fx.run(&["update", "--auto", "off", "--json"]).json()["changed"],
        false
    );
    assert_eq!(fx.run(&["update", "--auto", "--json"]).code, 1);
    // 不正な値・組み合わせは終了コード 1。
    assert_eq!(fx.run(&["update", "--auto", "maybe", "--json"]).code, 1);
    assert_eq!(
        fx.run(&["update", "--check", "--auto", "on", "--json"])
            .code,
        1
    );
    let text = fx.run(&["update", "--auto", "on"]).stdout;
    assert!(text.contains("自動更新を ON にしました"), "{text}");
}

#[test]
fn status_reads_install_json_and_falls_back_to_the_legacy_current_json() {
    let fx = Fx::new();
    let home = fx.base.join("home");
    assert_eq!(
        fx.run(&["status", "--json"]).json()["installedVersion"],
        Value::Null
    );
    fs::create_dir_all(home.join("app")).unwrap();
    fs::write(
        home.join("app").join("current.json"),
        br#"{"version":"0.0.9"}"#,
    )
    .unwrap();
    assert_eq!(
        fx.run(&["status", "--json"]).json()["installedVersion"],
        "0.0.9"
    );
    fs::create_dir_all(home.join("state")).unwrap();
    fs::write(
        home.join("state").join("install.json"),
        br#"{"version":"0.2.0"}"#,
    )
    .unwrap();
    assert_eq!(
        fx.run(&["status", "--json"]).json()["installedVersion"],
        "0.2.0"
    );
    // 単独 install の記録は状態フォルダーの install.json へ書き、旧記録は読むだけで書き換えも削除もしない。
    let out = fx.run(&["install"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let record: Value =
        serde_json::from_slice(&fs::read(home.join("state").join("install.json")).unwrap())
            .unwrap();
    assert_eq!(record["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        fs::read(home.join("app").join("current.json")).unwrap(),
        br#"{"version":"0.0.9"}"#
    );
    assert_eq!(
        fx.run(&["status", "--json"]).json()["installedVersion"],
        env!("CARGO_PKG_VERSION")
    );
}
