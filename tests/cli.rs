//! 利用者向け CLI（src/cli.rs・src/doctor.rs・connect の --remove）の契約テスト。
//!
//! 隔離: すべてのコマンドは子プロセスとして実行し、環境変数 `SCRIPTURA_HOME` / `SCRIPTURA_CONFIG` /
//! `SCRIPTURA_DB` / `CLAUDE_CONFIG_DIR` / `LOCALAPPDATA` を一時フォルダーへ向ける。実環境の設定・状態・
//! `~/.claude` は読み書きしない。`claude` / `gh` は既存のスタブ（本物の Claude は呼ばない）。
//! 環境変数は各子プロセスにだけ渡すので、テスト同士は干渉しない（プロセス全体の環境は変えない）。
//! fixture の置き場は `SCRIPTURA_TEST_BASE`（未指定なら既存テストと同じ `validation/`）。
#![cfg(feature = "test-support")]
use scriptura::config::{self, CURRENT_CONSENT_VERSION, Config, ConsentSource, Loaded, Mode};
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

    /// 既定のリポジトリで enable を実行する（同意つき・JSON 出力）。
    fn enable(&self, extra: &[&str]) -> Out {
        self.enable_repo(&self.repo, extra)
    }

    fn enable_repo(&self, repo: &Path, extra: &[&str]) -> Out {
        let version = CURRENT_CONSENT_VERSION.to_string();
        let mut args = vec![
            "enable",
            "--repo",
            repo.to_str().unwrap(),
            "--accept-consent",
            &version,
            "--json",
        ];
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
fn enable_without_matching_consent_is_refused_and_writes_nothing() {
    let fx = Fx::new();
    // 同意なし。
    let out = fx.run(&["enable", "--repo", fx.repo.to_str().unwrap(), "--json"]);
    assert_eq!(out.code, 1);
    let v = out.json();
    assert_eq!(v["ok"], false);
    let message = v["error"].as_str().unwrap();
    assert!(message.contains("--accept-consent"), "{message}");
    assert!(
        message.contains("Anthropic") && message.contains("料金"),
        "{message}"
    );
    // 版が違う同意。
    let out = fx.run(&[
        "enable",
        "--repo",
        fx.repo.to_str().unwrap(),
        "--accept-consent",
        "999",
        "--json",
    ]);
    assert_eq!(out.code, 1);
    assert!(!fx.config_path().exists());
    assert!(!fx.state_db().exists());
    // --json なしは標準エラーへ日本語で出す。
    let out = fx.run(&["enable", "--repo", fx.repo.to_str().unwrap()]);
    assert_eq!(out.code, 1);
    assert!(out.stdout.is_empty() && out.stderr.contains("--accept-consent"));
}

#[test]
fn enable_defaults_to_propose_and_creates_a_safe_config() {
    let fx = Fx::new();
    let out = fx.enable(&[]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let v = out.json();
    assert_eq!(v["ok"], true);
    assert_eq!(v["mode"], "propose");
    assert_eq!(v["paused"], false);
    assert_eq!(v["branches"], json!([]));
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
    let consent = config.consent.clone().unwrap();
    assert_eq!(consent.version, CURRENT_CONSENT_VERSION);
    assert_eq!(consent.source, ConsentSource::Cli);
    assert_eq!(config.repository.len(), 1);
    assert_eq!(config.repository[0].mode, Some(Mode::Propose));
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
    seed.consent = Some(config::Consent {
        version: CURRENT_CONSENT_VERSION,
        accepted_at: "2026-01-01T00:00:00Z".into(),
        source: ConsentSource::Cli,
    });
    seed.worker.timeout_secs = 300;
    seed.defaults.exclude = vec!["private/**".into()];
    seed.repository.push(config::Repository {
        root: other.to_string_lossy().into_owned(),
        remote: None,
        mode: Some(Mode::Push),
        branches: vec!["release".into()],
        documents: None,
        exclude: vec![],
        paused: true,
    });
    config::save_to(&fx.config_path(), &seed).unwrap();
    let out = fx.enable(&["--documents", "README.md,agents.md"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let config = fx.config();
    assert_eq!(config.worker.timeout_secs, 300);
    assert_eq!(config.defaults.exclude, vec!["private/**".to_string()]);
    // 既存の同意（accepted_at）は上書きしない。
    assert_eq!(config.consent.unwrap().accepted_at, "2026-01-01T00:00:00Z");
    assert_eq!(config.repository.len(), 2);
    assert_eq!(config.repository[0].root, other.to_string_lossy());
    assert!(config.repository[0].paused);
    assert_eq!(config.repository[0].branches, vec!["release".to_string()]);
    assert_eq!(
        config.repository[1].documents,
        Some(vec![config::Document::Readme, config::Document::Agents])
    );
    // 再実行は同じ登録を更新する（重複しない）。一時ファイルも残らない。
    assert_eq!(fx.enable(&["--mode", "off"]).code, 0);
    let config = fx.config();
    assert_eq!(config.repository.len(), 2);
    assert_eq!(config.repository[1].mode, Some(Mode::Off));
    let leftovers: Vec<_> = fs::read_dir(&fx.base)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    // 未対応の文書名は拒否する。
    let out = fx.enable(&["--documents", "CHANGELOG.md"]);
    assert_eq!(out.code, 1);
    assert!(
        out.json()["error"]
            .as_str()
            .unwrap()
            .contains("CHANGELOG.md")
    );
}

#[test]
fn push_mode_is_explicit_and_defaults_to_the_current_branch() {
    let fx = Fx::new();
    assert_eq!(fx.enable(&[]).json()["mode"], "propose");
    let out = fx.enable(&["--mode", "push"]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let v = out.json();
    assert_eq!(v["mode"], "push");
    assert_eq!(v["branches"], json!(["main"]));
    let repo = &fx.config().repository[0];
    assert_eq!(repo.mode, Some(Mode::Push));
    assert_eq!(repo.branches, vec!["main".to_string()]);
    // 明示したブランチはそのまま使う。
    let v = fx
        .enable(&["--mode", "push", "--branches", "main, release/1"])
        .json();
    assert_eq!(v["branches"], json!(["main", "release/1"]));
    // 不正なモードは拒否。
    assert_eq!(fx.enable(&["--mode", "force"]).code, 1);
}

#[test]
fn push_mode_without_remote_or_upstream_is_refused() {
    let fx = Fx::new();
    // remote が無い。
    let bare_less = fx.base.join("no-remote");
    fx.make_repo(&bare_less);
    let out = fx.enable_repo(&bare_less, &["--mode", "push"]);
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
    let out = fx.enable_repo(&no_upstream, &["--mode", "push"]);
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("upstream"));
    // --branches を明示しても upstream は必須。
    let out = fx.enable_repo(&no_upstream, &["--mode", "push", "--branches", "main"]);
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("upstream"));
    assert!(!fx.config_path().exists(), "拒否したときは設定を作らない");
    // propose なら remote が無くても登録できる（remote は記録しない）。
    let out = fx.enable_repo(&bare_less, &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(fx.config().repository[0].remote.is_none());
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
    // 同意は残る（登録だけ消える）。
    assert!(fx.config().consent.is_some());
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
    assert_eq!(v["consent"], Value::Null);
    assert_eq!(v["proposals"], json!({"pending": 0, "applied": 0}));
    assert!(v["version"].is_string() && v["installed"].is_boolean());

    assert_eq!(fx.enable(&["--branches", "main"]).code, 0);
    let v = fx.run(&["status", "--json"]).json();
    assert_eq!(v["config"]["state"], "ok");
    assert!(same_path(
        v["config"]["path"].as_str().unwrap(),
        fx.config_path()
    ));
    assert_eq!(v["consent"]["version"], CURRENT_CONSENT_VERSION);
    assert_eq!(v["consent"]["source"], "cli");
    assert_eq!(v["consent"]["current"], true);
    assert_eq!(v["policy"], "explicit");
    assert_eq!(v["defaults"]["mode"], "propose");
    let repo = &v["repositories"][0];
    assert_eq!(repo["mode"], "propose");
    assert_eq!(repo["paused"], false);
    assert_eq!(repo["branches"], json!(["main"]));
    assert_eq!(repo["proposals"], json!({"pending": 0, "applied": 0}));
    for key in [
        "model",
        "effort",
        "timeoutSecs",
        "maxBudgetUsd",
        "permission",
        "claudePath",
    ] {
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
    assert_eq!(item(&v, "consent")["status"], "ok");
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
    // 同意の版が古い。
    let mut config = Config::default();
    config.consent = Some(config::Consent {
        version: 0,
        accepted_at: "2026-01-01T00:00:00Z".into(),
        source: ConsentSource::Cli,
    });
    config::save_to(&fx.config_path(), &config).unwrap();
    let v = fx.run(&["doctor", "--json"]).json();
    assert_eq!(item(&v, "consent")["status"], "warn");
}

#[test]
fn doctor_warns_about_duplicate_registration_policy_and_legacy_hooks() {
    let fx = Fx::new();
    // 単独登録だけ・プラグインだけでは警告しない。
    fx.write_settings(&json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "\"C:/x/Scriptura/bin/scriptura.exe\" hook"}]}]}}));
    let v = fx.run(&["doctor", "--json"]).json();
    assert_eq!(item(&v, "hookDuplicate")["status"], "ok");
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
        (json!(false), json!(true), json!(1))
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
// run-now / review / apply
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
    fx.make_repo(&other);
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
    // 同意が古い設定も拒否（enable では版を満たすので、設定を直接書き換える）。
    assert_eq!(
        fx.run(&["resume", "--repo", fx.repo.to_str().unwrap()])
            .code,
        0
    );
    let mut config = fx.config();
    config.consent.as_mut().unwrap().version = 0;
    config::save_to(&fx.config_path(), &config).unwrap();
    let out = fx.exec(&["run-now", "--json"], &[], None, Some(&fx.repo));
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("同意"));
}

#[test]
fn run_now_creates_a_proposal_that_review_and_apply_delegate_to() {
    let fx = Fx::new();
    assert_eq!(fx.enable(&[]).code, 0);
    fx.push_change("a change worth documenting");
    let readme = fx.repo.join("README.md");
    let writes = json!({"README.md": "updated readme\n"}).to_string();

    // review: 更新案が無い間は空の一覧（失敗ではない）。
    let out = fx.exec(&["review", "--json"], &[], None, Some(&fx.repo));
    assert_eq!(out.code, 0);
    assert_eq!(out.json()["proposals"], json!([]));

    let out = fx.exec(
        &["run-now", "--json"],
        &[("HOOK_STUB_WRITES", &writes)],
        None,
        Some(&fx.repo),
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let v = out.json();
    assert_eq!(v["outcome"], "created");
    let sha = v["sha"].as_str().unwrap().to_string();
    assert!(Path::new(v["patchPath"].as_str().unwrap()).is_file());
    assert_eq!(
        fs::read_to_string(&readme).unwrap(),
        "readme",
        "propose は作業ツリーを変えない"
    );

    // review は diffstat・パッチのパス・対象文書を返す。
    let v = fx
        .exec(&["review", "--json"], &[], None, Some(&fx.repo))
        .json();
    assert_eq!(v["sha"], sha);
    assert_eq!(v["documents"], json!(["README.md"]));
    assert!(v["diffstat"].as_str().unwrap().contains("README.md"));
    assert!(Path::new(v["patchPath"].as_str().unwrap()).is_file());
    assert_eq!(v["proposals"].as_array().unwrap().len(), 1);
    // sha の前方指定でも選べる。
    let v = fx
        .exec(&["review", &sha[..8], "--json"], &[], None, Some(&fx.repo))
        .json();
    assert_eq!(v["sha"], sha);
    // status の件数にも反映される。
    assert_eq!(
        fx.run(&["status", "--json"]).json()["proposals"]["pending"],
        1
    );

    // 対象文書に未コミットの変更があれば apply は拒否し、理由をそのまま出す。
    fs::write(&readme, "local edit").unwrap();
    let out = fx.exec(&["apply", "--json"], &[], None, Some(&fx.repo));
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("未コミット"));
    assert_eq!(fs::read_to_string(&readme).unwrap(), "local edit");
    fx.git(&["checkout", "--", "README.md"]);

    // 適用。stage はしない。
    let out = fx.exec(&["apply", "--json"], &[], None, Some(&fx.repo));
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let v = out.json();
    assert_eq!(v["files"], json!(["README.md"]));
    assert_eq!(v["staged"], false);
    assert_eq!(
        fs::read_to_string(&readme).unwrap().replace("\r\n", "\n"),
        "updated readme\n"
    );
    assert!(
        fx.git(&["diff", "--cached", "--name-only"])
            .trim()
            .is_empty()
    );
    // 適用済みを再適用しようとすると拒否。
    assert_eq!(
        fx.exec(&["apply", "--json"], &[], None, Some(&fx.repo))
            .code,
        1
    );
    assert_eq!(
        fx.run(&["status", "--json"]).json()["proposals"],
        json!({"pending": 0, "applied": 1})
    );
}

#[test]
fn discard_removes_a_proposal_and_unknown_selectors_are_refused() {
    let fx = Fx::new();
    assert_eq!(fx.enable(&[]).code, 0);
    fx.push_change("another change");
    let writes = json!({"README.md": "discard me\n"}).to_string();
    let out = fx.exec(
        &["run-now", "--json"],
        &[("HOOK_STUB_WRITES", &writes)],
        None,
        Some(&fx.repo),
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert_eq!(
        fx.exec(&["review", "zzzz", "--json"], &[], None, Some(&fx.repo))
            .code,
        1
    );
    let out = fx.exec(&["discard", "--json"], &[], None, Some(&fx.repo));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.json()["discarded"], true);
    assert_eq!(
        fx.run(&["status", "--json"]).json()["proposals"]["pending"],
        0
    );
    // 余分な引数・不明なオプションは JSON のエラーで返る。
    let out = fx.run(&["review", "a", "b", "--json"]);
    assert_eq!(out.code, 1);
    assert_eq!(out.json()["ok"], false);
    assert_eq!(fx.run(&["status", "--bogus"]).code, 1);
}
