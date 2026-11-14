#![cfg(feature = "test-support")]
use rusqlite::Connection;
use scriptura::config::{self, Config, Document, Mode, Permission, Policy, Repository, Worker};
use scriptura::maintenance::*;
use serde_json::{Value, json};
use std::{
    env, fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Mutex, MutexGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
static ENV: Mutex<()> = Mutex::new(());
#[test]
fn uncertain_commit_or_push_from_stop_is_never_automatically_repeated() {
    for operation in ["commit", "push"] {
        let f = Fixture::new();
        handle_hook(&json!({"hook_event_name":"UserPromptSubmit","cwd":f.root})).unwrap();
        fs::write(f.root.join("source.txt"), "new push").unwrap();
        f.git(&["add", "source.txt"]);
        f.git(&["commit", "-m", "new"]);
        f.git(&["push"]);
        f.stub_git();
        f.helper_environment();
        f.writes(json!({f.root.join("AGENTS.md").to_str().unwrap():"maintained"}));
        unsafe {
            env::set_var("HOOK_GIT_UNCERTAIN_OPERATION", operation);
            env::set_var("SCRIPTURA_TEST_GIT_TIMEOUT_MS", "1500")
        };
        let cursor_before = {
            let db = open_state_db().unwrap();
            db.query_row(
                "SELECT fingerprint,pushed_commit FROM poll_cursors",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .unwrap()
        };
        let message = handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root}))
            .unwrap()
            .unwrap();
        assert!(message.contains(operation) && message.contains("未確定"));
        let state = f.state();
        assert_eq!(state["phase"], "committing");
        assert!(state["index"].as_str().is_some_and(|s| !s.is_empty()));
        assert_ne!(state["before"]["AGENTS.md"], state["after"]["AGENTS.md"]);
        if operation == "push" {
            assert!(!state["commit"].as_str().unwrap().is_empty());
        }
        let head = f.git(&["rev-parse", "HEAD"]);
        let remote = f.git(&["rev-parse", "origin/main"]);
        if operation == "push" {
            assert_eq!(head, remote)
        } else {
            assert_ne!(head, remote)
        }
        let cursor_after = {
            let db = open_state_db().unwrap();
            db.query_row(
                "SELECT fingerprint,pushed_commit,error FROM poll_cursors",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .unwrap()
        };
        assert_eq!(
            (&cursor_after.0, &cursor_after.1),
            (&cursor_before.0, &cursor_before.1)
        );
        assert!(cursor_after.2.is_some());
        let _ = handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root})).unwrap();
        assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
        assert_eq!(f.git(&["rev-parse", "origin/main"]), remote);
        assert_eq!(f.state()["phase"], "committing");
        assert_eq!(f.state()["error"], state["error"]);
        f.finish_live_helper();
    }
}
#[test]
fn github_url_path_is_not_normalized_before_guard() {
    assert_eq!(
        github_repository("https://github.com/owner/repo.git?query=/x#fragment").unwrap(),
        Some("owner/repo".into())
    );
    for url in [
        "https://github.com/owner/x/../repo",
        "https://github.com/owner/%2e%2e/repo",
        "https://github.com?next=/owner/repo",
    ] {
        assert!(github_repository(url).is_err(), "{url}");
    }
}
#[test]
fn graphql_falsey_errors_match_python_without_skipping_shape_checks() {
    let f = Fixture::new();
    f.git(&[
        "remote",
        "set-url",
        "--push",
        "origin",
        "https://github.com/owner/repo.git",
    ]);
    for value in [
        Value::Null,
        json!(false),
        json!(0),
        json!(""),
        json!([]),
        json!({}),
    ] {
        let mut response = json!({"data":{"repository":{"ref":{"associatedPullRequests":{"nodes":[],"pageInfo":{"hasNextPage":false}}}}}});
        response["errors"] = value;
        unsafe { env::set_var("HOOK_STUB_GH_JSON", response.to_string()) };
        assert_eq!(
            external_pr_error(&f.root, "origin", "refs/heads/main"),
            None
        );
    }
    unsafe {
        env::set_var(
            "HOOK_STUB_GH_JSON",
            json!({"errors":[],"data":{}}).to_string(),
        )
    };
    assert!(
        external_pr_error(&f.root, "origin", "refs/heads/main")
            .unwrap()
            .contains("不明")
    );
}
#[test]
fn null_stdio_with_inherited_windows_handles_still_has_deadline() {
    let f = Fixture::new();
    f.helper_environment();
    let c = scriptura::process::command(f.base.join("bin/null-parent.exe").to_str().unwrap());
    let result = scriptura::process::capture(c, None, Duration::from_millis(300), false);
    assert!(result.err().unwrap().to_string().contains("unconfirmed"));
    f.finish_live_helper();
}
#[test]
fn logical_parent_cwd_is_rejected_before_worker() {
    let f = Fixture::new();
    handle_hook(&json!({"hook_event_name":"UserPromptSubmit","cwd":f.root})).unwrap();
    fs::create_dir(f.root.join("child")).unwrap();
    fs::write(f.root.join("source.txt"), "new").unwrap();
    f.git(&["add", "source.txt"]);
    f.git(&["commit", "-m", "new"]);
    f.git(&["push"]);
    let cwd = f.root.join("child/..");
    assert!(
        handle_hook(&json!({"hook_event_name":"Stop","cwd":cwd}))
            .unwrap()
            .unwrap()
            .contains("リポジトリ外")
    );
    assert!(!f.base.join("prompt.json").exists());
}
#[test]
fn head_change_during_worker_preserves_uncommitted_document() {
    let f = Fixture::new();
    unsafe { env::set_var("HOOK_STUB_HEAD_CHANGE", "1") };
    f.writes(json!({f.root.join("AGENTS.md").to_str().unwrap():"worker change"}));
    assert!(f.maintain().unwrap().contains("HEAD"));
    assert_eq!(
        f.git(&["show", "--format=", "--name-only", "HEAD"]),
        "source.txt\n"
    );
    assert!(
        f.git(&["status", "--porcelain", "--", "AGENTS.md"])
            .contains("AGENTS.md")
    );
}
#[test]
fn tree_case_sensitive_chain_cannot_be_hidden_by_other_case_override() {
    let f = Fixture::new();
    let write_blob = |data: &[u8]| {
        let mut c = Command::new("git")
            .arg("-C")
            .arg(&f.root)
            .args(["hash-object", "-w", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(data).unwrap();
        String::from_utf8(c.wait_with_output().unwrap().stdout)
            .unwrap()
            .trim()
            .to_string()
    };
    for (name, data) in [
        ("AGENTS.md", vec![b'a'; 16000]),
        ("a/AGENTS.md", vec![b'b'; 14000]),
        ("A/AGENTS.override.md", b" ".to_vec()),
    ] {
        let oid = write_blob(&data);
        f.git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{oid},{name}"),
        ]);
    }
    f.git(&["commit", "-m", "case distinct instructions"]);
    assert!(
        agents_tree_size_error(&f.root, "HEAD", &[])
            .unwrap()
            .contains("chain")
    );
}
#[test]
fn fetch_without_push_does_not_trigger_worker() {
    let f = Fixture::new();
    handle_hook(&json!({"hook_event_name":"UserPromptSubmit","cwd":f.root})).unwrap();
    f.git(&["fetch", "origin"]);
    assert_eq!(
        handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root})).unwrap(),
        None
    );
    assert!(!f.base.join("prompt.json").exists());
}
struct Fixture {
    _guard: MutexGuard<'static, ()>,
    base: PathBuf,
    root: PathBuf,
    remote: PathBuf,
    old_path: std::ffi::OsString,
}
impl Fixture {
    /// 旧動作相当の設定（policy=all・push・bypass・同意あり・3 文書）を置いた環境。
    fn new() -> Self {
        Self::with_config(Some(Config::legacy_all_push()))
    }
    /// 利用者設定を `SCRIPTURA_CONFIG` の一時ファイルへ置く（None なら設定ファイル無し）。
    fn with_config(config: Option<Config>) -> Self {
        let guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        // 置き場は `SCRIPTURA_TEST_BASE` で変えられる（未指定なら従来どおり `validation/`）。
        let parent = env::var_os("SCRIPTURA_TEST_BASE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("validation"));
        let base = parent.join(format!(
            "maintenance-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        let root = base.join("repo");
        let remote = base.join("remote.git");
        let bin = base.join("bin");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let real = String::from_utf8(
            Command::new("where.exe")
                .arg("git.exe")
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();
        for name in [
            "claude.exe",
            "gh.exe",
            "pipe-parent.exe",
            "pipe-grandchild.exe",
            "managed-host.exe",
            "detached-parent.exe",
            "null-parent.exe",
            "filter.exe",
        ] {
            fs::copy(env!("CARGO_BIN_EXE_hook-test-stub"), bin.join(name)).unwrap();
        }
        let old_path = env::var_os("PATH").unwrap_or_default();
        unsafe {
            env::set_var(
                "PATH",
                format!("{};{}", bin.display(), old_path.to_string_lossy()),
            );
            env::set_var("SCRIPTURA_DB", base.join("state/state.sqlite3"));
            env::set_var("HOOK_STUB_PROMPT", base.join("prompt.json"));
            env::set_var("SCRIPTURA_CONFIG", base.join("config.toml"));
            env::set_var("HOOK_REAL_GIT", real);
            for key in [
                "HOOK_STUB_WRITES",
                "HOOK_STUB_EXIT",
                "HOOK_STUB_SLEEP",
                "HOOK_STUB_GH_JSON",
                "HOOK_STUB_GH_EXIT",
                "HOOK_STUB_GITHUB_AFTER_WORKER",
                "HOOK_PARENT_SLEEP",
                "HOOK_GIT_FAIL_PUSH",
                "HOOK_GIT_FAIL_COMMIT",
                "HOOK_STUB_GH_JSON_FILE",
                "HOOK_STUB_GH_BLOCK_AFTER",
                "HOOK_CHILD_EXIT",
                "HOOK_STUB_HEAD_CHANGE",
                "HOOK_GIT_UNCERTAIN_OPERATION",
                "SCRIPTURA_TEST_GIT_TIMEOUT_MS",
                "SCRIPTURA_HOME",
            ] {
                env::remove_var(key);
            }
        }
        let f = Self {
            _guard: guard,
            base,
            root,
            remote,
            old_path,
        };
        f.git(&["init", "-b", "main"]);
        f.git(&["config", "user.email", "test@example.invalid"]);
        f.git(&["config", "user.name", "Hook fixture"]);
        f.git(&["config", "core.autocrlf", "false"]);
        fs::write(f.root.join("source.txt"), "initial").unwrap();
        fs::write(f.root.join("AGENTS.md"), "instructions").unwrap();
        fs::write(f.root.join("DESIGN.md"), "design").unwrap();
        fs::write(f.root.join("README.md"), "readme").unwrap();
        f.git(&["add", "."]);
        f.git(&["commit", "-m", "initial"]);
        assert!(
            Command::new("git")
                .args(["init", "--bare"])
                .arg(&f.remote)
                .output()
                .unwrap()
                .status
                .success()
        );
        f.git(&["remote", "add", "origin", f.remote.to_str().unwrap()]);
        f.git(&["push", "-u", "origin", "main"]);
        if let Some(config) = config {
            f.set_config(&config);
        }
        f
    }
    fn config_path(&self) -> PathBuf {
        self.base.join("config.toml")
    }
    fn set_config(&self, config: &Config) {
        config::save_to(&self.config_path(), config).unwrap();
    }
    /// 登録リポジトリ 1 件だけを対象にする設定（policy=explicit）。
    fn registered_config(&self, edit: impl FnOnce(&mut Repository)) -> Config {
        let mut config = Config::legacy_all_push();
        config.enrollment.policy = Policy::Explicit;
        let mut repo = Repository {
            root: self.root.to_string_lossy().into_owned(),
            remote: None,
            mode: None,
            branches: vec![],
            documents: None,
            exclude: vec![],
            paused: false,
        };
        edit(&mut repo);
        config.repository.push(repo);
        config
    }
    /// source.txt を変更して commit・push する（新しい push を 1 つ作る）。
    fn push_change(&self, body: &str) {
        fs::write(self.root.join("source.txt"), body).unwrap();
        self.git(&["add", "source.txt"]);
        self.git(&["commit", "-m", body]);
        self.git(&["push"]);
    }
    fn hook(&self, event: &str) -> Option<String> {
        handle_hook(&json!({"hook_event_name":event,"cwd":self.root})).unwrap()
    }
    /// poll_cursors の (fingerprint, pushed_commit)。DB が無ければ None（作成しない）。
    fn cursor(&self) -> Option<(String, String)> {
        let path = state_db_path();
        if !path.exists() {
            return None;
        }
        let db = Connection::open(path).unwrap();
        db.query_row(
            "SELECT fingerprint,pushed_commit FROM poll_cursors",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
    }
    fn prompt_record(&self) -> Value {
        serde_json::from_slice(&fs::read(self.base.join("prompt.json")).unwrap()).unwrap()
    }
    fn git(&self, args: &[&str]) -> String {
        let r = Command::new("git")
            .arg("-C")
            .arg(&self.root)
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
    fn writes(&self, value: Value) {
        unsafe { env::set_var("HOOK_STUB_WRITES", value.to_string()) }
    }
    fn stub_git(&self) {
        fs::copy(
            env!("CARGO_BIN_EXE_hook-test-stub"),
            self.base.join("bin/git.exe"),
        )
        .unwrap();
    }
    fn helper_environment(&self) {
        unsafe {
            env::set_var("HOOK_CHILD_PID", self.base.join("child.pid"));
            env::set_var("HOOK_CHILD_EXIT", self.base.join("child-exit"));
        }
    }
    fn finish_live_helper(&self) {
        let pid = fs::read_to_string(self.base.join("child.pid")).unwrap();
        let r=Command::new("pwsh").args(["-NoProfile","-Command",&format!("$p=Get-Process -Id {pid} -ErrorAction SilentlyContinue;if(-not $p){{exit 1}};exit 0")]).output().unwrap();
        fs::write(self.base.join("child-exit"), "exit").unwrap();
        assert!(r.status.success(), "helper was killed");
        let r=Command::new("pwsh").args(["-NoProfile","-Command",&format!("$p=Get-Process -Id {pid} -ErrorAction SilentlyContinue;if($p){{$p.WaitForExit(5000);if(-not $p.HasExited){{exit 1}}}};exit 0")]).output().unwrap();
        assert!(r.status.success(), "fixture helper did not exit");
    }
    fn docs(&self) -> Vec<PathBuf> {
        ["AGENTS.md", "DESIGN.md", "README.md"]
            .map(|n| self.root.join(n))
            .to_vec()
    }
    fn state(&self) -> Value {
        serde_json::from_slice(&fs::read(publication_paths(&self.root).0).unwrap()).unwrap()
    }
    fn save(&self, state: &mut Value) {
        let p = publication_paths(&self.root).0;
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        save_publication(&p, state).unwrap();
    }
    fn maintain(&self) -> Option<String> {
        maintain_and_publish(
            &self.root,
            "fixture reason",
            &self.docs(),
            Some("fixture"),
            &[self.root.clone()],
        )
    }
    fn checkpoint(&self, phase: &str) -> Value {
        let head = self.git(&["rev-parse", "HEAD"]).trim().to_owned();
        let mut before = serde_json::Map::new();
        for name in ["AGENTS.md", "DESIGN.md", "README.md"] {
            before.insert(name.into(), json!(document_hash(&self.root, name).unwrap()));
        }
        json!({"phase":phase,"head":head,"branch":"main","remote":"origin","ref":"refs/heads/main","destination":hash(self.git(&["remote","get-url","--push","origin"]).as_bytes()),"before":before,"after":before,"history":[]})
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if env::var_os("HOOK_CHILD_EXIT").is_some() {
            let _ = fs::write(self.base.join("child-exit"), "exit");
        }
        unsafe {
            env::set_var("PATH", &self.old_path);
            for k in [
                "SCRIPTURA_DB",
                "SCRIPTURA_CONFIG",
                "HOOK_STUB_SLEEP",
                "HOOK_STUB_PROMPT",
                "HOOK_REAL_GIT",
                "HOOK_STUB_WRITES",
                "HOOK_STUB_EXIT",
                "HOOK_STUB_GH_JSON",
                "HOOK_STUB_GH_EXIT",
                "HOOK_STUB_GITHUB_AFTER_WORKER",
                "HOOK_CHILD_PID",
                "HOOK_PARENT_SLEEP",
                "HOOK_GIT_FAIL_PUSH",
                "HOOK_GIT_FAIL_COMMIT",
                "HOOK_STUB_GH_JSON_FILE",
                "HOOK_STUB_GH_BLOCK_AFTER",
                "HOOK_CHILD_EXIT",
                "HOOK_STUB_HEAD_CHANGE",
                "HOOK_GIT_UNCERTAIN_OPERATION",
                "SCRIPTURA_TEST_GIT_TIMEOUT_MS",
            ] {
                env::remove_var(k);
            }
        }
        eprintln!("cleanup fixture: {}", self.base.display());
    }
}
#[test]
fn github_url_forms_and_invalid_targets() {
    for u in [
        "git@github.com:Owner/Repo.git",
        "https://secret@github.com/Owner/Repo.git",
        "ssh://git@github.com/Owner/Repo",
    ] {
        assert_eq!(github_repository(u).unwrap(), Some("Owner/Repo".into()))
    }
    assert_eq!(github_repository("https://example.org/a/b").unwrap(), None);
    assert!(github_repository("https://github.com/a/b/c").is_err());
}
#[test]
fn python_normcase_and_publication_key() {
    let f = Fixture::new();
    assert_eq!(normcase("D:/Dev/İΟΣ"), "d:\\dev\\İοσ");
    assert_eq!(
        hash(normcase("D:\\Dev\\skills")),
        "05629082bd22e81e1dcbdf1da39fdfca9c45b422d7afcb1c905b21e88fa81fea"
    );
    assert!(
        publication_paths(&f.root)
            .0
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".json")
    );
}
#[test]
fn instruction_whitespace_matches_rust_trim() {
    for data in [b"".as_slice(), "\u{85}\u{a0}\u{3000}\r\n".as_bytes()] {
        assert!(!instruction_has_content(data))
    }
    assert!(instruction_has_content(b"\xef\xbb\xbf"));
    assert!(instruction_has_content(b"\xff"));
}
#[test]
fn worker_nochange_and_argument_contract() {
    let f = Fixture::new();
    assert_eq!(f.maintain(), None);
    assert_eq!(f.state()["phase"], "complete");
    let p: Value = serde_json::from_slice(&fs::read(f.base.join("prompt.json")).unwrap()).unwrap();
    let args = p["args"].as_array().unwrap();
    for arg in [
        "-p",
        "claude-sonnet-5-5",
        "--effort",
        "high",
        "--dangerously-skip-permissions",
        "--no-session-persistence",
    ] {
        assert!(args.iter().any(|a| a == arg));
    }
    assert!(!args.iter().any(|arg| {
        arg.as_str()
            .is_some_and(|arg| arg.contains("tool_output_token_limit"))
    }));
    assert_eq!(p["worker_env"], "1");
}
#[test]
fn worker_drops_inherited_desktop_session_env() {
    // デスクトップのセッションから起動された場合のホスト用環境変数は worker に渡さない。
    let f = Fixture::new();
    let keys = [
        "CLAUDECODE",
        "CLAUDE_CODE_SIMPLE",
        "CLAUDE_CODE_ENTRYPOINT",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ];
    unsafe {
        for k in keys {
            env::set_var(k, "1");
        }
    }
    let result = f.maintain();
    unsafe {
        for k in keys {
            env::remove_var(k);
        }
    }
    assert_eq!(result, None);
    let p: Value = serde_json::from_slice(&fs::read(f.base.join("prompt.json")).unwrap()).unwrap();
    assert_eq!(p["inherited_host_env"], json!([]));
    assert_eq!(p["oauth_token_kept"], true);
}
#[test]
fn worker_changes_only_owned_document_and_preserves_other_index() {
    let f = Fixture::new();
    fs::write(f.root.join("source.txt"), "user change").unwrap();
    f.git(&["add", "source.txt"]);
    f.writes(json!({f.root.join("AGENTS.md").to_str().unwrap():"updated instructions"}));
    assert_eq!(f.maintain(), None);
    assert_eq!(f.git(&["diff", "--cached", "--name-only"]), "source.txt\n");
    assert_eq!(f.state()["phase"], "complete");
    assert_eq!(
        f.git(&["show", "--format=", "--name-only", "HEAD"]),
        "AGENTS.md\n"
    );
}
#[test]
fn worker_failure_retry_and_user_edit_protection() {
    let f = Fixture::new();
    f.writes(json!({f.root.join("AGENTS.md").to_str().unwrap():"worker partial"}));
    unsafe { env::set_var("HOOK_STUB_EXIT", "7") };
    assert!(f.maintain().unwrap().contains("失敗"));
    assert_eq!(f.state()["phase"], "worker_failed");
    fs::write(f.root.join("AGENTS.md"), "user edit after failure").unwrap();
    assert!(f.maintain().unwrap().contains("ユーザー差分"));
    unsafe { env::remove_var("HOOK_STUB_EXIT") };
    assert_eq!(
        fs::read_to_string(f.root.join("AGENTS.md")).unwrap(),
        "user edit after failure"
    );
}
#[test]
fn unchanged_failed_worker_can_retry() {
    let f = Fixture::new();
    unsafe { env::set_var("HOOK_STUB_EXIT", "3") };
    assert!(f.maintain().is_some());
    unsafe { env::remove_var("HOOK_STUB_EXIT") };
    assert_eq!(f.maintain(), None);
    assert_eq!(f.state()["phase"], "complete");
}
#[test]
fn dirty_documents_are_not_owned() {
    let f = Fixture::new();
    fs::write(f.root.join("AGENTS.md"), "existing user edit").unwrap();
    assert_eq!(f.maintain(), None);
    assert!(f.state()["before"].get("AGENTS.md").is_none());
    assert_eq!(
        fs::read_to_string(f.root.join("AGENTS.md")).unwrap(),
        "existing user edit"
    );
}
#[test]
fn hidden_index_flags_are_not_owned() {
    let f = Fixture::new();
    for flag in ["--assume-unchanged", "--skip-worktree"] {
        f.git(&["update-index", flag, "AGENTS.md"]);
        assert!(index_stage0(&f.root, "AGENTS.md").is_none());
        f.git(&[
            "update-index",
            "--no-assume-unchanged",
            "--no-skip-worktree",
            "AGENTS.md",
        ]);
    }
}
#[test]
fn ordinary_ignored_documents_are_owned_and_unignored() {
    let f = Fixture::new();
    f.git(&["rm", "--cached", "README.md"]);
    fs::write(f.root.join(".gitignore"), "README.md\n").unwrap();
    f.git(&["add", ".gitignore"]);
    f.git(&["commit", "-m", "ignored readme"]);
    f.git(&["push"]);
    f.writes(json!({f.root.join(".gitignore").to_str().unwrap():"README.md\n!/README.md\n"}));
    assert_eq!(f.maintain(), None);
    assert_eq!(f.state()["ignored_before"], json!(["README.md"]));
    assert!(f.git(&["ls-files", "README.md"]).contains("README.md"));
}
#[test]
fn lock_prevents_second_worker() {
    let f = Fixture::new();
    let (_, lockpath) = publication_paths(&f.root);
    fs::create_dir_all(lockpath.parent().unwrap()).unwrap();
    let lock = Connection::open(lockpath).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(f.maintain().unwrap().contains("実行中"));
    assert!(!f.base.join("prompt.json").exists());
}
#[test]
fn state_atomic_replace_and_history_limit() {
    let f = Fixture::new();
    let mut state = json!({"phase":"ready"});
    for i in 0..40 {
        state["error"] = json!(i);
        f.save(&mut state);
        assert_eq!(f.state(), state);
    }
    assert_eq!(state["history"].as_array().unwrap().len(), 32);
    f.save(&mut state);
    assert_eq!(state["history"].as_array().unwrap().len(), 32);
}
#[test]
fn unknown_or_changed_destination_blocks_old_phases() {
    let f = Fixture::new();
    for phase in [
        "worker_running",
        "interrupted_recovered",
        "ready",
        "worker_failed",
        "staged",
        "push_pending",
    ] {
        let mut state = f.checkpoint(phase);
        state["destination"] = json!("changed");
        if phase == "push_pending" {
            state["commit"] = state["head"].clone()
        }
        f.save(&mut state);
        assert!(f.maintain().unwrap().contains("送信先"), "{phase}");
    }
    assert!(!f.base.join("prompt.json").exists());
}
#[test]
fn python_ready_checkpoint_resumes_without_worker() {
    let f = Fixture::new();
    let mut state = f.checkpoint("ready");
    f.save(&mut state);
    assert_eq!(f.maintain(), None);
    assert_eq!(f.state()["phase"], "complete");
    assert!(!f.base.join("prompt.json").exists());
}
#[test]
fn python_worker_running_and_legacy_missing_fields_recovers() {
    let f = Fixture::new();
    let mut state = f.checkpoint("worker_running");
    f.save(&mut state);
    assert_eq!(f.maintain(), None);
    assert_eq!(f.state()["phase"], "complete");
}
#[test]
fn interrupted_ignored_checkpoints_are_fail_closed() {
    let f = Fixture::new();
    let mut state = f.checkpoint("worker_running");
    state["before"]["README.md"] = json!(hash("old"));
    state["after"]["README.md"] = json!(hash("old"));
    fs::write(f.root.join("README.md"), "changed").unwrap();
    f.git(&["rm", "--cached", "README.md"]);
    f.git(&["commit", "-m", "remove tracked readme"]);
    fs::write(f.root.join(".gitignore"), "README.md\n").unwrap();
    assert!(
        interrupted_ignored_error(&f.root, &state, &f.docs())
            .unwrap()
            .contains("ignored")
    );
    state["after"]["README.md"] = json!(document_hash(&f.root, "README.md").unwrap());
    assert_eq!(interrupted_ignored_error(&f.root, &state, &f.docs()), None);
}
#[test]
fn interrupted_legacy_disappearance_is_blocked() {
    let f = Fixture::new();
    let mut state = f.checkpoint("worker_running");
    f.git(&["rm", "README.md"]);
    assert!(
        interrupted_ignored_error(&f.root, &state, &f.docs())
            .unwrap()
            .contains("消失")
    );
    state["ignored_before"] = json!([]);
    assert_eq!(interrupted_ignored_error(&f.root, &state, &f.docs()), None);
}
#[test]
fn checkpoint_clean_proof_requires_raw_bytes_and_index() {
    let f = Fixture::new();
    let entry = index_stage0(&f.root, "README.md").unwrap();
    let digest = document_hash(&f.root, "README.md").unwrap();
    assert!(tracked_clean_bytes_proven(
        &f.root,
        "README.md",
        digest.as_deref(),
        &entry
    ));
    fs::write(f.root.join("README.md"), "different").unwrap();
    let digest = document_hash(&f.root, "README.md").unwrap();
    assert!(!tracked_clean_bytes_proven(
        &f.root,
        "README.md",
        digest.as_deref(),
        &entry
    ));
}
#[test]
fn individual_and_chain_size_boundaries() {
    let f = Fixture::new();
    fs::write(f.root.join("AGENTS.md"), vec![b'a'; 29999]).unwrap();
    assert_eq!(agents_size_error(&f.root, &[f.root.clone()], true), None);
    fs::write(f.root.join("AGENTS.md"), vec![b'a'; 30000]).unwrap();
    assert!(
        agents_size_error(&f.root, &[], true)
            .unwrap()
            .contains("サイズ超過")
    );
    fs::write(f.root.join("AGENTS.md"), vec![b'a'; 16000]).unwrap();
    fs::create_dir(f.root.join("child")).unwrap();
    fs::write(f.root.join("child/AGENTS.md"), vec![b'b'; 14000]).unwrap();
    assert!(
        agents_size_error(&f.root, &[f.root.join("child")], true)
            .unwrap()
            .contains("chain")
    );
    fs::write(f.root.join("child/AGENTS.override.md"), " \t\n").unwrap();
    assert_eq!(
        agents_size_error(&f.root, &[f.root.join("child")], true),
        None
    );
}
#[test]
fn siblings_are_not_summed_but_tracked_descendants_are_checked() {
    let f = Fixture::new();
    for d in ["one", "two"] {
        fs::create_dir(f.root.join(d)).unwrap();
        fs::write(f.root.join(d).join("AGENTS.md"), vec![b'x'; 20000]).unwrap();
    }
    f.git(&["add", "."]);
    assert_eq!(agents_size_error(&f.root, &[], true), None);
    fs::write(f.root.join("two/AGENTS.md"), vec![b'x'; 30000]).unwrap();
    assert!(agents_size_error(&f.root, &[], true).is_some());
}
#[test]
fn tree_checks_bytes_chain_and_empty_override() {
    let f = Fixture::new();
    fs::write(f.root.join("AGENTS.md"), vec![b'a'; 16000]).unwrap();
    fs::create_dir(f.root.join("child")).unwrap();
    fs::write(f.root.join("child/AGENTS.md"), vec![b'b'; 14000]).unwrap();
    f.git(&["add", "."]);
    f.git(&["commit", "-m", "oversize chain"]);
    assert!(
        agents_tree_size_error(&f.root, "HEAD", &[])
            .unwrap()
            .contains("chain")
    );
    fs::write(f.root.join("child/AGENTS.override.md"), "\u{3000}\n").unwrap();
    f.git(&["add", "."]);
    f.git(&["commit", "-m", "empty override"]);
    assert_eq!(agents_tree_size_error(&f.root, "HEAD", &[]), None);
}
#[test]
fn nested_repository_observed_path_blocks_worker() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("nested")).unwrap();
    f.git(&["init", "nested"]);
    assert!(
        agents_size_error(&f.root, &[f.root.join("nested")], false)
            .unwrap()
            .contains("別リポジトリ")
    );
}
#[test]
fn external_pr_and_unknown_status_block_before_worker() {
    let f = Fixture::new();
    f.git(&[
        "remote",
        "set-url",
        "--push",
        "origin",
        "https://github.com/owner/repo.git",
    ]);
    unsafe {
        env::set_var("HOOK_STUB_GH_JSON",json!({"data":{"repository":{"ref":{"associatedPullRequests":{"nodes":[{"isCrossRepository":true,"headRefName":"main","headRepository":{"nameWithOwner":"owner/repo"},"baseRepository":{"nameWithOwner":"other/repo"}}],"pageInfo":{"hasNextPage":false}}}}}}).to_string())
    };
    assert!(f.maintain().unwrap().contains("OPEN PR"));
    assert!(!f.base.join("prompt.json").exists());
    unsafe { env::set_var("HOOK_STUB_GH_JSON", "{}") };
    assert!(f.maintain().unwrap().contains("不明"));
}
#[test]
fn pr_pagination_and_other_heads_are_validated() {
    let f = Fixture::new();
    f.git(&[
        "remote",
        "set-url",
        "--push",
        "origin",
        "https://github.com/owner/repo.git",
    ]);
    unsafe {
        env::set_var("HOOK_STUB_GH_JSON",json!({"data":{"repository":{"ref":{"associatedPullRequests":{"nodes":[{"isCrossRepository":true,"headRefName":"different","headRepository":{"nameWithOwner":"owner/repo"},"baseRepository":{"nameWithOwner":"other/repo"}}],"pageInfo":{"hasNextPage":false}}}}}}).to_string())
    };
    assert_eq!(
        external_pr_error(&f.root, "origin", "refs/heads/main"),
        None
    );
    unsafe {
        env::set_var("HOOK_STUB_GH_JSON",json!({"data":{"repository":{"ref":{"associatedPullRequests":{"nodes":[],"pageInfo":{"hasNextPage":true,"endCursor":"same"}}}}}}).to_string())
    };
    assert!(
        external_pr_error(&f.root, "origin", "refs/heads/main")
            .unwrap()
            .contains("不明")
    );
}
#[test]
fn existing_python_db_schema_is_migrated() {
    let f = Fixture::new();
    let path = state_db_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE poll_cursors(repo_root TEXT NOT NULL,upstream TEXT NOT NULL,destination TEXT NOT NULL,fingerprint TEXT NOT NULL,updated_at REAL NOT NULL,error TEXT,PRIMARY KEY(repo_root,upstream,destination));").unwrap();
    drop(db);
    let db = open_state_db().unwrap();
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM pragma_table_info('poll_cursors') WHERE name='pushed_commit'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert!(f.root.exists());
}
#[test]
fn hook_initialization_and_stop_process_local_push() {
    let f = Fixture::new();
    assert_eq!(
        handle_hook(&json!({"hook_event_name":"UserPromptSubmit","cwd":f.root})).unwrap(),
        None
    );
    fs::write(f.root.join("source.txt"), "new change").unwrap();
    f.git(&["add", "source.txt"]);
    f.git(&["commit", "-m", "new pushed source"]);
    f.git(&["push"]);
    f.writes(json!({f.root.join("README.md").to_str().unwrap():"documented change"}));
    assert_eq!(
        handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root})).unwrap(),
        None
    );
    assert_eq!(f.state()["phase"], "complete");
    let count = f.git(&["rev-list", "--count", "HEAD"]);
    assert_eq!(count.trim(), "3");
    assert_eq!(
        handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root})).unwrap(),
        None
    );
    assert_eq!(f.git(&["rev-list", "--count", "HEAD"]), count);
}
#[test]
fn recursive_and_invalid_hook_payloads_do_nothing() {
    let f = Fixture::new();
    for p in [
        json!({}),
        json!({"hook_event_name":"Other","cwd":f.root}),
        json!({"hook_event_name":"Stop","cwd":f.root,"stop_hook_active":true}),
    ] {
        assert_eq!(handle_hook(&p).unwrap(), None);
    }
    assert!(!state_db_path().exists());
}
#[test]
fn malformed_payload_binary_is_fail_open_without_secrets() {
    let f = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_scriptura"))
        .arg("hook")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"{broken").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let p: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(p["systemMessage"].as_str().unwrap().contains("内部エラー"));
    assert!(f.root.exists());
}
#[test]
fn managed_job_closes_inherited_descendant_pipes() {
    let f = Fixture::new();
    unsafe {
        env::set_var("HOOK_CHILD_PID", f.base.join("child.pid"));
    }
    let mut c = scriptura::process::command(f.base.join("bin/pipe-parent.exe").to_str().unwrap());
    c.current_dir(&f.root);
    let r = scriptura::process::capture(c, None, Duration::from_secs(5), true).unwrap();
    assert_eq!(r.code, Some(0));
    assert!(r.stderr.len() <= 8192);
}
#[test]
fn managed_job_timeout_kills_descendants() {
    let f = Fixture::new();
    unsafe {
        env::set_var("HOOK_CHILD_PID", f.base.join("child.pid"));
        env::set_var("HOOK_PARENT_SLEEP", "1");
    }
    let c = scriptura::process::command(f.base.join("bin/pipe-parent.exe").to_str().unwrap());
    let r = scriptura::process::capture(c, None, Duration::from_millis(300), true);
    assert_eq!(r.err().unwrap().kind(), std::io::ErrorKind::TimedOut);
}

#[test]
fn commit_failure_recovers_owned_index() {
    let f = Fixture::new();
    f.stub_git();
    f.writes(json!({f.root.join("AGENTS.md").to_str().unwrap():"changed"}));
    unsafe { env::set_var("HOOK_GIT_FAIL_COMMIT", "1") };
    assert!(f.maintain().unwrap().contains("commit"));
    assert_eq!(f.state()["phase"], "staged");
    let index = f.git(&["write-tree"]);
    assert_eq!(f.state()["index"], index);
    unsafe { env::remove_var("HOOK_GIT_FAIL_COMMIT") };
    assert_eq!(f.maintain(), None);
    assert_eq!(f.state()["phase"], "complete");
}
#[test]
fn push_failure_retains_commit_and_poll_retries_without_worker() {
    let f = Fixture::new();
    handle_hook(&json!({"hook_event_name":"UserPromptSubmit","cwd":f.root})).unwrap();
    fs::write(f.root.join("source.txt"), "new").unwrap();
    f.git(&["add", "source.txt"]);
    f.git(&["commit", "-m", "new"]);
    f.git(&["push"]);
    f.stub_git();
    f.writes(json!({f.root.join("AGENTS.md").to_str().unwrap():"changed"}));
    unsafe { env::set_var("HOOK_GIT_FAIL_PUSH", "1") };
    assert!(
        handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root}))
            .unwrap()
            .unwrap()
            .contains("push")
    );
    let state = f.state();
    assert_eq!(state["phase"], "push_pending");
    let commit = f.git(&["rev-parse", "HEAD"]);
    unsafe {
        env::remove_var("HOOK_GIT_FAIL_PUSH");
        env::set_var("HOOK_STUB_EXIT", "9")
    };
    assert_eq!(
        handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root})).unwrap(),
        None
    );
    assert_eq!(f.git(&["rev-parse", "HEAD"]), commit);
    assert_eq!(f.state()["phase"], "complete");
}
#[test]
fn changed_index_blocks_staged_recovery() {
    let f = Fixture::new();
    f.stub_git();
    f.writes(json!({f.root.join("AGENTS.md").to_str().unwrap():"changed"}));
    unsafe { env::set_var("HOOK_GIT_FAIL_COMMIT", "1") };
    assert!(f.maintain().is_some());
    fs::write(f.root.join("source.txt"), "other stage").unwrap();
    f.git(&["add", "source.txt"]);
    unsafe { env::remove_var("HOOK_GIT_FAIL_COMMIT") };
    assert!(f.maintain().unwrap().contains("index"));
    assert_eq!(f.state()["phase"], "staged");
}
#[test]
fn unpushed_head_and_worktree_changes_do_not_run_worker() {
    let f = Fixture::new();
    handle_hook(&json!({"hook_event_name":"UserPromptSubmit","cwd":f.root})).unwrap();
    fs::write(f.root.join("source.txt"), "new").unwrap();
    assert_eq!(
        handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root})).unwrap(),
        None
    );
    f.git(&["add", "source.txt"]);
    f.git(&["commit", "-m", "unpushed"]);
    assert_eq!(
        handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root})).unwrap(),
        None
    );
    assert!(!f.base.join("prompt.json").exists());
}
#[test]
fn stopped_prompt_does_not_consume_prior_turn_push() {
    let f = Fixture::new();
    handle_hook(&json!({"hook_event_name":"UserPromptSubmit","cwd":f.root})).unwrap();
    fs::write(f.root.join("source.txt"), "after turn").unwrap();
    f.git(&["add", "source.txt"]);
    f.git(&["commit", "-m", "after turn"]);
    f.git(&["push"]);
    handle_hook(&json!({"hook_event_name":"UserPromptSubmit","cwd":f.root})).unwrap();
    assert!(!f.base.join("prompt.json").exists());
    assert_eq!(
        handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root})).unwrap(),
        None
    );
    assert!(f.base.join("prompt.json").exists());
}
#[test]
fn oversized_worker_output_is_not_committed() {
    let f = Fixture::new();
    let before = f.git(&["rev-parse", "HEAD"]);
    f.writes(json!({f.root.join("AGENTS.md").to_str().unwrap():"a".repeat(30000)}));
    assert!(f.maintain().unwrap().contains("サイズ超過"));
    assert_eq!(f.git(&["rev-parse", "HEAD"]), before);
    assert_eq!(f.state()["phase"], "ready");
}
#[test]
fn clean_filter_expansion_is_retained_without_push() {
    let f = Fixture::new();
    let mut state = f.checkpoint("ready");
    fs::write(f.root.join("AGENTS.md"), "tiny edited").unwrap();
    state["after"]["AGENTS.md"] = json!(document_hash(&f.root, "AGENTS.md").unwrap());
    f.save(&mut state);
    fs::write(f.root.join(".gitattributes"), "AGENTS.md filter=expand\n").unwrap();
    f.git(&[
        "config",
        "filter.expand.clean",
        &f.base
            .join("bin/filter.exe")
            .to_string_lossy()
            .replace('\\', "/"),
    ]);
    let before = f.git(&["rev-parse", "origin/main"]);
    assert!(f.maintain().unwrap().contains("公開 commit"));
    assert_eq!(f.git(&["rev-parse", "origin/main"]), before);
    assert_ne!(f.git(&["rev-parse", "HEAD"]), before);
    assert_eq!(f.state()["phase"], "push_pending");
    assert_eq!(
        fs::read_to_string(f.root.join("AGENTS.md")).unwrap(),
        "tiny edited"
    );
}
#[test]
fn completed_recovery_records_warnings_and_preserves_later_push() {
    let f = Fixture::new();
    handle_hook(&json!({"hook_event_name":"UserPromptSubmit","cwd":f.root})).unwrap();
    fs::write(f.root.join("source.txt"), "new").unwrap();
    f.git(&["add", "source.txt"]);
    f.git(&["commit", "-m", "new"]);
    f.git(&["push"]);
    f.writes(json!({f.root.join("README.md").to_str().unwrap():"maintained"}));
    handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root})).unwrap();
    let complete = f.state();
    let commit = f.git(&["rev-parse", "HEAD"]);
    let db = open_state_db().unwrap();
    db.execute("DELETE FROM poll_cursors", []).unwrap();
    drop(db);
    fs::write(f.root.join("AGENTS.md"), "x".repeat(30000)).unwrap();
    assert_eq!(
        handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root})).unwrap(),
        None
    );
    assert!(
        f.state()["warning"]
            .as_str()
            .unwrap()
            .contains("サイズ超過")
    );
    assert_eq!(f.git(&["rev-parse", "HEAD"]), commit);
    assert_eq!(f.state()["commit"], complete["commit"]);
}
#[test]
fn deleted_override_and_nested_design_do_not_request_replacement() {
    let f = Fixture::new();
    let scope = Scope::default();
    let reason = build_maintenance_reason(
        &f.root,
        &scope,
        &[
            f.root.join("AGENTS.override.md"),
            f.root.join("child/DESIGN.md"),
        ],
        "",
        "pushed",
    );
    assert!(!reason.contains("有効な AGENTS.md がありません"));
    assert!(!reason.contains("有効な DESIGN.md がありません"));
    assert!(reason.contains("意図的なら再作成せず"));
}
#[test]
fn windows_case_changed_and_relative_cwd_keep_validation() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("child")).unwrap();
    fs::write(f.root.join("child/AGENTS.md"), "child instructions").unwrap();
    let cwd = f.root.join("child").to_string_lossy().to_lowercase();
    assert!(
        active_agents_files(&f.root, std::path::Path::new(&cwd))
            .iter()
            .any(|p| p.ends_with("child/AGENTS.md"))
    );
    assert_eq!(
        agents_size_error(&f.root, &[PathBuf::from(&cwd)], true),
        None
    );
    let cwd = std::path::absolute(".").unwrap();
    assert!(cwd.is_absolute());
}
#[test]
fn casefold_collisions_are_ambiguous_in_index() {
    let f = Fixture::new();
    f.git(&["config", "core.ignorecase", "false"]);
    let blob = f.git(&["hash-object", "-w", "AGENTS.md"]).trim().to_owned();
    f.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("100644,{blob},Straße/AGENTS.md"),
    ]);
    f.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("100644,{blob},STRASSE/AGENTS.md"),
    ]);
    assert_eq!(
        index_stage0(&f.root, "strasse/agents.md").unwrap().kind,
        "ambiguous"
    );
}
#[test]
fn parent_death_closes_worker_job() {
    let f = Fixture::new();
    unsafe {
        env::set_var("HOOK_CHILD_PID", f.base.join("child.pid"));
        env::set_var("HOOK_PARENT_SLEEP", "1")
    };
    let mut host = Command::new(f.base.join("bin/managed-host.exe"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid_path = f.base.join("child.pid");
    for _ in 0..50 {
        if pid_path.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid = fs::read_to_string(pid_path).unwrap();
    host.kill().unwrap();
    host.wait().unwrap();
    let mut c = Command::new("pwsh");
    let r=c.args(["-NoProfile","-Command",&format!("$p=Get-Process -Id {pid} -ErrorAction SilentlyContinue; if($p){{try{{$p.WaitForExit(5000)}}catch{{}}; if(-not $p.HasExited){{exit 1}}}}; exit 0")]).output().unwrap();
    assert!(r.status.success());
}

#[test]
fn first_push_on_new_branch_after_monitor_start_is_processed() {
    let f = Fixture::new();
    handle_hook(&json!({"hook_event_name":"UserPromptSubmit","cwd":f.root})).unwrap();
    // Reflog timestamps have second precision; make the monitor-before-push ordering explicit.
    open_state_db()
        .unwrap()
        .execute(
            "UPDATE poll_metadata SET value='0' WHERE name='started_at'",
            [],
        )
        .unwrap();
    f.git(&["switch", "-c", "topic"]);
    fs::write(f.root.join("source.txt"), "topic").unwrap();
    f.git(&["add", "source.txt"]);
    f.git(&["commit", "-m", "topic"]);
    f.git(&["push", "-u", "origin", "topic"]);
    assert_eq!(
        handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root})).unwrap(),
        None
    );
    assert!(f.base.join("prompt.json").exists());
}
#[test]
fn scope_tracks_changed_child_override_and_root_readme_only() {
    let f = Fixture::new();
    let previous = f.git(&["rev-parse", "HEAD"]);
    fs::create_dir(f.root.join("child")).unwrap();
    for (name, body) in [
        ("AGENTS.override.md", "override"),
        ("DESIGN.md", "child design"),
        ("README.md", "child readme"),
        ("source.txt", "new"),
    ] {
        fs::write(f.root.join("child").join(name), body).unwrap();
    }
    f.git(&["add", "."]);
    f.git(&["commit", "-m", "child"]);
    let scope = maintenance_scope(&f.root, previous.trim(), "HEAD").unwrap();
    assert!(
        scope
            .agents
            .contains(&f.root.join("child/AGENTS.override.md"))
    );
    assert!(scope.design.contains(&f.root.join("child/DESIGN.md")));
    assert_eq!(scope.readme, vec![f.root.join("README.md")]);
}
#[test]
fn ignored_personal_override_is_counted_and_never_owned() {
    let f = Fixture::new();
    fs::write(f.root.join(".gitignore"), "AGENTS.override.md\n").unwrap();
    f.git(&["add", ".gitignore"]);
    f.git(&["commit", "-m", "personal override"]);
    f.git(&["push"]);
    fs::write(f.root.join("AGENTS.override.md"), "x".repeat(30000)).unwrap();
    assert!(f.maintain().unwrap().contains("サイズ超過"));
    assert!(f.state()["before"].get("AGENTS.override.md").is_none());
    assert_eq!(
        fs::read(f.root.join("AGENTS.override.md")).unwrap().len(),
        30000
    );
}
#[test]
fn committed_agents_symlink_is_fail_closed() {
    let f = Fixture::new();
    let mut child = Command::new("git")
        .arg("-C")
        .arg(&f.root)
        .args(["hash-object", "-w", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"README.md").unwrap();
    let blob = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
    f.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("120000,{},linked/AGENTS.md", blob.trim()),
    ]);
    f.git(&["commit", "-m", "symlink tree"]);
    assert!(
        agents_tree_size_error(&f.root, "HEAD", &[])
            .unwrap()
            .contains("実体")
    );
}
#[test]
fn unchanged_turn_and_noop_push_do_not_continue() {
    let f = Fixture::new();
    handle_hook(&json!({"hook_event_name":"UserPromptSubmit","cwd":f.root})).unwrap();
    for _ in 0..2 {
        handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root})).unwrap();
    }
    f.git(&["push"]);
    handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root})).unwrap();
    assert!(!f.base.join("prompt.json").exists());
}
#[test]
fn all_push_urls_are_checked() {
    let f = Fixture::new();
    f.git(&[
        "remote",
        "set-url",
        "--add",
        "--push",
        "origin",
        f.remote.to_str().unwrap(),
    ]);
    f.git(&[
        "remote",
        "set-url",
        "--add",
        "--push",
        "origin",
        "https://github.com/owner/repo.git",
    ]);
    unsafe { env::set_var("HOOK_STUB_GH_EXIT", "1") };
    assert!(
        external_pr_error(&f.root, "origin", "refs/heads/main")
            .unwrap()
            .contains("不明")
    );
}
#[test]
fn sent_pending_recovery_warns_without_repush() {
    let f = Fixture::new();
    f.writes(json!({f.root.join("AGENTS.md").to_str().unwrap():"changed"}));
    assert_eq!(f.maintain(), None);
    let mut state = f.state();
    state["phase"] = json!("push_pending");
    state.as_object_mut().unwrap().remove("validation_dirs");
    f.save(&mut state);
    fs::write(f.root.join("AGENTS.md"), "x".repeat(30000)).unwrap();
    let commit = f.git(&["rev-parse", "HEAD"]);
    f.stub_git();
    unsafe { env::set_var("HOOK_GIT_FAIL_PUSH", "1") };
    assert_eq!(f.maintain(), None);
    assert_eq!(f.state()["phase"], "complete");
    assert!(
        f.state()["warning"]
            .as_str()
            .unwrap()
            .contains("サイズ超過")
    );
    assert_eq!(f.git(&["rev-parse", "HEAD"]), commit);
}
#[test]
fn external_pr_created_during_worker_blocks_commit() {
    let f = Fixture::new();
    f.stub_git();
    f.git(&[
        "remote",
        "set-url",
        "--push",
        "origin",
        "https://github.com/owner/repo.git",
    ]);
    let response = f.base.join("gh-response.json");
    let empty = json!({"data":{"repository":{"ref":{"associatedPullRequests":{"nodes":[],"pageInfo":{"hasNextPage":false}}}}}});
    fs::write(&response, empty.to_string()).unwrap();
    let blocked = json!({"data":{"repository":{"ref":{"associatedPullRequests":{"nodes":[{"isCrossRepository":true,"headRefName":"main","headRepository":{"nameWithOwner":"owner/repo"},"baseRepository":{"nameWithOwner":"other/repo"}}],"pageInfo":{"hasNextPage":false}}}}}});
    unsafe {
        env::set_var("HOOK_STUB_GH_JSON_FILE", &response);
        env::set_var("HOOK_GIT_FAIL_PUSH", "1")
    };
    f.writes(json!({f.root.join("AGENTS.md").to_str().unwrap():"worker change",response.to_str().unwrap():blocked.to_string()}));
    let head = f.git(&["rev-parse", "HEAD"]);
    assert!(f.maintain().unwrap().contains("OPEN PR"));
    assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(f.state()["phase"], "ready");
}
#[test]
fn external_pr_created_before_push_retains_pending_commit() {
    let f = Fixture::new();
    f.stub_git();
    f.git(&[
        "remote",
        "set-url",
        "--push",
        "origin",
        "https://github.com/owner/repo.git",
    ]);
    unsafe {
        env::set_var("HOOK_GIT_FAIL_PUSH", "1");
        env::set_var("HOOK_STUB_GH_BLOCK_AFTER", "2")
    };
    f.writes(json!({f.root.join("AGENTS.md").to_str().unwrap():"worker change"}));
    let head = f.git(&["rev-parse", "HEAD"]);
    assert!(f.maintain().unwrap().contains("OPEN PR"));
    assert_ne!(f.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(f.state()["phase"], "push_pending");
}
#[test]
fn unmanaged_pipe_deadline_preserves_helper_after_parent_exit_or_timeout() {
    for parent_running in [false, true] {
        let f = Fixture::new();
        f.helper_environment();
        if parent_running {
            unsafe { env::set_var("HOOK_PARENT_SLEEP", "1") }
        }
        let c = scriptura::process::command(f.base.join("bin/pipe-parent.exe").to_str().unwrap());
        let result = scriptura::process::capture(
            c,
            Some(vec![b'x'; 1000000]),
            Duration::from_millis(300),
            false,
        );
        assert!(result.err().unwrap().to_string().contains("unconfirmed"));
        f.finish_live_helper();
    }
}
#[test]
fn unmanaged_success_does_not_kill_detached_helper() {
    let f = Fixture::new();
    let exit = f.base.join("child-exit");
    unsafe {
        env::set_var("HOOK_CHILD_PID", f.base.join("child.pid"));
        env::set_var("HOOK_CHILD_EXIT", &exit);
    }
    let c = scriptura::process::command(f.base.join("bin/detached-parent.exe").to_str().unwrap());
    let result = scriptura::process::capture(c, None, Duration::from_secs(5), false).unwrap();
    assert_eq!(result.code, Some(0));
    let pid = fs::read_to_string(f.base.join("child.pid")).unwrap();
    let r=Command::new("pwsh").args(["-NoProfile","-Command",&format!("$p=Get-Process -Id {pid} -ErrorAction SilentlyContinue; if(-not $p){{exit 1}}; exit 0")]).output().unwrap();
    fs::write(exit, "exit").unwrap();
    assert!(r.status.success());
    let r=Command::new("pwsh").args(["-NoProfile","-Command",&format!("$p=Get-Process -Id {pid} -ErrorAction SilentlyContinue; if($p){{$p.WaitForExit(5000);if(-not $p.HasExited){{exit 1}}}};exit 0")]).output().unwrap();
    assert!(r.status.success());
}

// ---------------------------------------------------------------------------
// P3: 利用者設定による起動判定・旧利用者の移行・documents の絞り込み
// ---------------------------------------------------------------------------
#[test]
fn no_config_and_no_state_does_nothing_and_creates_nothing() {
    let f = Fixture::with_config(None);
    assert_eq!(f.hook("UserPromptSubmit"), None);
    f.push_change("pushed by a brand new user");
    assert_eq!(f.hook("Stop"), None);
    // 同意なしに状態 DB も状態フォルダーも設定ファイルも作らない。
    assert!(!state_db_path().exists());
    assert!(!state_db_path().parent().unwrap().exists());
    assert!(!f.config_path().exists());
    assert!(!f.base.join("prompt.json").exists());
}
/// 状態 DB の user_version を直接書き換える（旧 DB は 0、新版が開いた DB は 1）。
fn set_state_db_version(version: i64) {
    let db = Connection::open(state_db_path()).unwrap();
    db.execute_batch(&format!("PRAGMA user_version={version}"))
        .unwrap();
}
#[test]
fn state_created_by_the_new_version_never_triggers_legacy_migration() {
    // 新版の hook / enable が作った DB（user_version=1）は、poll_cursors に行があっても旧利用者ではない。
    // 設定を失った（消した・保存に失敗した）新規利用者に「全リポジトリ・push・権限バイパス」を作らない。
    let f = Fixture::new();
    assert_eq!(f.hook("UserPromptSubmit"), None);
    assert!(f.cursor().is_some());
    let version: i64 = Connection::open(state_db_path())
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, STATE_SCHEMA_VERSION);
    fs::remove_file(f.config_path()).unwrap();
    f.push_change("push without config");
    f.writes(json!({f.root.join("README.md").to_str().unwrap():"must not be written"}));
    assert_eq!(f.hook("Stop"), None);
    assert!(!f.config_path().exists());
    assert!(!f.base.join("prompt.json").exists());
    // enable 相当の baseline だけで作られた DB でも同じ（fixture は環境変数を共有するので先に破棄する）。
    drop(f);
    let g = Fixture::with_config(None);
    baseline(&g.root).unwrap();
    assert!(g.cursor().is_some());
    g.push_change("push after baseline only");
    assert_eq!(g.hook("Stop"), None);
    assert!(!g.config_path().exists());
    assert!(!g.base.join("prompt.json").exists());
}
#[test]
fn legacy_state_without_config_migrates_once_and_behaves_as_before() {
    let f = Fixture::new();
    assert_eq!(f.hook("UserPromptSubmit"), None);
    // 設定導入前の旧利用者（版を持たない user_version=0 の状態 DB だけがある）の状態にする。
    set_state_db_version(0);
    fs::remove_file(f.config_path()).unwrap();
    f.push_change("new push");
    f.writes(json!({f.root.join("README.md").to_str().unwrap():"documented change"}));
    let message = f.hook("Stop").unwrap();
    assert!(message.contains("引き継"), "{message}");
    match config::load_from(&f.config_path()) {
        config::Loaded::Ok(c) => {
            assert_eq!(
                c.consent.unwrap().source,
                config::ConsentSource::LegacyMigration
            );
            assert_eq!(c.enrollment.policy, Policy::All);
            assert_eq!(c.defaults.mode, Mode::Push);
            assert_eq!(c.worker.permission, Permission::Bypass);
            assert_eq!(c.defaults.documents, Document::ALL.to_vec());
        }
        other => panic!("{other:?}"),
    }
    // 従来と同一の動作: worker が走り、文書 commit が push される。
    assert_eq!(f.state()["phase"], "complete");
    assert_eq!(f.git(&["rev-list", "--count", "HEAD"]).trim(), "3");
    assert_eq!(
        f.git(&["rev-parse", "HEAD"]),
        f.git(&["rev-parse", "origin/main"])
    );
    // 2 回目以降は案内も設定の書き直しも無い。
    let text = fs::read_to_string(f.config_path()).unwrap();
    assert_eq!(f.hook("Stop"), None);
    assert_eq!(fs::read_to_string(f.config_path()).unwrap(), text);
    // 状態 DB は schema 版を持つ。
    let db = Connection::open(state_db_path()).unwrap();
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, STATE_SCHEMA_VERSION);
}
#[test]
fn missing_consent_and_unenrolled_repositories_are_ignored_silently() {
    let mut no_consent = Config::legacy_all_push();
    no_consent.consent = None;
    let mut unenrolled = Config::legacy_all_push();
    unenrolled.enrollment.policy = Policy::Explicit;
    for config in [no_consent, unenrolled] {
        let f = Fixture::with_config(Some(config));
        assert_eq!(f.hook("UserPromptSubmit"), None);
        f.push_change("ignored push");
        assert_eq!(f.hook("Stop"), None);
        assert!(!state_db_path().exists());
        assert!(!f.base.join("prompt.json").exists());
    }
}
#[test]
fn remote_mismatch_and_disallowed_branch_hold_the_cursor() {
    let f = Fixture::new();
    f.hook("UserPromptSubmit");
    let before = f.cursor().unwrap();
    f.push_change("held push");
    f.set_config(&f.registered_config(|r| r.remote = Some("github.com/acme/other".into())));
    let message = f.hook("Stop").unwrap();
    assert!(message.contains("remote"), "{message}");
    assert_eq!(f.cursor().unwrap(), before);
    f.set_config(&f.registered_config(|r| r.branches = vec!["develop".into()]));
    let message = f.hook("Stop").unwrap();
    assert!(message.contains("ブランチ"), "{message}");
    assert_eq!(f.cursor().unwrap(), before);
    assert!(!f.base.join("prompt.json").exists());
    // 設定が整うと、位置を進めていなかった push が処理される。
    f.set_config(&f.registered_config(|r| r.branches = vec!["main".into()]));
    assert_eq!(f.hook("Stop"), None);
    assert!(f.base.join("prompt.json").exists());
    assert_ne!(f.cursor().unwrap(), before);
}
#[test]
fn paused_and_off_advance_the_cursor_without_running() {
    for off in ["paused", "mode-off"] {
        let f = Fixture::new();
        f.hook("UserPromptSubmit");
        let before = f.cursor().unwrap();
        f.push_change("skipped push");
        f.set_config(&f.registered_config(|r| {
            if off == "paused" {
                r.paused = true
            } else {
                r.mode = Some(Mode::Off)
            }
        }));
        assert_eq!(f.hook("Stop"), None);
        assert_ne!(f.cursor().unwrap(), before, "{off}");
        assert!(!f.base.join("prompt.json").exists());
        // 有効に戻しても、見送った過去の push は処理しない。
        f.set_config(&f.registered_config(|_| {}));
        assert_eq!(f.hook("Stop"), None);
        assert!(!f.base.join("prompt.json").exists(), "{off}");
        f.push_change("processed push");
        assert_eq!(f.hook("Stop"), None);
        assert!(f.base.join("prompt.json").exists(), "{off}");
    }
}
#[test]
fn repository_local_files_never_enable_maintenance() {
    let enabling = toml::to_string_pretty(&Config::legacy_all_push()).unwrap();
    let mut explicit = Config::legacy_all_push();
    explicit.enrollment.policy = Policy::Explicit;
    for user_config in [None, Some(explicit)] {
        let f = Fixture::with_config(user_config);
        fs::create_dir_all(f.root.join(".scriptura")).unwrap();
        for name in [
            ".scriptura.toml",
            "scriptura.toml",
            ".scriptura/config.toml",
        ] {
            fs::write(f.root.join(name), &enabling).unwrap();
        }
        fs::write(
            f.root.join("AGENTS.md"),
            format!("```toml\n{enabling}\n```\n"),
        )
        .unwrap();
        f.git(&["add", "."]);
        f.git(&["commit", "-m", "repository-local enabling attempts"]);
        f.git(&["push"]);
        assert_eq!(f.hook("UserPromptSubmit"), None);
        f.push_change("push after local config");
        assert_eq!(f.hook("Stop"), None);
        assert!(!state_db_path().exists());
        assert!(!f.base.join("prompt.json").exists());
    }
}
#[test]
fn documents_setting_narrows_prompt_and_allowed_paths() {
    let mut config = Config::legacy_all_push();
    config.defaults.documents = vec![Document::Readme];
    let f = Fixture::with_config(Some(config));
    f.hook("UserPromptSubmit");
    f.push_change("docs filter");
    f.writes(json!({f.root.join("README.md").to_str().unwrap():"readme only"}));
    assert_eq!(f.hook("Stop"), None);
    let prompt = f.prompt_record()["prompt"].as_str().unwrap().to_owned();
    assert!(prompt.contains("README.md"));
    assert!(
        !prompt.contains("AGENTS") && !prompt.contains("DESIGN"),
        "{prompt}"
    );
    let state = f.state();
    assert!(state["before"].get("README.md").is_some());
    assert!(state["before"].get("AGENTS.md").is_none());
    assert!(state["before"].get("DESIGN.md").is_none());
    assert_eq!(
        f.git(&["show", "--format=", "--name-only", "HEAD"]),
        "README.md\n"
    );
}
#[test]
fn documents_outside_the_setting_are_out_of_scope_for_the_worker() {
    let f = Fixture::new();
    // 対象外の変更の検出（全体比較）は Restricted のときだけ行う。
    let options = MaintenanceOptions {
        documents: vec![Document::Readme],
        ..options_with(Permission::Restricted)
    };
    f.writes(json!({f.root.join("AGENTS.md").to_str().unwrap():"not allowed"}));
    let head = f.git(&["rev-parse", "HEAD"]);
    let message = maintain_and_publish_with(
        &f.root,
        "fixture reason",
        &[f.root.join("README.md")],
        Some("fixture"),
        &[f.root.clone()],
        &options,
    )
    .unwrap();
    assert!(message.contains("AGENTS.md"), "{message}");
    assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(f.state()["phase"], "worker_failed");
}
#[test]
fn newer_state_schema_is_left_untouched() {
    let f = Fixture::new();
    let path = state_db_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch("PRAGMA user_version=99").unwrap();
    }
    assert_eq!(f.hook("UserPromptSubmit"), None);
    f.push_change("newer schema");
    assert_eq!(f.hook("Stop"), None);
    let db = Connection::open(&path).unwrap();
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 99);
    let tables: i64 = db
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tables, 0);
    assert!(!path.with_extension("poll-lock.sqlite3").exists());
    assert!(!f.base.join("prompt.json").exists());
}
#[test]
fn broken_config_does_nothing_with_a_single_message() {
    for text in [
        "not = [toml",
        "schema = 1\nunknown_key = true\n",
        "schema = 99\n",
    ] {
        let f = Fixture::with_config(None);
        fs::write(f.config_path(), text).unwrap();
        let message = f.hook("UserPromptSubmit").unwrap();
        assert!(message.contains("設定"), "{message}");
        // TOML のエラー全文（設定ファイルの該当行の抜粋）は出さず、先頭 1 行だけにする。
        assert!(!message.contains('\n'), "{message}");
        f.push_change("broken config");
        let message = f.hook("Stop").unwrap();
        assert!(message.contains("設定"), "{message}");
        assert!(!message.contains('\n'), "{message}");
        assert!(!state_db_path().exists());
        assert!(!f.base.join("prompt.json").exists());
        // 壊れた設定は書き換えない。
        assert_eq!(fs::read_to_string(f.config_path()).unwrap(), text);
    }
}
#[test]
fn baseline_records_the_current_push_without_processing_it() {
    let f = Fixture::new();
    f.push_change("before enabling");
    baseline(&f.root).unwrap();
    assert!(f.cursor().is_some());
    assert_eq!(f.hook("Stop"), None);
    assert!(!f.base.join("prompt.json").exists());
    f.push_change("after enabling");
    assert_eq!(f.hook("Stop"), None);
    assert!(f.base.join("prompt.json").exists());
}
#[test]
fn run_now_processes_the_current_state_even_when_already_recorded() {
    let f = Fixture::new();
    f.hook("UserPromptSubmit");
    f.push_change("recorded");
    assert_eq!(f.hook("Stop"), None);
    fs::write(f.base.join("prompt.json"), "").unwrap();
    f.writes(json!({f.root.join("DESIGN.md").to_str().unwrap():"forced"}));
    // 通常の poll は位置が同じなので何もしない。
    assert_eq!(f.hook("Stop"), None);
    assert_eq!(fs::read_to_string(f.base.join("prompt.json")).unwrap(), "");
    assert_eq!(
        run_now(&f.root, MaintenanceOptions::default()).unwrap(),
        None
    );
    assert!(f.prompt_record()["prompt"].is_string());
}

// ---------------------------------------------------------------------------
// P4: worker の権限制限・設定反映・対象外変更の検出
// ---------------------------------------------------------------------------
fn section(args: &[Value], flag: &str) -> Vec<String> {
    let start = args.iter().position(|a| a == flag).unwrap() + 1;
    args[start..]
        .iter()
        .map(|a| a.as_str().unwrap().to_owned())
        .take_while(|a| !a.starts_with("--"))
        .collect()
}
fn pair(args: &[Value], flag: &str, value: &str) -> bool {
    args.windows(2).any(|w| w[0] == flag && w[1] == value)
}
#[test]
fn restricted_worker_argument_contract_and_settings() {
    let f = Fixture::new();
    // PATH のスタブを退避し、設定の claude_path だけで起動できること（優先されること）を確かめる。
    let alt = f.base.join("alt");
    fs::create_dir_all(&alt).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_hook-test-stub"), alt.join("claude.exe")).unwrap();
    fs::rename(
        f.base.join("bin/claude.exe"),
        f.base.join("bin/claude-disabled.exe"),
    )
    .unwrap();
    let extra = vec!["custom/**".to_string()];
    let options = MaintenanceOptions {
        documents: Document::ALL.to_vec(),
        exclude: config::effective_exclude(&[&extra]),
        worker: Worker {
            model: "model-x".into(),
            effort: "low".into(),
            timeout_secs: 300,
            max_budget_usd: 2.5,
            permission: Permission::Restricted,
            claude_path: alt.join("claude.exe").to_string_lossy().into_owned(),
        },
    };
    let keys = [
        "CLAUDECODE",
        "CLAUDE_CODE_SIMPLE",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ];
    unsafe {
        for k in keys {
            env::set_var(k, "1");
        }
    }
    let result = maintain_and_publish_with(
        &f.root,
        "fixture reason",
        &f.docs(),
        Some("fixture"),
        &[f.root.clone()],
        &options,
    );
    unsafe {
        for k in keys {
            env::remove_var(k);
        }
    }
    assert_eq!(result, None);
    let p = f.prompt_record();
    let args = p["args"].as_array().unwrap();
    assert!(!args.iter().any(|a| a == "--dangerously-skip-permissions"));
    for flag in [
        "-p",
        "--safe-mode",
        "--no-session-persistence",
        "--strict-mcp-config",
    ] {
        assert!(args.iter().any(|a| a == flag), "{flag}");
    }
    for (flag, value) in [
        ("--model", "model-x"),
        ("--effort", "low"),
        ("--permission-mode", "dontAsk"),
        ("--tools", "Read,Edit,Write,Glob,Grep,Bash"),
        ("--max-budget-usd", "2.5"),
        ("--output-format", "text"),
    ] {
        assert!(pair(args, flag, value), "{flag} {value}");
    }
    let allowed = section(args, "--allowedTools");
    for rule in [
        "Read(./**)",
        "Edit(./AGENTS.md)",
        "Edit(./DESIGN.md)",
        "Edit(./README.md)",
        "Bash(git status *)",
        "Bash(git check-ignore *)",
        "Bash(git ls-files *)",
    ] {
        assert!(allowed.iter().any(|a| a == rule), "{rule}: {allowed:?}");
    }
    // 無条件の Read / Glob / Grep / Edit / Write / Bash は許可しない（読み取りは cwd 配下、編集は許可パス、
    // git は読み取り専用の 3 種だけ）。
    for bare in ["Read", "Glob", "Grep", "Edit", "Write", "Bash"] {
        assert!(!allowed.iter().any(|a| a == bare), "{bare}");
    }
    // git diff 系は --output= などで作業ツリー外へ書けるため許可しない（検証は Rust 側の事後検査が担う）。
    assert!(
        !allowed.iter().any(|a| a.contains("git diff")),
        "{allowed:?}"
    );
    let denied = section(args, "--disallowedTools");
    for rule in [
        "Read(.env*)",
        "Edit(.env*)",
        "Read(**/.env*)",
        "Read(*.pem)",
        "Read(**/secrets/**)",
        "Edit(**/secrets/**)",
        "Read(custom/**)",
    ] {
        assert!(denied.iter().any(|a| a == rule), "{rule}: {denied:?}");
    }
    // 環境変数の除去は restricted でも維持される。
    assert_eq!(p["inherited_host_env"], json!([]));
    assert_eq!(p["oauth_token_kept"], true);
    assert_eq!(p["worker_env"], "1");
}
#[test]
fn worker_args_omit_budget_when_zero_and_keep_bypass_contract() {
    let worker = Worker {
        permission: Permission::Bypass,
        ..Worker::default()
    };
    let args = worker_args(&worker, &[], &[]);
    assert!(!args.iter().any(|a| a == "--max-budget-usd"));
    assert!(args.iter().any(|a| a == "--dangerously-skip-permissions"));
    assert!(
        !args
            .iter()
            .any(|a| a == "--safe-mode" || a == "--allowedTools")
    );
    // 規則として安全に書けないパスは許可規則に出さない。
    let restricted = Worker {
        permission: Permission::Restricted,
        ..Worker::default()
    };
    let args = worker_args(
        &restricted,
        &["a*b/AGENTS.md".into(), "ok/README.md".into()],
        &[],
    );
    assert!(args.iter().any(|a| a == "Edit(./ok/README.md)"));
    assert!(!args.iter().any(|a| a.contains("a*b")));
}
#[test]
fn find_in_path_works_like_which() {
    let f = Fixture::new();
    let bin = f.base.join("bin");
    let joined = env::join_paths([f.base.join("missing"), bin.clone()]).unwrap();
    assert_eq!(
        find_in_path("claude.exe", Some(joined.clone())),
        Some(bin.join("claude.exe"))
    );
    assert_eq!(find_in_path("no-such-tool.exe", Some(joined)), None);
    assert_eq!(find_in_path("claude.exe", None), None);
}
#[test]
fn worker_timeout_comes_from_settings() {
    let f = Fixture::new();
    unsafe { env::set_var("HOOK_STUB_SLEEP", "30") };
    let options = MaintenanceOptions {
        worker: Worker {
            timeout_secs: 1,
            permission: Permission::Restricted,
            ..Worker::default()
        },
        ..MaintenanceOptions::default()
    };
    let started = std::time::Instant::now();
    let message = run_maintenance_worker_with(&f.root, "prompt", &[], &options).unwrap();
    assert!(
        message.contains("1 秒") && message.contains("タイムアウト"),
        "{message}"
    );
    assert!(started.elapsed() < Duration::from_secs(20));
}
fn options_with(permission: Permission) -> MaintenanceOptions {
    MaintenanceOptions {
        worker: Worker {
            permission,
            ..Worker::default()
        },
        ..MaintenanceOptions::default()
    }
}
impl Fixture {
    fn maintain_with(&self, permission: Permission) -> Option<String> {
        maintain_and_publish_with(
            &self.root,
            "fixture reason",
            &self.docs(),
            Some("fixture"),
            &[self.root.clone()],
            &options_with(permission),
        )
    }
    /// HEAD コミットが変更したファイルの一覧。
    fn head_files(&self) -> Vec<String> {
        self.git(&["show", "--name-only", "--format=", "HEAD"])
            .lines()
            .map(str::to_owned)
            .collect()
    }
}
#[test]
fn out_of_scope_change_fails_without_commit_when_restricted() {
    // push モードの全体比較は Restricted のときだけ。追跡・未追跡・入れ子の対象外変更を検出する。
    for case in ["tracked", "untracked", "nested"] {
        let f = Fixture::new();
        let (target, shown) = match case {
            "tracked" => ("source.txt", "source.txt"),
            "untracked" => ("extra.txt", "extra.txt"),
            _ => ("deep/er/new.txt", "deep/er/new.txt"),
        };
        f.writes(json!({
            f.root.join(target).to_str().unwrap(): "tampered",
            f.root.join("AGENTS.md").to_str().unwrap(): "legitimate doc edit"
        }));
        let head = f.git(&["rev-parse", "HEAD"]);
        let remote = f.git(&["rev-parse", "origin/main"]);
        let message = f.maintain_with(Permission::Restricted).unwrap();
        assert!(message.contains(shown), "{case}: {message}");
        assert!(message.contains("許可文書以外"), "{message}");
        // commit もパッチ（push）も作らない。
        assert_eq!(f.git(&["rev-parse", "HEAD"]), head, "{case}");
        assert_eq!(f.git(&["rev-parse", "origin/main"]), remote, "{case}");
        assert_eq!(f.state()["phase"], "worker_failed", "{case}");
    }
}
#[test]
fn ignored_files_changed_during_worker_are_not_violations_in_push_mode() {
    // push モードは利用者の実作業ツリーで動く。target/ や node_modules/ のような無視対象は
    // 利用者・ツールが並行して書き換えるので、worker の違反として扱わない（文書の commit は成立する）。
    let f = Fixture::new();
    fs::write(
        f.root.join(".gitignore"),
        "cache/
",
    )
    .unwrap();
    f.git(&["add", ".gitignore"]);
    f.git(&["commit", "-m", "ignore cache"]);
    f.git(&["push"]);
    f.writes(json!({
        f.root.join("cache/x.bin").to_str().unwrap(): "concurrent build output",
        f.root.join("AGENTS.md").to_str().unwrap(): "legitimate doc edit"
    }));
    assert_eq!(f.maintain_with(Permission::Restricted), None);
    assert_eq!(f.state()["phase"], "complete");
    assert_eq!(f.head_files(), vec!["AGENTS.md".to_string()]);
}
#[test]
fn bypass_only_guards_the_commit_target_hashes() {
    // Bypass（旧動作互換）は全体比較をせず、従来どおり commit 対象文書のハッシュ検査だけを守る。
    // 対象外ファイルの変更は commit に混ざらない（文書だけが commit される）。
    for target in ["source.txt", "extra.txt"] {
        let f = Fixture::new();
        f.writes(json!({
            f.root.join(target).to_str().unwrap(): "changed beside the docs",
            f.root.join("AGENTS.md").to_str().unwrap(): "legitimate doc edit"
        }));
        assert_eq!(f.maintain_with(Permission::Bypass), None, "{target}");
        assert_eq!(f.state()["phase"], "complete", "{target}");
        assert_eq!(f.head_files(), vec!["AGENTS.md".to_string()], "{target}");
        assert_eq!(
            fs::read_to_string(f.root.join(target)).unwrap(),
            "changed beside the docs",
            "{target}"
        );
    }
}
#[test]
fn out_of_scope_change_blocks_retry_until_reverted() {
    let f = Fixture::new();
    f.writes(json!({f.root.join("source.txt").to_str().unwrap():"tampered"}));
    assert!(
        f.maintain_with(Permission::Restricted)
            .unwrap()
            .contains("許可文書以外")
    );
    // 変更が残っている間は worker を再実行しない。
    fs::write(f.base.join("prompt.json"), "").unwrap();
    assert!(
        f.maintain_with(Permission::Restricted)
            .unwrap()
            .contains("前回")
    );
    assert_eq!(fs::read_to_string(f.base.join("prompt.json")).unwrap(), "");
    // 利用者が元に戻すと再開する。
    fs::write(f.root.join("source.txt"), "initial").unwrap();
    f.writes(json!({}));
    assert_eq!(f.maintain_with(Permission::Restricted), None);
    assert_eq!(f.state()["phase"], "complete");
    assert!(f.base.join("prompt.json").metadata().unwrap().len() > 0);
}
