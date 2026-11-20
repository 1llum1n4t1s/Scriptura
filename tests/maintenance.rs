#![cfg(feature = "test-support")]
use rusqlite::Connection;
use scriptura::config::{
    self, Config, Document, MigratedFrom, Permission, Policy, Repository, Worker,
};
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

/// 実行ファイルの名前（Windows は `.exe` 付き、それ以外は拡張子なし）。
/// スタブは `file_stem` で役割を決めるので、どちらの名前でも同じ役割になる。
fn exe(name: &str) -> String {
    format!("{name}{}", env::consts::EXE_SUFFIX)
}

/// 監視の開始時刻（`started_at`）が属する秒が終わるまで待つ。
/// reflog の時刻は秒精度（切り捨て）で、開始時刻は小数秒。待たないと、監視開始と同じ秒に行った
/// テスト内の push が「開始前の push」と判定されて見送られる。Windows では git が遅くて表に出ないが、
/// 速い Linux ランナーでは起きる。製品の判定は変えず、テストの順序だけを確定させる。
fn settle_after_monitor_start() {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let rest = 1_000_000_000 - u128::from(now.subsec_nanos());
    std::thread::sleep(Duration::from_nanos(rest as u64) + Duration::from_millis(20));
}

/// PATH から本物の git を探す（`where.exe` や `which` に頼らず、Windows・Linux で同じ手順にする）。
/// フィクスチャが PATH へスタブを足す前に呼ぶこと。
fn find_real_git() -> String {
    let name = exe("git");
    let path = env::var_os("PATH").unwrap_or_default();
    env::split_paths(&path)
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("PATH に {name} が見つかりません"))
        .to_string_lossy()
        .into_owned()
}

/// 指定 PID のプロセスがまだ生きているか。
#[cfg(windows)]
fn process_alive(pid: &str) -> bool {
    // 外部プロセス（pwsh）を起動せず Win32 で直接見る（100ms ごとのポーリングで起動コストが積み上がるため）。
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    const STILL_ACTIVE: u32 = 259;
    let pid: u32 = pid.trim().parse().expect("PID が数値ではありません");
    // SAFETY: 取得したハンドルは必ず CloseHandle で閉じる。
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // 権限不足で開けないだけなら、プロセス自体は存在する（生きている扱い）。
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        // 終了コード取得に失敗した場合は、開けた以上は生きている扱いにする。
        !ok || code == STILL_ACTIVE
    }
}

/// 指定 PID のプロセスがまだ生きているか。ゾンビ（終了済みで回収待ち）は終了扱いにする。
#[cfg(not(windows))]
fn process_alive(pid: &str) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        // 「pid (comm) 状態 ...」。comm に空白や括弧が入り得るので、最後の `)` の後ろを見る。
        Ok(stat) => stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| state != "Z" && state != "X"),
        // /proc が無い環境では kill -0 で存在だけ確かめる。
        Err(_) if !std::path::Path::new("/proc/self").exists() => Command::new("kill")
            .args(["-0", pid])
            .output()
            .is_ok_and(|o| o.status.success()),
        Err(_) => false,
    }
}

/// 指定 PID のプロセスが `millis` ミリ秒以内に終了するまで待つ。終了していれば true。
fn wait_process_exit(pid: &str, millis: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(millis);
    loop {
        if !process_alive(pid) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
#[test]
fn uncertain_commit_or_push_from_stop_is_never_automatically_repeated() {
    for operation in ["commit", "push"] {
        let f = Fixture::new();
        f.hook("UserPromptSubmit");
        fs::write(f.root.join("source.txt"), "new push").unwrap();
        f.git(&["add", "source.txt"]);
        f.git(&["commit", "-m", "new"]);
        f.git(&["push"]);
        f.stub_git();
        // 不確実な操作の後に、git スタブが孫（pipe-grandchild）を残す。
        f.stubs(&["pipe-grandchild"]);
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
// 継承された Windows のハンドルが pipe を握り続ける現象の検査。Linux では同じ状況が起きず、期限内に正常終了する。
#[cfg(windows)]
#[test]
fn null_stdio_with_inherited_windows_handles_still_has_deadline() {
    let f = Fixture::new();
    f.stubs(&["null-parent", "pipe-grandchild"]);
    f.helper_environment();
    let c = scriptura::process::command(
        f.base
            .join("bin")
            .join(exe("null-parent"))
            .to_str()
            .unwrap(),
    );
    let result = scriptura::process::capture(c, None, Duration::from_millis(3000), false);
    assert!(result.err().unwrap().to_string().contains("unconfirmed"));
    f.finish_live_helper();
}
#[test]
fn logical_parent_cwd_is_rejected_before_worker() {
    let f = Fixture::new();
    f.hook("UserPromptSubmit");
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
    f.hook("UserPromptSubmit");
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
        // 本物の git は、PATH へスタブを足す前に PATH から探す。
        let real = find_real_git();
        // 常に置くのは worker（claude）と gh だけ。スタブ 1 本は約 2.6MB あり、全テストで 8 本ずつ
        // コピーすると `validation/` が膨らむため、残りの役割は使うテストが `stubs` で置く。
        // 置き忘れは「実行ファイルが無い」で即失敗するので、検査が黙って空洞化することはない。
        for name in [&exe("claude"), &exe("gh")] {
            fs::copy(env!("CARGO_BIN_EXE_hook-test-stub"), bin.join(name)).unwrap();
        }
        let old_path = env::var_os("PATH").unwrap_or_default();
        unsafe {
            // PATH の区切り文字は OS で違う（Windows は `;`、Linux は `:`）ので join_paths で組む。
            let dirs = std::iter::once(bin.clone()).chain(env::split_paths(&old_path));
            env::set_var("PATH", env::join_paths(dirs).unwrap());
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
            branches: vec![],
            documents: None,
            exclude: vec![],
            paused: false,
            migrated_from: None,
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
        let result = handle_hook(&json!({"hook_event_name":event,"cwd":self.root})).unwrap();
        if event == "UserPromptSubmit" {
            settle_after_monitor_start();
        }
        result
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
    /// 指定した役割のスタブを `bin/` へ置く（役割はファイル名の file_stem で決まる）。
    /// `pipe-parent` / `detached-parent` / `null-parent` / `managed-host` は孫として
    /// `pipe-grandchild` を同じ場所から起動するので、呼び出し側で併せて指定する。
    fn stubs(&self, roles: &[&str]) {
        for role in roles {
            fs::copy(
                env!("CARGO_BIN_EXE_hook-test-stub"),
                self.base.join("bin").join(exe(role)),
            )
            .unwrap();
        }
    }
    fn stub_git(&self) {
        fs::copy(
            env!("CARGO_BIN_EXE_hook-test-stub"),
            self.base.join("bin").join(exe("git")),
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
        let alive = process_alive(&pid);
        fs::write(self.base.join("child-exit"), "exit").unwrap();
        assert!(alive, "helper was killed");
        assert!(wait_process_exit(&pid, 5000), "fixture helper did not exit");
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
    // normcase は Windows の大文字小文字・区切り文字の正規化を再現するもの。Windows でだけ確かめる。
    #[cfg(windows)]
    {
        assert_eq!(normcase("D:/Dev/İΟΣ"), "d:\\dev\\İοσ");
        assert_eq!(
            hash(normcase("D:\\Dev\\skills")),
            "05629082bd22e81e1dcbdf1da39fdfca9c45b422d7afcb1c905b21e88fa81fea"
        );
    }
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
    // 中断した worker の終了確認（`worker_quiescence_error`）は Windows でだけ行える。
    #[cfg(windows)]
    {
        assert_eq!(f.maintain(), None);
        assert_eq!(f.state()["phase"], "complete");
    }
    // Windows 以外では終了を確認できないので、fail-closed で再開を見送り、worker も起動しない。
    #[cfg(not(windows))]
    {
        let message = f.maintain().expect("再開を見送る理由を返す");
        assert!(message.contains("中断 worker"), "{message}");
        assert_eq!(f.state()["phase"], "worker_running");
        assert!(!f.base.join("prompt.json").exists());
    }
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
    assert_eq!(f.hook("UserPromptSubmit"), None);
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
/// PostToolUse（git push 直後）と SessionStart（取りこぼしの回収）は、旧登録の Stop と同じく新しい push を処理し、
/// 同じ push を二重に処理しない（旧登録と新登録が重なっても、処理済み位置が重複を防ぐ）。
#[test]
fn post_tool_use_and_session_start_process_a_push_once() {
    for event in ["PostToolUse", "SessionStart"] {
        let f = Fixture::new();
        assert_eq!(f.hook("UserPromptSubmit"), None);
        fs::write(f.root.join("source.txt"), "new change").unwrap();
        f.git(&["add", "source.txt"]);
        f.git(&["commit", "-m", "new pushed source"]);
        f.git(&["push"]);
        f.writes(json!({f.root.join("README.md").to_str().unwrap():"documented change"}));
        let payload = json!({
            "hook_event_name":event,"cwd":f.root,
            "tool_name":"Bash","tool_input":{"command":"git push"},"source":"startup"
        });
        assert_eq!(handle_hook(&payload).unwrap(), None, "{event}");
        assert_eq!(f.state()["phase"], "complete", "{event}");
        let count = f.git(&["rev-list", "--count", "HEAD"]);
        assert_eq!(count.trim(), "3", "{event}");
        // 同じ push は、どのイベントで呼ばれても再処理しない。
        assert_eq!(handle_hook(&payload).unwrap(), None);
        assert_eq!(f.hook("Stop"), None);
        assert_eq!(f.git(&["rev-list", "--count", "HEAD"]), count, "{event}");
    }
}
/// Claude Code・Codex の PostToolUse（失敗時の PostToolUseFailure を含む）の payload から、`push` の語を含みうる
/// コマンドだけを通す（`if` を持たない Codex でも状態 DB・ロックに触れる前に終わる）。判定できない形は処理側に倒す。
#[test]
fn post_tool_use_payload_filter_accepts_push_words_and_skips_other_commands() {
    use scriptura::maintenance::payload_may_push;
    let bash = |c: Value| json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":c}});
    for pushed in [
        "git push",
        "git push origin main",
        "rtk git push",
        "git -C D:/repo push origin HEAD",
        "git -c core.x=y push",
        "FOO=1 git push",
        "npm test && git push",
        "cd repo; git push --force-with-lease",
        "bash -lc 'git push'",
        "\"C:\\Program Files\\Git\\cmd\\git.exe\" push",
        "/usr/bin/git push",
        // `if`（`* push*`）を通る、git の語を含まない形も落とさない。
        "g push",
        "make push",
        "npm run push",
        "$GIT push origin",
        "git push && gh pr create",
    ] {
        assert!(payload_may_push(&bash(json!(pushed))), "{pushed}");
    }
    // Codex は command が配列のことがある。
    assert!(payload_may_push(&bash(json!(["bash", "-lc", "git push"]))));
    for other in [
        "git status",
        "git pull && cargo test",
        "pushd /tmp",
        "ls",
        "",
    ] {
        assert!(!payload_may_push(&bash(json!(other))), "{other}");
    }
    assert!(!payload_may_push(&bash(json!(["ls", "-la"]))));
    // ファイル編集・MCP のツールは対象外（Codex の apply_patch は command にパッチ本文が入る）。
    for tool in ["apply_patch", "Edit", "mcp__x__y"] {
        let p = json!({"tool_name":tool,"tool_input":{"command":"git push"}});
        assert!(!payload_may_push(&p), "{tool}");
    }
    // 判定できない形（command が無い・未知の型）は処理側に倒す。
    assert!(payload_may_push(&json!({"tool_name":"Bash"})));
    assert!(payload_may_push(
        &json!({"tool_name":"Bash","tool_input":{"cmd":"x"}})
    ));
    assert!(payload_may_push(&json!({})));
}
/// PostToolUse・SessionStart（非同期）は、別の保守がロックを持っていても待って処理する。同期の旧登録（Stop）は待たずに見送る。
/// push 成功後に後段が失敗した（PostToolUseFailure）場合も、PostToolUse と同じに処理する。
#[test]
fn async_hooks_wait_for_the_poll_lock_and_stop_does_not() {
    let f = Fixture::new();
    assert_eq!(f.hook("UserPromptSubmit"), None);
    fs::write(f.root.join("source.txt"), "new change").unwrap();
    f.git(&["add", "source.txt"]);
    f.git(&["commit", "-m", "new pushed source"]);
    f.git(&["push"]);
    f.writes(json!({f.root.join("README.md").to_str().unwrap():"documented change"}));
    let lock_path = state_db_path().with_extension("poll-lock.sqlite3");
    let held = Connection::open(&lock_path).unwrap();
    held.execute_batch("BEGIN IMMEDIATE").unwrap();
    // Stop は待たずに見送る（保守されず、コミットは増えない）。
    assert_eq!(
        handle_hook(&json!({"hook_event_name":"Stop","cwd":f.root})).unwrap(),
        None
    );
    assert_eq!(f.git(&["rev-list", "--count", "HEAD"]).trim(), "2");
    // 別の保守が終わる（ロックが解ける）まで待ってから処理する。
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        held.execute_batch("COMMIT").unwrap();
    });
    let payload = json!({
        "hook_event_name":"PostToolUseFailure","cwd":f.root,
        "tool_name":"Bash","tool_input":{"command":"git push && gh pr create"}
    });
    let started = std::time::Instant::now();
    assert_eq!(handle_hook(&payload).unwrap(), None);
    assert!(started.elapsed() >= std::time::Duration::from_millis(1000));
    release.join().unwrap();
    assert_eq!(f.state()["phase"], "complete");
    assert_eq!(f.git(&["rev-list", "--count", "HEAD"]).trim(), "3");
}
/// push を含まない PostToolUse と、知らないイベント名・cwd の無い payload は、状態に触れずに何もしない。
/// イベント名の綴りの違い（Codex など）は吸収し、既知の形はどちらでも動く。
#[test]
fn post_tool_use_without_push_and_unknown_payloads_touch_no_state() {
    let f = Fixture::new();
    for p in [
        json!({"hook_event_name":"PostToolUse","cwd":f.root,"tool_name":"Bash","tool_input":{"command":"git status"}}),
        json!({"hook_event_name":"PostToolUse","cwd":f.root,"tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch git push"}}),
        json!({"hook_event_name":"post_tool_use","cwd":f.root,"tool_name":"Bash","tool_input":{"command":["bash","-lc","ls"]}}),
        json!({"hookEventName":"PostToolUse","cwd":f.root,"tool_input":{"command":"echo hi"}}),
        json!({"hook_event_name":"PreToolUse","cwd":f.root,"tool_name":"Bash","tool_input":{"command":"git push"}}),
        json!({"hook_event_name":"SessionEnd","cwd":f.root}),
        json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"git push"}}),
    ] {
        assert_eq!(handle_hook(&p).unwrap(), None, "{p}");
    }
    assert!(!state_db_path().exists());
}
/// Codex 形式（`hook_event_name`・`cwd`・`tool_input.command`・`tool_response` などを持つ payload）の PostToolUse と、
/// `source` を持つ SessionStart でも push を処理する。
#[test]
fn codex_shaped_payloads_process_a_push() {
    let f = Fixture::new();
    assert_eq!(f.hook("UserPromptSubmit"), None);
    fs::write(f.root.join("source.txt"), "new change").unwrap();
    f.git(&["add", "source.txt"]);
    f.git(&["commit", "-m", "new pushed source"]);
    f.git(&["push"]);
    f.writes(json!({f.root.join("README.md").to_str().unwrap():"documented change"}));
    let payload = json!({
        "session_id":"s","transcript_path":"t","model":"gpt","permission_mode":"default",
        "hook_event_name":"PostToolUse","cwd":f.root,"tool_name":"Bash","tool_use_id":"u",
        "tool_input":{"command":"rtk git -C . push origin HEAD"},
        "tool_response":{"exit_code":0,"stdout":"ok"}
    });
    assert_eq!(handle_hook(&payload).unwrap(), None);
    assert_eq!(f.state()["phase"], "complete");
    assert_eq!(f.git(&["rev-list", "--count", "HEAD"]).trim(), "3");
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
// Job Object（Windows 限定の仕組み）で子孫を管理する検査。Linux の process::capture は managed 時に Job を使わない。
#[cfg(windows)]
#[test]
fn managed_job_closes_inherited_descendant_pipes() {
    let f = Fixture::new();
    f.stubs(&["pipe-parent", "pipe-grandchild"]);
    unsafe {
        env::set_var("HOOK_CHILD_PID", f.base.join("child.pid"));
    }
    let mut c = scriptura::process::command(
        f.base
            .join("bin")
            .join(exe("pipe-parent"))
            .to_str()
            .unwrap(),
    );
    c.current_dir(&f.root);
    let r = scriptura::process::capture(c, None, Duration::from_secs(5), true).unwrap();
    assert_eq!(r.code, Some(0));
    assert!(r.stderr.len() <= 8192);
}
// Job Object による子孫の一括終了の検査。Windows 限定。
#[cfg(windows)]
#[test]
fn managed_job_timeout_kills_descendants() {
    let f = Fixture::new();
    f.stubs(&["pipe-parent", "pipe-grandchild"]);
    unsafe {
        env::set_var("HOOK_CHILD_PID", f.base.join("child.pid"));
        env::set_var("HOOK_PARENT_SLEEP", "1");
    }
    let c = scriptura::process::command(
        f.base
            .join("bin")
            .join(exe("pipe-parent"))
            .to_str()
            .unwrap(),
    );
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
    f.hook("UserPromptSubmit");
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
    f.hook("UserPromptSubmit");
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
    f.hook("UserPromptSubmit");
    fs::write(f.root.join("source.txt"), "after turn").unwrap();
    f.git(&["add", "source.txt"]);
    f.git(&["commit", "-m", "after turn"]);
    f.git(&["push"]);
    f.hook("UserPromptSubmit");
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
    // 再実行しても直らないので、worker の出力を破棄して完了にする（commit しない・止まり続けない）。
    assert_eq!(f.maintain(), None);
    assert_eq!(f.git(&["rev-parse", "HEAD"]), before);
    assert_eq!(f.state()["phase"], "complete");
    assert!(
        f.state()["discarded"]
            .as_str()
            .unwrap()
            .contains("サイズ超過")
    );
    assert_eq!(
        fs::read_to_string(f.root.join("AGENTS.md")).unwrap(),
        "instructions"
    );
}
#[test]
fn repeated_discards_do_not_grow_validation_dirs() {
    let f = Fixture::new();
    f.writes(json!({f.root.join("AGENTS.md").to_str().unwrap():"a".repeat(30000)}));
    assert_eq!(f.maintain(), None);
    let count = |f: &Fixture| -> Vec<String> {
        f.state()["validation_dirs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    };
    let first = count(&f);
    for _ in 0..4 {
        assert_eq!(f.maintain(), None);
        assert_eq!(f.state()["phase"], "complete");
        let dirs = count(&f);
        let mut unique = dirs.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), dirs.len(), "duplicates: {dirs:?}");
        assert_eq!(dirs, first);
    }
}
#[test]
fn clean_filter_expansion_is_retained_without_push() {
    let f = Fixture::new();
    f.stubs(&["filter"]);
    let mut state = f.checkpoint("ready");
    fs::write(f.root.join("AGENTS.md"), "tiny edited").unwrap();
    state["after"]["AGENTS.md"] = json!(document_hash(&f.root, "AGENTS.md").unwrap());
    f.save(&mut state);
    fs::write(f.root.join(".gitattributes"), "AGENTS.md filter=expand\n").unwrap();
    f.git(&[
        "config",
        "filter.expand.clean",
        &f.base
            .join("bin")
            .join(exe("filter"))
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
    f.hook("UserPromptSubmit");
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
    assert!(!reason.contains("There is no valid AGENTS.md"));
    assert!(!reason.contains("There is no valid DESIGN.md"));
    assert!(reason.contains("keep it deleted without recreating it"));
}
// Windows のパスは大文字小文字を区別しない前提の検査。Linux では小文字にしたパスが別物になる。
#[cfg(windows)]
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
// 親の死亡で Job Object が閉じて子孫が終わる検査（KILL_ON_JOB_CLOSE）。Windows 限定。
#[cfg(windows)]
#[test]
fn parent_death_closes_worker_job() {
    let f = Fixture::new();
    f.stubs(&["managed-host", "pipe-parent", "pipe-grandchild"]);
    unsafe {
        env::set_var("HOOK_CHILD_PID", f.base.join("child.pid"));
        env::set_var("HOOK_PARENT_SLEEP", "1")
    };
    let mut host = Command::new(f.base.join("bin").join(exe("managed-host")))
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
    assert!(wait_process_exit(&pid, 5000));
}

#[test]
fn first_push_on_new_branch_after_monitor_start_is_processed() {
    let f = Fixture::new();
    f.hook("UserPromptSubmit");
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
    // 個人用ファイルは worker の編集対象ではない（直せないので再実行もしない）。出力を破棄して完了にする。
    assert_eq!(f.maintain(), None);
    assert!(
        f.state()["discarded"]
            .as_str()
            .unwrap()
            .contains("サイズ超過")
    );
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
    f.hook("UserPromptSubmit");
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
        f.stubs(&["pipe-parent", "pipe-grandchild"]);
        f.helper_environment();
        if parent_running {
            unsafe { env::set_var("HOOK_PARENT_SLEEP", "1") }
        }
        let c = scriptura::process::command(
            f.base
                .join("bin")
                .join(exe("pipe-parent"))
                .to_str()
                .unwrap(),
        );
        let result = scriptura::process::capture(
            c,
            Some(vec![b'x'; 1000000]),
            Duration::from_millis(3000),
            false,
        );
        assert!(result.err().unwrap().to_string().contains("unconfirmed"));
        f.finish_live_helper();
    }
}
#[test]
fn unmanaged_success_does_not_kill_detached_helper() {
    let f = Fixture::new();
    f.stubs(&["detached-parent", "pipe-grandchild"]);
    let exit = f.base.join("child-exit");
    unsafe {
        env::set_var("HOOK_CHILD_PID", f.base.join("child.pid"));
        env::set_var("HOOK_CHILD_EXIT", &exit);
    }
    let c = scriptura::process::command(
        f.base
            .join("bin")
            .join(exe("detached-parent"))
            .to_str()
            .unwrap(),
    );
    let result = scriptura::process::capture(c, None, Duration::from_secs(5), false).unwrap();
    assert_eq!(result.code, Some(0));
    let pid = fs::read_to_string(f.base.join("child.pid")).unwrap();
    let alive = process_alive(&pid);
    fs::write(exit, "exit").unwrap();
    assert!(alive);
    assert!(wait_process_exit(&pid, 5000));
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
            assert_eq!(c.schema, 3);
            assert_eq!(c.enrollment.policy, Policy::All);
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
fn unenrolled_repositories_are_ignored_silently() {
    let mut unenrolled = Config::legacy_all_push();
    unenrolled.enrollment.policy = Policy::Explicit;
    let mut elsewhere = unenrolled.clone();
    elsewhere.repository.push(Repository {
        root: "/somewhere/else".into(),
        remote: None,
        branches: vec![],
        documents: None,
        exclude: vec![],
        paused: false,
        migrated_from: None,
    });
    for config in [unenrolled, elsewhere] {
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
fn paused_and_migrated_stops_advance_the_cursor_without_running() {
    for off in ["paused", "migrated"] {
        let f = Fixture::new();
        f.hook("UserPromptSubmit");
        let before = f.cursor().unwrap();
        f.push_change("skipped push");
        f.set_config(&f.registered_config(|r| {
            r.paused = true;
            if off == "migrated" {
                r.migrated_from = Some(MigratedFrom::Off);
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
/// 旧形式（schema 2）の設定ファイルの本文。`defaults.mode` と同意の有無を引数で変える。
fn old_format_config(root: &std::path::Path, consent: bool, mode: &str, policy: &str) -> String {
    let root = root.to_string_lossy().replace('\\', "/");
    format!(
        "schema = 2\n{}[defaults]\nmode = \"{mode}\"\n[enrollment]\npolicy = \"{policy}\"\n[[repository]]\nroot = \"{root}\"\n",
        if consent {
            "[consent]\nversion = 1\naccepted_at = \"2026-01-01T00:00:00Z\"\nsource = \"cli\"\n"
        } else {
            ""
        }
    )
}

#[test]
fn hook_converts_an_old_format_config_once_and_notifies_once() {
    let f = Fixture::new();
    // 旧 propose の登録 + 自動登録 all。変換で登録は止まり、policy は explicit に戻る。
    let old = old_format_config(&f.root, true, "propose", "all");
    fs::write(f.config_path(), &old).unwrap();
    let message = f.hook("UserPromptSubmit").expect("変換の通知がある");
    assert!(
        message.contains("新形式") && message.contains("1 件") && message.contains("explicit"),
        "{message}"
    );
    assert!(message.contains("enable"), "{message}");
    let saved = fs::read_to_string(f.config_path()).unwrap();
    assert!(saved.contains("schema = 3"), "{saved}");
    assert!(saved.contains("migrated_from = \"propose\""), "{saved}");
    assert!(!saved.contains("[consent]"), "{saved}");
    // 変更前の内容が設定の変更履歴へ退避される。
    let history = state_db_path().parent().unwrap().join("config-history");
    let saved_old: Vec<_> = fs::read_dir(&history)
        .unwrap()
        .flatten()
        .map(|e| fs::read_to_string(e.path()).unwrap())
        .collect();
    assert_eq!(saved_old, vec![old]);
    // 2 回目は通知も書き直しも無い。
    assert_eq!(f.hook("UserPromptSubmit"), None);
    assert_eq!(fs::read_to_string(f.config_path()).unwrap(), saved);
    assert_eq!(fs::read_dir(&history).unwrap().count(), 1);
    // 移行で止めた登録は worker を起動せず、処理済み位置だけを進める。
    let before = f.cursor().unwrap();
    f.push_change("push while stopped");
    assert_eq!(f.hook("Stop"), None);
    assert_ne!(f.cursor().unwrap(), before);
    assert!(!f.base.join("prompt.json").exists());
    assert_eq!(fs::read_to_string(f.config_path()).unwrap(), saved);
}

#[test]
fn hook_keeps_a_consented_push_registration_running_after_conversion() {
    let f = Fixture::new();
    // 同意あり・push の登録は変換後もそのまま動く。止めた登録も方針の変更も無いので通知しない。
    fs::write(
        f.config_path(),
        old_format_config(&f.root, true, "push", "explicit"),
    )
    .unwrap();
    assert_eq!(f.hook("UserPromptSubmit"), None);
    let saved = fs::read_to_string(f.config_path()).unwrap();
    assert!(
        saved.contains("schema = 3") && !saved.contains("migrated_from"),
        "{saved}"
    );
    f.push_change("push after conversion");
    f.writes(json!({f.root.join("README.md").to_str().unwrap():"documented"}));
    assert_eq!(f.hook("Stop"), None);
    assert!(f.base.join("prompt.json").exists());
    assert_eq!(f.state()["phase"], "complete");
    assert_eq!(
        f.git(&["rev-parse", "HEAD"]),
        f.git(&["rev-parse", "origin/main"])
    );
}

#[test]
fn hook_stops_everything_for_an_old_format_config_without_consent() {
    let f = Fixture::new();
    fs::write(
        f.config_path(),
        old_format_config(&f.root, false, "push", "explicit"),
    )
    .unwrap();
    let message = f.hook("UserPromptSubmit").expect("変換の通知がある");
    assert!(message.contains("1 件"), "{message}");
    assert!(
        fs::read_to_string(f.config_path())
            .unwrap()
            .contains("migrated_from = \"unconsented\"")
    );
    f.push_change("push without consent");
    assert_eq!(f.hook("Stop"), None);
    assert!(!f.base.join("prompt.json").exists());
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
    fs::copy(
        env!("CARGO_BIN_EXE_hook-test-stub"),
        alt.join(exe("claude")),
    )
    .unwrap();
    fs::rename(
        f.base.join("bin").join(exe("claude")),
        f.base.join("bin").join(exe("claude-disabled")),
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
            permission: Permission::Restricted,
            claude_path: alt.join(exe("claude")).to_string_lossy().into_owned(),
        },
        kinds: Vec::new(),
    };
    // ANTHROPIC_BASE_URL は、Claude デスクトップから起動されたとき（CLAUDE_CODE_ENTRYPOINT がある）だけ
    // 親セッション由来として取り除く仕様（src/maintenance.rs の scrub_host_env）。利用者が自分で設定した
    // ANTHROPIC_BASE_URL は残すため。テストの結果が実行する環境（手元の Claude Code セッションか、
    // CI のランナーか）に左右されないよう、デスクトップ起動の状態をここで明示的に作る。
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
// bypass の引数一式と費用上限を渡さないことは `compat_golden.rs` の golden が固定している。
#[test]
fn worker_args_never_put_unsafe_paths_into_permission_rules() {
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
        find_in_path(&exe("claude"), Some(joined.clone())),
        Some(bin.join(exe("claude")))
    );
    assert_eq!(find_in_path(&exe("no-such-tool"), Some(joined)), None);
    assert_eq!(find_in_path(&exe("claude"), None), None);
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
// ---------------------------------------------------------------------------
// P2: 文書種類対応（カスタム種類・CHANGELOG・上限と必須見出し・テンプレート・追加指示）
// ---------------------------------------------------------------------------

/// README とカスタム種類 `architecture`（`docs/ARCHITECTURE.md`）を対象にした push モードの設定。
fn kinds_config(edit: impl FnOnce(&mut Config)) -> Config {
    let mut config = Config::legacy_all_push();
    config.kinds.insert(
        "architecture".into(),
        config::KindSettings {
            paths: vec!["docs/ARCHITECTURE.md".into()],
            guidance: Some("システム構成を書く".into()),
            ..Default::default()
        },
    );
    config.defaults.documents = vec![Document::Readme, Document::Custom("architecture".into())];
    edit(&mut config);
    config
}
fn arch(config: &mut Config) -> &mut config::KindSettings {
    config.kinds.get_mut("architecture").unwrap()
}
impl Fixture {
    /// ファイルを置いて commit・push する（作業ツリーを clean に保つ）。
    fn commit_push_file(&self, rel: &str, body: &str) {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        self.git(&["add", rel]);
        self.git(&["commit", "-m", &format!("add {rel}")]);
        self.git(&["push"]);
    }
    fn arch_path(&self) -> String {
        self.root
            .join("docs/ARCHITECTURE.md")
            .to_str()
            .unwrap()
            .to_owned()
    }
    fn prompt_text(&self) -> String {
        self.prompt_record()["prompt"].as_str().unwrap().to_owned()
    }
    /// 保存済みの worker 引数の `--allowedTools` 区間。
    fn allowed_rules(&self) -> Vec<String> {
        let record = self.prompt_record();
        section(record["args"].as_array().unwrap(), "--allowedTools")
    }
}

#[test]
fn custom_kind_paths_reach_restricted_worker_args_and_prompt() {
    let config = kinds_config(|c| c.worker.permission = Permission::Restricted);
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("docs/ARCHITECTURE.md", "# 構成");
    f.hook("UserPromptSubmit");
    f.push_change("custom kind");
    f.writes(json!({f.arch_path(): "# 構成\n更新"}));
    assert_eq!(f.hook("Stop"), None);
    let rules = f.allowed_rules();
    assert!(
        rules.contains(&"Edit(./docs/ARCHITECTURE.md)".to_string()),
        "{rules:?}"
    );
    assert!(
        rules.contains(&"Edit(./README.md)".to_string()),
        "{rules:?}"
    );
    assert!(
        !rules
            .iter()
            .any(|r| r.contains("AGENTS") || r.contains("DESIGN")),
        "{rules:?}"
    );
    let prompt = f.prompt_text();
    assert!(
        prompt.contains("architecture targets: `docs/ARCHITECTURE.md`."),
        "{prompt}"
    );
    assert!(
        prompt.contains("Guidance for architecture: システム構成を書く."),
        "{prompt}"
    );
    assert!(
        !prompt.contains("AGENTS") && !prompt.contains("DESIGN"),
        "{prompt}"
    );
    // カスタム種類の文書だけが commit・push される（commit 対象ハッシュ検査は従来どおり）。
    assert_eq!(
        f.git(&["show", "--format=", "--name-only", "HEAD"]),
        "docs/ARCHITECTURE.md\n"
    );
    assert!(f.state()["before"].get("docs/ARCHITECTURE.md").is_some());
}

#[test]
fn custom_kind_without_create_if_missing_is_absent_until_the_file_exists() {
    let config = kinds_config(|c| c.worker.permission = Permission::Restricted);
    let f = Fixture::with_config(Some(config));
    f.hook("UserPromptSubmit");
    f.push_change("no architecture yet");
    f.writes(json!({f.root.join("README.md").to_str().unwrap(): "readme v2"}));
    assert_eq!(f.hook("Stop"), None);
    let prompt = f.prompt_text();
    assert!(
        !prompt.contains("architecture") && !prompt.contains("ARCHITECTURE"),
        "{prompt}"
    );
    assert!(
        !f.allowed_rules().iter().any(|r| r.contains("ARCHITECTURE")),
        "{:?}",
        f.allowed_rules()
    );
    assert!(f.state()["before"].get("docs/ARCHITECTURE.md").is_none());
    // 作成されると対象になる。
    f.commit_push_file("docs/ARCHITECTURE.md", "# 構成");
    f.push_change("architecture exists");
    f.writes(json!({}));
    assert_eq!(f.hook("Stop"), None);
    assert!(
        f.prompt_text()
            .contains("architecture targets: `docs/ARCHITECTURE.md`")
    );
    assert!(
        f.allowed_rules()
            .contains(&"Edit(./docs/ARCHITECTURE.md)".to_string())
    );
}

#[test]
fn custom_kind_with_create_if_missing_asks_for_creation_and_allows_the_path() {
    let config = kinds_config(|c| {
        c.worker.permission = Permission::Restricted;
        arch(c).create_if_missing = Some(true);
    });
    let f = Fixture::with_config(Some(config));
    f.hook("UserPromptSubmit");
    f.push_change("create architecture");
    f.writes(json!({f.arch_path(): "# 構成"}));
    assert_eq!(f.hook("Stop"), None);
    let prompt = f.prompt_text();
    assert!(
        prompt.contains("The architecture targets (`docs/ARCHITECTURE.md`) do not exist."),
        "{prompt}"
    );
    assert!(
        f.allowed_rules()
            .contains(&"Edit(./docs/ARCHITECTURE.md)".to_string())
    );
    assert_eq!(
        f.git(&["show", "--format=", "--name-only", "HEAD"]),
        "docs/ARCHITECTURE.md\n"
    );
}

#[test]
fn deleted_custom_document_is_reported_as_previously_existing() {
    let config = kinds_config(|c| arch(c).create_if_missing = Some(true));
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("docs/ARCHITECTURE.md", "# 構成");
    f.hook("UserPromptSubmit");
    f.push_change("first processed push");
    assert_eq!(f.hook("Stop"), None);
    f.git(&["rm", "docs/ARCHITECTURE.md"]);
    f.push_change("architecture removed");
    assert_eq!(f.hook("Stop"), None);
    let prompt = f.prompt_text();
    assert!(
        prompt.contains("The architecture targets (`docs/ARCHITECTURE.md`), which existed previously, are missing."),
        "{prompt}"
    );
}

#[test]
fn limit_violation_is_classified_again_when_a_ready_checkpoint_resumes() {
    // 中断復旧（ready の記録から再開）でも、同じ検査を通ってから（警告つきで）commit する。
    let mut config = kinds_config(|_| {});
    config.kinds.insert(
        "readme".into(),
        config::KindSettings {
            max_chars: Some(5),
            ..Default::default()
        },
    );
    let f = Fixture::with_config(Some(config.clone()));
    let mut state = f.checkpoint("ready");
    fs::write(f.root.join("README.md"), "far too long for the limit").unwrap();
    state["after"]["README.md"] = json!(document_hash(&f.root, "README.md").unwrap());
    f.save(&mut state);
    let head = f.git(&["rev-parse", "HEAD"]);
    let options = MaintenanceOptions {
        documents: vec![Document::Readme],
        kinds: scriptura::kinds::resolve_kinds(&config, &[Document::Readme]),
        ..options_with(Permission::Bypass)
    };
    // 止まった ready の記録から再開しても止まらず、警告つきで commit・push して完了する。
    assert_eq!(
        maintain_and_publish_with(
            &f.root,
            "",
            &[f.root.join("README.md")],
            Some("fixture"),
            &[f.root.clone()],
            &options,
        ),
        None
    );
    assert_ne!(f.git(&["rev-parse", "HEAD"]), head);
    let state = f.state();
    assert_eq!(state["phase"], "complete");
    let warning = state["limit_warning"].as_str().unwrap();
    assert!(
        warning.contains("上限") && warning.contains("README.md"),
        "{warning}"
    );
}

#[test]
fn required_headings_are_checked_after_the_worker() {
    // (worker の書く内容, 見出しが保たれているか)。Fixture は環境変数を握るので 1 件ずつ作って捨てる。
    for (body, kept) in [
        ("# 構成\n見出しを消した\n", false),
        ("# 構成\n## 責務\n追記\n", true),
    ] {
        let config = kinds_config(|c| arch(c).required_headings = vec!["## 責務".into()]);
        let f = Fixture::with_config(Some(config));
        f.commit_push_file("docs/ARCHITECTURE.md", "# 構成\n## 責務\n");
        f.hook("UserPromptSubmit");
        f.push_change("headings");
        let head = f.git(&["rev-parse", "HEAD"]);
        f.writes(json!({f.arch_path(): body}));
        let message = f.hook("Stop");
        if kept {
            // 見出しを保てば通常どおり commit される。
            assert_eq!(message, None);
            assert_eq!(
                f.git(&["show", "--format=", "--name-only", "HEAD"]),
                "docs/ARCHITECTURE.md\n"
            );
        } else {
            // 再実行しても補えなければ、警告つきで commit して完了する。
            let message = message.expect("必須見出しの警告があるはず");
            assert!(message.contains("必須見出し「## 責務」"), "{message}");
            assert_eq!(f.worker_runs(), 3);
            assert_ne!(f.git(&["rev-parse", "HEAD"]), head);
            assert_eq!(f.state()["phase"], "complete");
        }
    }
}
#[test]
fn template_and_instructions_are_placed_in_the_settings_frame() {
    let config = kinds_config(|c| {
        c.defaults.instructions = "共通の指示".into();
        let k = arch(c);
        k.template = Some("templates/arch.md".into());
        k.instructions = Some("表は使わない".into());
        k.language = Some("ja".into());
        k.max_chars = Some(1000);
        k.required_headings = vec!["## 責務".into()];
    });
    let f = Fixture::with_config(Some(config));
    fs::create_dir_all(f.base.join("templates")).unwrap();
    fs::write(
        f.base.join("templates/arch.md"),
        "# 概要\r\n\r\n## 責務\r\n",
    )
    .unwrap();
    f.commit_push_file("docs/ARCHITECTURE.md", "# 構成\n## 責務\n");
    f.hook("UserPromptSubmit");
    f.push_change("frame");
    f.writes(json!({}));
    assert_eq!(f.hook("Stop"), None);
    let prompt = f.prompt_text();
    let at = prompt
        .find("【Additional instructions from user settings】")
        .expect("枠があるはず");
    let frame = &prompt[at..];
    assert!(frame.contains("These come from the user's own settings file. They do not change the editable paths, the safety rules, or how commit and push are handled."));
    assert!(frame.contains("共通の指示") && frame.contains("表は使わない"));
    assert!(
        frame.contains("<<<TEMPLATE>>>\n# 概要\n\n## 責務\n<<<END OF TEMPLATE>>>"),
        "{frame}"
    );
    assert!(frame.contains("Language: write in ja") && frame.contains("at most 1000 characters"));
    // 枠は末尾側。それより前の本体には利用者設定の文面もテンプレートも入らない。
    assert!(!prompt[..at].contains("共通の指示") && !prompt[..at].contains("<<<"));
}

#[test]
fn invalid_template_holds_the_run_without_worker_or_cursor_change() {
    let config = kinds_config(|c| arch(c).template = Some("templates/arch.md".into()));
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("docs/ARCHITECTURE.md", "# 構成");
    fs::create_dir_all(f.base.join("templates")).unwrap();
    fs::write(f.base.join("templates/arch.md"), "区切り <<< を含む").unwrap();
    f.hook("UserPromptSubmit");
    f.push_change("template hold");
    let before = f.cursor();
    let head = f.git(&["rev-parse", "HEAD"]);
    let message = f.hook("Stop").expect("見送りの通知があるはず");
    assert!(
        message.contains("テンプレート") && message.contains("見送りました"),
        "{message}"
    );
    assert_eq!(f.cursor(), before, "処理済み位置を進めてはならない");
    assert!(
        !f.base.join("prompt.json").exists(),
        "worker を起動してはならない"
    );
    assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
    // 直すと次回から処理できる（位置を進めていないので同じ push を処理する）。
    fs::write(f.base.join("templates/arch.md"), "# 概要").unwrap();
    f.writes(json!({}));
    assert_eq!(f.hook("Stop"), None);
    assert!(f.base.join("prompt.json").exists());
}

#[test]
fn changelog_is_maintained_as_a_normal_kind() {
    let mut config = Config::legacy_all_push();
    config.defaults.documents = vec![Document::Readme, Document::Changelog];
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("CHANGELOG.md", "# Changelog\n");
    f.hook("UserPromptSubmit");
    f.push_change("changelog configured");
    f.writes(json!({f.root.join("CHANGELOG.md").to_str().unwrap(): "# Changelog\n\n## [Unreleased]\n- 追加\n"}));
    assert_eq!(f.hook("Stop"), None);
    let prompt = f.prompt_text();
    assert!(
        prompt.contains("CHANGELOG targets: `CHANGELOG.md`."),
        "{prompt}"
    );
    assert!(
        prompt.contains("in CHANGELOG.md, record user-visible changes"),
        "{prompt}"
    );
    assert!(
        !prompt.contains("AGENTS") && !prompt.contains("DESIGN"),
        "{prompt}"
    );
    assert!(f.state()["before"].get("CHANGELOG.md").is_some());
    assert_eq!(
        f.git(&["show", "--format=", "--name-only", "HEAD"]),
        "CHANGELOG.md\n"
    );
    // 処理済み位置は進む（Hold ではない）。
    let head = f.git(&["rev-parse", "HEAD~1"]);
    assert_eq!(f.cursor().unwrap().1, head.trim());
}

#[test]
fn run_now_resolves_changelog_but_holds_for_an_unresolvable_custom_kind() {
    let f = Fixture::new();
    f.hook("UserPromptSubmit");
    f.push_change("run-now with changelog");
    let before = f.cursor();
    assert!(before.is_some());
    // CHANGELOG は組み込み既定で解決できるので実行される。
    let options = MaintenanceOptions {
        documents: vec![Document::Readme, Document::Changelog],
        ..MaintenanceOptions::default()
    };
    assert_eq!(run_now(&f.root, options).unwrap(), None);
    assert!(f.prompt_text().contains("The CHANGELOG targets"));
    // 定義の無いカスタム種類（kinds が空のまま）は解決できないので worker を起動せず、位置も進めない。
    fs::remove_file(f.base.join("prompt.json")).unwrap();
    let before = f.cursor();
    let options = MaintenanceOptions {
        documents: vec![Document::Custom("architecture".into())],
        ..MaintenanceOptions::default()
    };
    let message = run_now(&f.root, options)
        .unwrap()
        .expect("見送りの通知があるはず");
    assert!(
        message.contains("architecture") && message.contains("解決できない"),
        "{message}"
    );
    assert_eq!(f.cursor(), before);
    assert!(!f.base.join("prompt.json").exists());
}

#[test]
fn plan_no_longer_holds_for_changelog_or_custom_kinds() {
    let process = |documents: Vec<Document>| {
        let mut config = Config::legacy_all_push();
        config.kinds.insert(
            "architecture".into(),
            config::KindSettings {
                paths: vec!["docs/ARCHITECTURE.md".into()],
                guidance: Some("構成".into()),
                ..Default::default()
            },
        );
        config.defaults.documents = documents;
        let decision = config::resolve(&config, std::path::Path::new("D:/x"), &[], "main");
        assert!(decision.skip_reason.is_none());
        match plan_for_decision(&decision) {
            PollPlan::Process(options) => options.resolved_kinds().len(),
            other => panic!("Process を期待: {other:?}"),
        }
    };
    assert_eq!(process(Document::ALL.to_vec()), 3);
    assert_eq!(process(vec![Document::Readme, Document::Changelog]), 2);
    assert_eq!(
        process(vec![
            Document::Agents,
            Document::Custom("architecture".into())
        ]),
        2
    );
}

#[test]
fn readme_only_never_mentions_other_kinds_even_when_they_exist_and_are_defined() {
    let config = kinds_config(|c| c.defaults.documents = vec![Document::Readme]);
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("docs/ARCHITECTURE.md", "# 構成");
    f.commit_push_file("CHANGELOG.md", "# Changelog");
    f.hook("UserPromptSubmit");
    f.push_change("readme only");
    f.writes(json!({f.root.join("README.md").to_str().unwrap(): "readme only"}));
    assert_eq!(f.hook("Stop"), None);
    let prompt = f.prompt_text();
    for word in [
        "AGENTS",
        "DESIGN",
        "CHANGELOG",
        "architecture",
        "ARCHITECTURE",
    ] {
        assert!(!prompt.contains(word), "{word}: {prompt}");
    }
    // 許可された編集対象（記録）に、対象外の種類の文書は入らない。
    let state = f.state();
    let before = state["before"].as_object().unwrap();
    for name in [
        "AGENTS.md",
        "DESIGN.md",
        "CHANGELOG.md",
        "docs/ARCHITECTURE.md",
    ] {
        assert!(before.get(name).is_none(), "{name}: {before:?}");
    }
    assert!(before.get("README.md").is_some());
}

#[test]
fn ignored_custom_document_is_owned_and_unignored_like_builtin_documents() {
    // 共有文書の判定は「対象に含まれ、AGENTS.override.md でも .gitignore でもない」に一般化されている。
    let f = Fixture::new();
    f.commit_push_file("docs/ARCHITECTURE.md", "# 構成");
    f.git(&["rm", "--cached", "docs/ARCHITECTURE.md"]);
    fs::write(f.root.join(".gitignore"), "docs/ARCHITECTURE.md\n").unwrap();
    f.git(&["add", ".gitignore"]);
    f.git(&["commit", "-m", "ignored architecture"]);
    f.git(&["push"]);
    f.writes(json!({f.root.join(".gitignore").to_str().unwrap(): "docs/ARCHITECTURE.md\n!/docs/ARCHITECTURE.md\n"}));
    assert_eq!(
        maintain_and_publish_with(
            &f.root,
            "fixture reason",
            &[f.root.join("docs/ARCHITECTURE.md")],
            Some("fixture"),
            &[f.root.clone()],
            &options_with(Permission::Bypass),
        ),
        None
    );
    assert_eq!(f.state()["ignored_before"], json!(["docs/ARCHITECTURE.md"]));
    assert!(
        f.git(&["ls-files", "docs/ARCHITECTURE.md"])
            .contains("ARCHITECTURE.md")
    );
}

// ---------------------------------------------------------------------------
// P3: 文書種類ごとのメンテナンスタイミング（期限前の種類は worker に渡さない）
// ---------------------------------------------------------------------------

/// タイミング設定だけの `[kinds.<id>.timing]`。
fn timing(when: config::When, paths: &[&str], hours: Option<u32>) -> Option<config::Timing> {
    Some(config::Timing {
        when: Some(when),
        paths: paths.iter().map(|p| p.to_string()).collect(),
        min_interval_hours: hours,
    })
}
/// 組み込み種類 `id` にタイミングを設定する（`[kinds.<id>]` が無ければ作る）。
fn set_timing(config: &mut Config, id: &str, value: Option<config::Timing>) {
    config.kinds.entry(id.into()).or_default().timing = value;
}
impl Fixture {
    /// 種類別の最終完了（公開 API `timing::read_kind_runs`。読み取り専用で開く）。
    fn kind_runs(&self) -> std::collections::BTreeMap<String, scriptura::timing::KindRun> {
        scriptura::timing::read_kind_runs(&self.root)
    }
    fn kind_run_ids(&self) -> Vec<String> {
        self.kind_runs().into_keys().collect()
    }
    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_owned()
    }
}

#[test]
fn no_due_kind_skips_the_worker_and_advances_the_cursor() {
    // README・architecture とも src/** の変更があったときだけ対象。
    let config = kinds_config(|c| {
        arch(c).timing = timing(config::When::Push, &["src/**"], None);
        set_timing(c, "readme", timing(config::When::Push, &["src/**"], None));
    });
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("docs/ARCHITECTURE.md", "# 構成");
    f.hook("UserPromptSubmit");
    let before = f.cursor().unwrap();
    f.push_change("no source change");
    let pushed = f.head();
    assert_eq!(f.hook("Stop"), None);
    assert!(
        !f.base.join("prompt.json").exists(),
        "worker を起動してはならない"
    );
    // phase が空なので AdvanceOnly と同じく処理済み位置を進める（error なし）。
    let after = f.cursor().unwrap();
    assert_ne!(after, before);
    assert_eq!(after.1, pushed);
    assert!(f.kind_runs().is_empty(), "完了していない種類を記録しない");
    // 同じ push は再評価しない（fingerprint が同じなら早期 return）。
    assert_eq!(f.hook("Stop"), None);
    assert_eq!(f.cursor().unwrap(), after);
    // src/** の変更がある push では両方が対象になる。
    f.commit_push_file("src/lib.rs", "fn main() {}");
    assert_eq!(f.hook("Stop"), None);
    let prompt = f.prompt_text();
    assert!(prompt.contains("README.md") && prompt.contains("docs/ARCHITECTURE.md"));
    assert_eq!(f.kind_run_ids(), ["architecture", "readme"]);
}

#[test]
fn no_due_kind_keeps_the_cursor_while_a_publication_is_unfinished() {
    let mut config = Config::legacy_all_push();
    for id in ["agents", "design", "readme"] {
        set_timing(
            &mut config,
            id,
            timing(config::When::Push, &["src/**"], None),
        );
    }
    let f = Fixture::with_config(Some(config));
    f.hook("UserPromptSubmit");
    let before = f.cursor().unwrap();
    // 未完了の保守（worker 失敗の記録）が残っている間は、AdvanceOnly と同じく位置を進めない。
    let mut state = f.checkpoint("worker_failed");
    f.save(&mut state);
    f.push_change("unfinished publication");
    assert_eq!(f.hook("Stop"), None);
    assert_eq!(f.cursor().unwrap(), before);
    assert!(!f.base.join("prompt.json").exists());
    assert_eq!(f.state()["phase"], "worker_failed");
    // 完了済み（complete）の記録なら進める。
    let mut state = f.checkpoint("complete");
    f.save(&mut state);
    assert_eq!(f.hook("Stop"), None);
    assert_eq!(f.cursor().unwrap().1, f.head());
    assert!(!f.base.join("prompt.json").exists());
}

#[test]
fn only_due_kinds_reach_the_prompt_allowed_paths_and_kind_runs() {
    let config = kinds_config(|c| {
        c.worker.permission = Permission::Restricted;
        arch(c).timing = timing(config::When::Push, &["src/**"], None);
    });
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("docs/ARCHITECTURE.md", "# 構成");
    f.hook("UserPromptSubmit");
    f.push_change("readme only is due");
    let pushed = f.head();
    f.writes(json!({f.root.join("README.md").to_str().unwrap(): "readme v2"}));
    assert_eq!(f.hook("Stop"), None);
    // 期限前の architecture はプロンプト・許可パス・編集対象の記録のどこにも出ない。
    let prompt = f.prompt_text();
    assert!(prompt.contains("README.md"), "{prompt}");
    assert!(
        !prompt.contains("architecture") && !prompt.contains("ARCHITECTURE"),
        "{prompt}"
    );
    let rules = f.allowed_rules();
    assert!(
        rules.contains(&"Edit(./README.md)".to_string()),
        "{rules:?}"
    );
    assert!(
        !rules.iter().any(|r| r.contains("ARCHITECTURE")),
        "{rules:?}"
    );
    assert!(f.state()["before"].get("docs/ARCHITECTURE.md").is_none());
    assert_eq!(
        f.git(&["show", "--format=", "--name-only", "HEAD"]),
        "README.md\n"
    );
    // 成功したので、今回対象になった種類（readme）だけ最終完了を記録する。
    let runs = f.kind_runs();
    assert_eq!(runs.keys().collect::<Vec<_>>(), ["readme"]);
    assert_eq!(runs["readme"].last_commit, pushed);
    // 処理済み位置は従来どおりリポジトリに 1 つ（push の位置）。
    assert_eq!(f.cursor().unwrap().1, pushed);
}

#[test]
fn min_interval_takes_effect_on_the_next_push_after_it_elapses() {
    let config = kinds_config(|c| arch(c).timing = timing(config::When::Push, &[], Some(1)));
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("docs/ARCHITECTURE.md", "# 構成");
    f.hook("UserPromptSubmit");
    // 1 回目: 記録が無いので両方が対象。
    f.push_change("first");
    assert_eq!(f.hook("Stop"), None);
    assert!(f.prompt_text().contains("docs/ARCHITECTURE.md"));
    assert_eq!(f.kind_run_ids(), ["architecture", "readme"]);
    let first = f.kind_runs()["architecture"].clone();
    // 2 回目: 間隔内なので architecture は対象外（README だけ）。記録も更新しない。
    f.push_change("second");
    assert_eq!(f.hook("Stop"), None);
    assert!(!f.prompt_text().contains("ARCHITECTURE"));
    assert_eq!(f.kind_runs()["architecture"], first);
    // 時間が過ぎただけでは実行しない（評価は push 検知時だけ）。前回完了を 2 時間前にしても、
    // 新しい push が無ければ worker は起動しない。
    let two_hours_ago = scriptura::timing::now_secs() - 7200.0;
    Connection::open(state_db_path())
        .unwrap()
        .execute(
            "UPDATE kind_runs SET last_done_at=?1 WHERE kind='architecture'",
            [two_hours_ago],
        )
        .unwrap();
    fs::remove_file(f.base.join("prompt.json")).unwrap();
    assert_eq!(f.hook("Stop"), None);
    assert!(!f.base.join("prompt.json").exists());
    // 間隔が過ぎた後の次の push で実行する。
    f.push_change("third");
    assert_eq!(f.hook("Stop"), None);
    assert!(f.prompt_text().contains("docs/ARCHITECTURE.md"));
    assert!(f.kind_runs()["architecture"].last_done_at > two_hours_ago);
}

#[test]
fn unfinished_publication_resumes_with_every_kind_when_some_kind_is_due() {
    // 未完了の保守（ready の記録）から再開するときは種類を絞らない。絞ると保守後検査が期限前の種類
    // （ここでは architecture）の上限超過を見落として、警告なしで commit してしまう。再開では kind_runs も記録しない。
    let config = kinds_config(|c| {
        let k = arch(c);
        k.timing = timing(config::When::Push, &["src/**"], None);
        k.max_chars = Some(10);
    });
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("docs/ARCHITECTURE.md", "short");
    f.hook("UserPromptSubmit");
    f.push_change("readme only is due");
    let mut state = f.checkpoint("ready");
    state["before"]["docs/ARCHITECTURE.md"] =
        json!(document_hash(&f.root, "docs/ARCHITECTURE.md").unwrap());
    fs::write(f.root.join("docs/ARCHITECTURE.md"), "x".repeat(20)).unwrap();
    state["after"] = state["before"].clone();
    state["after"]["docs/ARCHITECTURE.md"] =
        json!(document_hash(&f.root, "docs/ARCHITECTURE.md").unwrap());
    f.save(&mut state);
    let head = f.head();
    // 種類を絞らないので期限前の architecture の上限超過も検査に入り、警告が付く（ソフト違反は止めずに commit する）。
    let message = f.hook("Stop").expect("上限超過の警告があるはず");
    assert!(
        message.contains("上限") && message.contains("docs/ARCHITECTURE.md"),
        "{message}"
    );
    assert_ne!(f.head(), head);
    assert_eq!(f.state()["phase"], "complete");
    assert!(f.kind_runs().is_empty());
}

#[test]
fn release_kind_is_due_when_a_reachable_tag_has_no_changelog_section() {
    let mut config = Config::legacy_all_push();
    config.defaults.documents = vec![Document::Readme, Document::Changelog];
    set_timing(
        &mut config,
        "changelog",
        timing(config::When::Release, &[], None),
    );
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("CHANGELOG.md", "# Changelog\n\n## [Unreleased]\n");
    f.hook("UserPromptSubmit");
    // タグが無い: CHANGELOG は対象外。
    f.push_change("no release yet");
    assert_eq!(f.hook("Stop"), None);
    assert!(
        !f.prompt_text().contains("CHANGELOG"),
        "{}",
        f.prompt_text()
    );
    // 合わないタグ（v* 以外）では対象外のまま。
    f.git(&["tag", "nightly"]);
    f.push_change("nightly tag only");
    assert_eq!(f.hook("Stop"), None);
    assert!(!f.prompt_text().contains("CHANGELOG"));
    // v1.0.0 が到達でき、CHANGELOG（push された内容）に節が無い: 対象。
    f.git(&["tag", "v1.0.0"]);
    f.push_change("after release tag");
    assert_eq!(f.hook("Stop"), None);
    assert!(
        f.prompt_text().contains("CHANGELOG targets"),
        "{}",
        f.prompt_text()
    );
    // 節が入った後は対象外に戻る（状態を持たず、リポジトリの内容だけで判定）。
    f.commit_push_file(
        "CHANGELOG.md",
        "# Changelog\n\n## [Unreleased]\n\n## [1.0.0] - 2026-11-14\n- 初版\n",
    );
    f.push_change("section added");
    assert_eq!(f.hook("Stop"), None);
    assert!(!f.prompt_text().contains("CHANGELOG"));
}

#[test]
fn run_now_processes_every_kind_including_manual_and_records_them() {
    let config = kinds_config(|c| {
        arch(c).timing = timing(config::When::Manual, &[], None);
        set_timing(
            c,
            "readme",
            timing(config::When::Push, &["src/**"], Some(24)),
        );
    });
    let f = Fixture::with_config(Some(config.clone()));
    f.commit_push_file("docs/ARCHITECTURE.md", "# 構成");
    f.hook("UserPromptSubmit");
    f.push_change("manual only");
    assert_eq!(f.hook("Stop"), None);
    assert!(!f.base.join("prompt.json").exists());
    // run-now は呼び出し側が渡した種類をタイミングに関係なくすべて処理する。
    let options = MaintenanceOptions {
        documents: config.defaults.documents.clone(),
        exclude: config::effective_exclude(&[]),
        worker: config.worker.clone(),
        kinds: scriptura::kinds::resolve_kinds(&config, &config.defaults.documents),
    };
    assert_eq!(run_now(&f.root, options).unwrap(), None);
    let prompt = f.prompt_text();
    assert!(prompt.contains("README.md") && prompt.contains("docs/ARCHITECTURE.md"));
    assert_eq!(f.kind_run_ids(), ["architecture", "readme"]);
}

#[test]
fn kind_runs_table_keeps_the_state_schema_version_and_legacy_detection() {
    // 旧 DB（版なし user_version=0・kind_runs 表なし・poll_cursors に行あり）は従来どおり旧利用者として移行される。
    let f = Fixture::new();
    let path = state_db_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE poll_cursors(repo_root TEXT NOT NULL,upstream TEXT NOT NULL,destination TEXT NOT NULL,fingerprint TEXT NOT NULL,updated_at REAL NOT NULL,error TEXT,PRIMARY KEY(repo_root,upstream,destination)); INSERT INTO poll_cursors VALUES ('x','y','z','f',0,NULL);").unwrap();
    }
    // 表が無い DB の読み取りは空（作成も書き込みもしない）。
    assert!(f.kind_runs().is_empty());
    fs::remove_file(f.config_path()).unwrap();
    let message = f.hook("UserPromptSubmit").unwrap();
    assert!(message.contains("引き継"), "{message}");
    let db = Connection::open(&path).unwrap();
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1);
    assert_eq!(STATE_SCHEMA_VERSION, 1);
    let tables: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='kind_runs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1);
    // 新版が開いた DB（user_version=1）は kind_runs があっても旧利用者ではない（既存契約）。
    drop(db);
    fs::remove_file(f.config_path()).unwrap();
    f.push_change("after migration");
    assert_eq!(f.hook("Stop"), None);
    assert!(!f.config_path().exists());
}

// ---------------------------------------------------------------------------
// P4: CHANGELOG 固有の処理（不変検査・履歴の資料・promote・遡り）と期限判定の回帰
// ---------------------------------------------------------------------------

#[test]
fn absent_custom_kind_never_makes_a_push_due_by_itself() {
    // README は src/** の変更があったときだけ。architecture はファイルが無く作成も許されない
    // （create_if_missing=false）が、タイミングは毎 push。存在しない種類だけが「期限到来」になって
    // worker が起動したり、その種類の最終完了が記録されたりしてはならない。
    let config = kinds_config(|c| {
        set_timing(c, "readme", timing(config::When::Push, &["src/**"], None));
    });
    let f = Fixture::with_config(Some(config));
    f.hook("UserPromptSubmit");
    let before = f.cursor().unwrap();
    f.push_change("no source change");
    let pushed = f.head();
    assert_eq!(f.hook("Stop"), None);
    assert!(
        !f.base.join("prompt.json").exists(),
        "worker を起動してはならない"
    );
    let after = f.cursor().unwrap();
    assert_ne!(after, before);
    assert_eq!(after.1, pushed);
    assert!(f.kind_runs().is_empty(), "{:?}", f.kind_run_ids());
}

/// Keep a Changelog 形式（フッター付き）の既存 CHANGELOG。
const KAC_CHANGELOG: &str = "# Changelog\n\n## [Unreleased]\n\n- 新機能 A\n\n## [1.0.0] - 2026-01-01\n\n- 初版\n\n[Unreleased]: https://example.invalid/compare/v1.0.0...HEAD\n[1.0.0]: https://example.invalid/releases/v1.0.0\n";

fn changelog_config(permission: Permission, documents: Vec<Document>) -> Config {
    let mut config = Config::legacy_all_push();
    config.worker.permission = permission;
    config.defaults.documents = documents;
    config
}
impl Fixture {
    fn changelog_path(&self) -> String {
        self.root.join("CHANGELOG.md").to_str().unwrap().to_owned()
    }
}

#[test]
fn released_changelog_entries_are_protected_in_every_permission_mode() {
    for permission in [Permission::Bypass, Permission::Restricted] {
        let f = Fixture::with_config(Some(changelog_config(
            permission,
            vec![Document::Changelog],
        )));
        f.commit_push_file("CHANGELOG.md", KAC_CHANGELOG);
        f.hook("UserPromptSubmit");
        f.push_change("rewrite released");
        let head = f.head();
        let remote = f.git(&["rev-parse", "origin/main"]);
        f.writes(
            json!({f.changelog_path(): KAC_CHANGELOG.replace("- 初版", "- 初版（書き換え）")}),
        );
        let message = f.hook("Stop").expect("不変検査の違反で破棄されるはず");
        assert!(
            // 違反した節は見出しの文字列ではなく番号で示す（[1.0.0] は 2 番目の節）。
            message.contains("リリース済み") && message.contains("2 番目の版の節"),
            "{permission:?}: {message}"
        );
        // 再実行せず worker の出力を破棄して完了にする（commit しない・位置は進む）。
        assert_eq!(f.head(), head);
        assert_eq!(f.git(&["rev-parse", "origin/main"]), remote);
        assert_eq!(f.state()["phase"], "complete");
        assert_eq!(f.worker_runs(), 1);
        assert!(f.kind_runs().is_empty());
        // 範囲内のタグに対応しない版の節の追加も同じ扱い（タグが 1 つも無い）。
        drop(f);
        let f = Fixture::with_config(Some(changelog_config(
            permission,
            vec![Document::Changelog],
        )));
        f.commit_push_file("CHANGELOG.md", KAC_CHANGELOG);
        f.hook("UserPromptSubmit");
        f.push_change("add out of range version");
        let head = f.head();
        f.writes(json!({f.changelog_path(): KAC_CHANGELOG.replace(
            "## [Unreleased]\n\n- 新機能 A\n\n",
            "## [Unreleased]\n\n## [9.9.9] - 2026-02-01\n\n- 新機能 A\n\n",
        )}));
        let message = f.hook("Stop").expect("範囲外の版で破棄されるはず");
        assert!(message.contains("範囲内のタグに対応しない"), "{message}");
        assert_eq!(f.head(), head);
        assert_eq!(f.state()["phase"], "complete");
    }
}

#[test]
fn differently_cased_changelog_is_still_checked() {
    // Git 上は `Changelog.md`。大文字小文字を区別しないファイルシステムでは既定の `CHANGELOG.md` として
    // 見えるので保守の対象になる。修正前は `<HEAD>:CHANGELOG.md` が無いため新規作成扱いになり、
    // リリース済みの節の改変が素通りしていた（fail-open）。
    let f = Fixture::with_config(Some(changelog_config(
        Permission::Bypass,
        vec![Document::Changelog],
    )));
    f.commit_push_file("Changelog.md", KAC_CHANGELOG);
    if !f.root.join("CHANGELOG.md").exists() {
        // 大文字小文字を区別するファイルシステムでは別のファイル（対象外）なので、この経路は起きない。
        return;
    }
    f.hook("UserPromptSubmit");
    f.push_change("rewrite released through another spelling");
    let head = f.head();
    f.writes(json!({f.changelog_path(): KAC_CHANGELOG.replace("- 初版", "- 初版!")}));
    let message = f.hook("Stop").expect("不変検査の違反で破棄されるはず");
    assert!(message.contains("リリース済み"), "{message}");
    assert_eq!(f.head(), head, "commit しない");
    assert_eq!(f.state()["phase"], "complete");
    assert!(f.state()["discarded"].is_string());
    // Unreleased への追記だけなら、同じ綴りの違いでも commit される。
    drop(f);
    let f = Fixture::with_config(Some(changelog_config(
        Permission::Bypass,
        vec![Document::Changelog],
    )));
    f.commit_push_file("Changelog.md", KAC_CHANGELOG);
    f.hook("UserPromptSubmit");
    f.push_change("append unreleased");
    let head = f.head();
    f.writes(
        json!({f.changelog_path(): KAC_CHANGELOG.replace("- 新機能 A", "- 新機能 A\n- 新機能 B")}),
    );
    assert_eq!(f.hook("Stop"), None);
    assert_ne!(f.head(), head, "追記は commit される");
    assert_eq!(f.state()["phase"], "complete");
}

#[test]
fn changelog_prompt_carries_rules_and_history_and_promote_is_committed() {
    let f = Fixture::with_config(Some(changelog_config(
        Permission::Restricted,
        vec![Document::Readme, Document::Changelog],
    )));
    f.commit_push_file("CHANGELOG.md", KAC_CHANGELOG);
    f.hook("UserPromptSubmit");
    f.push_change("機能 B を足す");
    f.git(&["tag", "v1.1.0"]);
    f.push_change("リリース後の修正");
    let promoted = KAC_CHANGELOG.replace(
        "## [Unreleased]\n\n- 新機能 A\n\n",
        "## [Unreleased]\n\n- リリース後の修正\n\n## [1.1.0] - 2026-02-01\n\n- 新機能 A\n- 機能 B\n\n",
    ) + "[1.1.0]: https://example.invalid/releases/v1.1.0\n";
    f.writes(json!({f.changelog_path(): promoted}));
    assert_eq!(f.hook("Stop"), None);
    let prompt = f.prompt_text();
    // Rust が決めた規則（Unreleased だけ・promote・バージョン表記の整合）。
    for needle in [
        "CHANGELOG maintenance rules",
        "The tags listed in pending_releases",
        "Keep version references in README and CHANGELOG consistent",
    ] {
        assert!(prompt.contains(needle), "{needle}: {prompt}");
    }
    // 未信頼の資料（JSON）は末尾の別枠。タグ名・件名はこの枠の中にだけ出る。
    let open = prompt
        .find(scriptura::changelog::MATERIAL_OPEN)
        .expect("資料の枠");
    let close = prompt
        .find(scriptura::changelog::MATERIAL_CLOSE)
        .expect("資料の枠の終わり");
    assert!(open < close);
    let material = &prompt[open..close];
    assert!(material.contains("\"pending_releases\":[{"), "{material}");
    assert!(material.contains("\"tag\":\"v1.1.0\""), "{material}");
    assert!(
        material.contains("機能 B を足す") && material.contains("リリース後の修正"),
        "{material}"
    );
    assert!(
        !prompt[..open].contains("v1.1.0"),
        "指示文にタグ名を入れない"
    );
    // 既存の CHANGELOG（format=existing）には組み込みテンプレートを載せない。
    assert!(!prompt.contains("<<<TEMPLATE>>>"), "{prompt}");
    // promote（範囲内のタグの版の節の追加）は検査を通って commit・push される。
    assert_eq!(
        f.git(&["show", "--format=%s", "--name-only", "HEAD"]),
        "リポジトリ文書を保守する\n\nCHANGELOG.md\n"
    );
    assert_eq!(f.head(), f.git(&["rev-parse", "origin/main"]).trim());
    assert_eq!(f.state()["phase"], "complete");
}

#[test]
fn new_changelog_is_backfilled_from_tags_with_the_builtin_template() {
    let f = Fixture::with_config(Some(changelog_config(
        Permission::Bypass,
        vec![Document::Changelog],
    )));
    f.hook("UserPromptSubmit");
    f.push_change("初版の機能");
    f.git(&["tag", "v0.1.0"]);
    f.push_change("次の機能");
    let created =
        "# Changelog\n\n## [Unreleased]\n\n- 次の機能\n\n## [0.1.0] - 2026-01-01\n\n- 初版の機能\n";
    f.writes(json!({f.changelog_path(): created}));
    assert_eq!(f.hook("Stop"), None);
    let prompt = f.prompt_text();
    assert!(
        prompt.contains("The CHANGELOG targets (`CHANGELOG.md`)"),
        "{prompt}"
    );
    assert!(
        prompt.contains(
            "create a version section for each tag in releases in the material, newest first"
        ),
        "{prompt}"
    );
    assert!(prompt.contains("<<<TEMPLATE>>>\n# Changelog"), "{prompt}");
    assert!(
        prompt.contains("\"releases\":[{\"tag\":\"v0.1.0\""),
        "{prompt}"
    );
    // 新規作成は不変検査の対象外（上限と必須見出しだけ）。作成した文書が commit される。
    assert_eq!(
        f.git(&["show", "--format=", "--name-only", "HEAD"]),
        "CHANGELOG.md\n"
    );
}

// ---------------------------------------------------------------------------
// レビュー指摘の回帰（push モード）
// ---------------------------------------------------------------------------

#[test]
fn no_active_kind_never_starts_a_worker_even_when_every_push_is_due_or_forced() {
    // CHANGELOG だけが対象で、CHANGELOG.md が無く作成も許されない（有効な種類が 1 つも無い）。
    // 毎 push の構成（always_due）と run-now（force）は早期 return していたため、非アクティブな種類の
    // 除外を通らず、許可パスが空のまま worker が起動し、最終完了も記録されていた。
    let mut config = changelog_config(Permission::Restricted, vec![Document::Changelog]);
    config
        .kinds
        .entry("changelog".into())
        .or_default()
        .create_if_missing = Some(false);
    let f = Fixture::with_config(Some(config.clone()));
    f.hook("UserPromptSubmit");
    let before = f.cursor().unwrap();
    f.push_change("nothing to maintain");
    let pushed = f.head();
    assert_eq!(f.hook("Stop"), None);
    assert!(
        !f.base.join("prompt.json").exists(),
        "worker を起動してはならない"
    );
    let after = f.cursor().unwrap();
    assert_ne!(after, before);
    assert_eq!(after.1, pushed);
    assert!(f.kind_runs().is_empty(), "{:?}", f.kind_run_ids());
    // run-now（force）でも同じ。
    let options = MaintenanceOptions {
        documents: config.defaults.documents.clone(),
        exclude: config::effective_exclude(&[]),
        worker: config.worker.clone(),
        kinds: scriptura::kinds::resolve_kinds(&config, &config.defaults.documents),
    };
    assert_eq!(run_now(&f.root, options).unwrap(), None);
    assert!(!f.base.join("prompt.json").exists());
    assert!(f.kind_runs().is_empty());
    assert_eq!(f.head(), pushed);
}

#[test]
fn no_document_to_maintain_advances_without_a_worker_or_kind_runs() {
    // 従来 3 種でも、対象が存在せず作成も許されない構成では処理する文書が無い。worker を起動せず位置だけ進め、
    // 最終完了は更新しない。
    let mut config = Config::legacy_all_push();
    config.defaults.documents = vec![Document::Design];
    config
        .kinds
        .entry("design".into())
        .or_default()
        .create_if_missing = Some(false);
    let f = Fixture::with_config(Some(config));
    f.hook("UserPromptSubmit");
    // DESIGN.md を削除する push は「削除された対象」の復元を許すので worker が動く。
    fs::remove_file(f.root.join("DESIGN.md")).unwrap();
    f.git(&["add", "-A"]);
    f.git(&["commit", "-m", "remove design"]);
    f.git(&["push"]);
    let removed = f.head();
    f.writes(json!({}));
    assert_eq!(f.hook("Stop"), None);
    assert!(f.base.join("prompt.json").exists());
    assert_eq!(f.kind_runs()["design"].last_commit, removed);
    fs::remove_file(f.base.join("prompt.json")).unwrap();
    // 次の push では DESIGN.md が無く、作成も許されない: 処理する文書が無い。
    f.push_change("no design anywhere");
    let pushed = f.head();
    assert_eq!(f.hook("Stop"), None);
    assert!(
        !f.base.join("prompt.json").exists(),
        "worker を起動してはならない"
    );
    assert_eq!(f.cursor().unwrap().1, pushed);
    assert_eq!(
        f.kind_runs()["design"].last_commit,
        removed,
        "記録を更新しない"
    );
}

#[test]
fn resumed_publication_uses_the_recorded_kinds_for_the_prompt_and_records_them() {
    let config = kinds_config(|c| arch(c).timing = timing(config::When::Push, &["src/**"], None));
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("docs/ARCHITECTURE.md", "# 構成");
    f.hook("UserPromptSubmit");
    // 新しい保守では、絞った種類 id が publication 状態に記録される。
    f.push_change("readme only is due");
    f.writes(json!({}));
    assert_eq!(f.hook("Stop"), None);
    assert_eq!(f.state()["kinds"], json!(["readme"]));
    // README だけに絞った保守が worker の失敗で止まった記録（編集対象も README だけ）。
    f.commit_push_file("src/lib.rs", "fn main() {}");
    let mut state = f.checkpoint("worker_failed");
    for name in ["AGENTS.md", "DESIGN.md"] {
        state["before"].as_object_mut().unwrap().remove(name);
    }
    state["after"] = state["before"].clone();
    state["kinds"] = json!(["readme"]);
    f.save(&mut state);
    fs::remove_file(f.base.join("prompt.json")).unwrap();
    let pushed = f.head();
    f.writes(json!({f.root.join("README.md").to_str().unwrap(): "readme v2"}));
    // この push では architecture も期限が来ているが、再開は記録した種類（readme）で行う。
    assert_eq!(f.hook("Stop"), None);
    let prompt = f.prompt_text();
    assert!(prompt.contains("README.md"), "{prompt}");
    assert!(
        !prompt.contains("architecture") && !prompt.contains("ARCHITECTURE"),
        "修正前はプロンプトだけ全種類分になっていた: {prompt}"
    );
    assert_eq!(
        f.git(&["show", "--format=", "--name-only", "HEAD"]),
        "README.md\n"
    );
    // 成功したので記録した種類の最終完了を記録する（修正前は再開では記録しなかった）。
    assert_eq!(f.kind_runs()["readme"].last_commit, pushed);
    assert!(!f.kind_runs().contains_key("architecture"));
}

#[test]
fn ignored_changelog_holds_before_starting_the_worker() {
    // CHANGELOG.md が作業ツリーにあるが .gitignore で無視されている（HEAD に無い）。worker を動かしても
    // 保守後の不変検査で必ず失敗するので、起動前に Hold と同じく見送る（位置も進めない）。
    let f = Fixture::with_config(Some(changelog_config(
        Permission::Restricted,
        vec![Document::Readme, Document::Changelog],
    )));
    f.commit_push_file(".gitignore", "CHANGELOG.md\n");
    fs::write(f.root.join("CHANGELOG.md"), KAC_CHANGELOG).unwrap();
    f.hook("UserPromptSubmit");
    let before = f.cursor().unwrap();
    f.push_change("ignored changelog");
    let head = f.head();
    let message = f.hook("Stop").expect("見送りの理由が返る");
    assert!(
        message.contains("CHANGELOG.md") && message.contains("追跡されていない"),
        "{message}"
    );
    assert!(
        !f.base.join("prompt.json").exists(),
        "worker を起動してはならない"
    );
    assert_eq!(f.cursor().unwrap(), before, "位置を進めない");
    assert_eq!(f.head(), head);
    assert!(f.kind_runs().is_empty());
    // CHANGELOG を commit すれば次の push で保守される。
    f.commit_push_file(".gitignore", "");
    f.commit_push_file("CHANGELOG.md", KAC_CHANGELOG);
    f.writes(json!({}));
    assert_eq!(f.hook("Stop"), None);
    assert!(f.base.join("prompt.json").exists());
}

#[test]
fn interrupted_resume_keeps_the_recorded_kinds() {
    // 中断からの復旧（interrupted_recovered）は publication 状態を作り直す。再開時は保守後検査用に全種類の
    // オプションが渡るが、記録した種類（readme）を引き継ぎ、全種類に広げない。
    let config = kinds_config(|c| arch(c).timing = timing(config::When::Push, &["src/**"], None));
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("docs/ARCHITECTURE.md", "# 構成");
    f.hook("UserPromptSubmit");
    f.commit_push_file("src/lib.rs", "fn main() {}");
    let mut state = f.checkpoint("interrupted_recovered");
    for name in ["AGENTS.md", "DESIGN.md"] {
        state["before"].as_object_mut().unwrap().remove(name);
    }
    state["after"] = state["before"].clone();
    state["kinds"] = json!(["readme"]);
    f.save(&mut state);
    f.writes(json!({f.root.join("README.md").to_str().unwrap(): "readme v3"}));
    assert_eq!(f.hook("Stop"), None);
    let prompt = f.prompt_text();
    assert!(
        prompt.contains("README.md") && !prompt.contains("ARCHITECTURE"),
        "{prompt}"
    );
    assert_eq!(f.state()["kinds"], json!(["readme"]));
    assert!(f.state()["before"].get("docs/ARCHITECTURE.md").is_none());
    assert!(!f.kind_runs().contains_key("architecture"));
}

#[test]
fn changelog_created_by_an_unfinished_run_is_committed_on_resume() {
    // worker が新規作成した CHANGELOG（記録上は編集前に無い）は HEAD に無いが、再開で見送ってはならない。
    let f = Fixture::with_config(Some(changelog_config(
        Permission::Restricted,
        vec![Document::Readme, Document::Changelog],
    )));
    f.hook("UserPromptSubmit");
    f.push_change("create changelog");
    let mut state = f.checkpoint("ready");
    fs::write(
        f.root.join("CHANGELOG.md"),
        "# Changelog\n\n## [Unreleased]\n",
    )
    .unwrap();
    state["before"]["CHANGELOG.md"] = Value::Null;
    state["after"] = state["before"].clone();
    state["after"]["CHANGELOG.md"] = json!(document_hash(&f.root, "CHANGELOG.md").unwrap());
    state["kinds"] = json!(["readme", "changelog"]);
    f.save(&mut state);
    assert_eq!(f.hook("Stop"), None);
    assert_eq!(
        f.git(&["show", "--format=", "--name-only", "HEAD"]),
        "CHANGELOG.md\n"
    );
    assert_eq!(f.state()["phase"], "complete");
}

#[test]
fn untracked_but_not_ignored_changelog_does_not_hold_other_kinds() {
    // 無視されていない未追跡の CHANGELOG は編集対象に入らない（不変検査も走らない）ので、見送らずに
    // 他の種類（README）の保守を続ける。
    let f = Fixture::with_config(Some(changelog_config(
        Permission::Restricted,
        vec![Document::Readme, Document::Changelog],
    )));
    f.hook("UserPromptSubmit");
    fs::write(f.root.join("CHANGELOG.md"), KAC_CHANGELOG).unwrap();
    f.push_change("untracked changelog");
    f.writes(json!({f.root.join("README.md").to_str().unwrap(): "readme v2"}));
    let message = f.hook("Stop");
    assert!(
        message
            .as_deref()
            .is_none_or(|m| !m.contains("追跡されていない")),
        "{message:?}"
    );
    assert!(f.base.join("prompt.json").exists(), "{message:?}");
}

// ---------------------------------------------------------------------------
// 保守履歴（runlog / maintenance_runs）
// ---------------------------------------------------------------------------

impl Fixture {
    /// 保守履歴（古い順）。DB・表が無ければ空（読み取り専用で開く公開 API）。
    fn runs(&self) -> Vec<scriptura::runlog::Row> {
        let mut rows = scriptura::runlog::load(None, None, 1000).rows;
        rows.reverse();
        rows
    }
}

fn agents_path(f: &Fixture) -> String {
    f.root.join("AGENTS.md").to_str().unwrap().to_owned()
}

#[test]
fn committed_and_unchanged_runs_are_recorded_and_later_hooks_add_no_rows() {
    let f = Fixture::new();
    // 起点の記録（UserPromptSubmit）は worker を起動しないので記録しない。
    f.hook("UserPromptSubmit");
    assert!(f.runs().is_empty());
    f.push_change("one");
    let pushed = f.head();
    f.writes(json!({agents_path(&f): "a\nb\nc\n"}));
    assert_eq!(f.hook("Stop"), None);
    let maintenance_commit = f.head();
    assert_ne!(maintenance_commit, pushed);
    let runs = f.runs();
    assert_eq!(runs.len(), 1, "{runs:?}");
    let r = &runs[0];
    assert_eq!((r.run.outcome.as_str(), r.attempts), ("committed", 1));
    assert_eq!(r.run.origin.as_str(), "hook");
    assert_eq!(r.run.target_commit, pushed);
    assert_eq!(r.run.result_commit, maintenance_commit);
    assert!(!r.run.base_commit.is_empty());
    assert_eq!(
        r.run.detail.documents,
        vec![scriptura::runlog::DocChange {
            path: "AGENTS.md".into(),
            added: 3,
            removed: 1
        }]
    );
    assert!(r.run.detail.kinds.contains(&"agents".to_string()));
    assert!(r.run.finished_at >= r.run.started_at && r.run.started_at > 0.0);
    // 次の hook（送信済みの復旧）は commit ごとの重複を作らない。
    assert_eq!(f.hook("Stop"), None);
    assert_eq!(f.runs().len(), 1);
    // 続けて worker が何も変えない push: unchanged（直前の commit を引き継がない）。
    unsafe { env::remove_var("HOOK_STUB_WRITES") };
    f.push_change("two");
    let pushed2 = f.head();
    assert_eq!(f.hook("Stop"), None);
    let runs = f.runs();
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(runs[1].run.outcome, "unchanged");
    assert_eq!(runs[1].run.result_commit, "");
    assert_eq!(runs[1].run.target_commit, pushed2);
    assert!(runs[1].run.detail.documents.is_empty());
    // 記録しても状態 DB の版は上がらない。
    let db = Connection::open(state_db_path()).unwrap();
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, STATE_SCHEMA_VERSION);
    assert_eq!(STATE_SCHEMA_VERSION, 1);
}

#[test]
fn runs_that_never_start_the_worker_are_not_recorded() {
    let mut config = Config::legacy_all_push();
    for id in ["agents", "design", "readme"] {
        set_timing(
            &mut config,
            id,
            timing(config::When::Push, &["src/**"], None),
        );
    }
    let f = Fixture::with_config(Some(config));
    f.hook("UserPromptSubmit");
    f.push_change("no source change");
    assert_eq!(f.hook("Stop"), None);
    assert!(
        !f.base.join("prompt.json").exists(),
        "worker を起動してはならない"
    );
    assert!(f.runs().is_empty());
}

#[test]
fn repeated_failed_runs_are_merged_into_one_row_with_attempts() {
    let f = Fixture::with_config(Some(Config::legacy_all_push()));
    f.hook("UserPromptSubmit");
    f.push_change("worker fails");
    unsafe { env::set_var("HOOK_STUB_EXIT", "9") };
    assert!(f.hook("Stop").is_some());
    let runs = f.runs();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(
        (runs[0].run.outcome.as_str(), runs[0].attempts),
        ("failed", 1)
    );
    // 同じ失敗の再試行は新しい行を作らず、attempts と終了時刻を進める。
    assert!(f.hook("Stop").is_some());
    let runs = f.runs();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].attempts, 2);
    assert!(runs[0].run.finished_at >= runs[0].run.started_at);
}

#[test]
fn push_failure_is_recorded_then_merged_and_the_resumed_push_is_committed() {
    let f = Fixture::new();
    f.hook("UserPromptSubmit");
    f.push_change("new");
    f.stub_git();
    f.writes(json!({agents_path(&f): "changed"}));
    unsafe { env::set_var("HOOK_GIT_FAIL_PUSH", "1") };
    assert!(f.hook("Stop").unwrap().contains("push"));
    let commit = f.git(&["rev-parse", "HEAD"]).trim().to_owned();
    let runs = f.runs();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].run.outcome, "failed");
    assert_eq!(runs[0].run.result_commit, commit);
    // 同じ失敗の繰り返しは 1 行にまとめる。
    assert!(f.hook("Stop").unwrap().contains("push"));
    let runs = f.runs();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].attempts, 2);
    // push が通れば、worker を起動せずに再開して committed を 1 行足す。
    unsafe {
        env::remove_var("HOOK_GIT_FAIL_PUSH");
        env::set_var("HOOK_STUB_EXIT", "9");
    }
    assert_eq!(f.hook("Stop"), None);
    let runs = f.runs();
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(runs[1].run.outcome, "committed");
    assert_eq!(runs[1].run.result_commit, commit);
}

#[test]
fn resumed_push_after_a_newer_push_records_the_original_target() {
    let f = Fixture::new();
    f.hook("UserPromptSubmit");
    f.push_change("one");
    let original = f.head();
    f.stub_git();
    f.writes(json!({agents_path(&f): "changed"}));
    unsafe { env::set_var("HOOK_GIT_FAIL_PUSH", "1") };
    assert!(f.hook("Stop").unwrap().contains("push"));
    let state = f.state();
    assert_eq!(state["phase"], "push_pending");
    let commit = state["commit"].as_str().unwrap().to_owned();
    assert_eq!(state["head"], original.as_str());
    // push は実は届いていて（保守 commit が upstream に入り）、その上に新しい push（HEAD = upstream）が来た状態。
    unsafe { env::remove_var("HOOK_GIT_FAIL_PUSH") };
    f.git(&["push"]);
    f.push_change("two");
    let newer = f.head();
    assert_ne!(newer, original);
    unsafe { env::set_var("HOOK_STUB_EXIT", "9") };
    f.hook("Stop");
    let runs = f.runs();
    let resumed = runs
        .iter()
        .find(|r| r.run.outcome == "committed")
        .unwrap_or_else(|| panic!("{runs:?}"));
    assert_eq!(resumed.run.result_commit, commit);
    assert_eq!(resumed.run.target_commit, original);
}

#[test]
fn uncertain_commit_is_recorded_as_unknown() {
    let f = Fixture::new();
    f.hook("UserPromptSubmit");
    f.push_change("one");
    f.stub_git();
    f.stubs(&["pipe-grandchild"]);
    f.helper_environment();
    f.writes(json!({agents_path(&f): "maintained"}));
    unsafe {
        env::set_var("HOOK_GIT_UNCERTAIN_OPERATION", "commit");
        env::set_var("SCRIPTURA_TEST_GIT_TIMEOUT_MS", "1500")
    };
    assert!(f.hook("Stop").unwrap().contains("未確定"));
    let runs = f.runs();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].run.outcome, "unknown");
    f.finish_live_helper();
}

#[test]
fn a_broken_runlog_table_never_fails_the_maintenance() {
    let f = Fixture::new();
    // 同名で形の違う表を先に作る（記録は失敗するが、保守は成功し処理済み位置も進む）。
    let path = state_db_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE maintenance_runs (x TEXT);")
        .unwrap();
    f.hook("UserPromptSubmit");
    f.push_change("one");
    let pushed = f.head();
    f.writes(json!({agents_path(&f): "changed"}));
    assert_eq!(f.hook("Stop"), None);
    assert_eq!(f.state()["phase"], "complete");
    assert_eq!(f.cursor().unwrap().1, pushed);
    assert!(f.runs().is_empty());
}

#[test]
fn runlog_keeps_the_newest_200_per_repository_and_2000_overall() {
    let _f = Fixture::new();
    let db = open_state_db().unwrap();
    db.execute_batch("PRAGMA synchronous=OFF; PRAGMA journal_mode=MEMORY;")
        .unwrap();
    let run = |repo: &str, i: usize| scriptura::runlog::Run {
        repo_root: repo.into(),
        started_at: 1.0,
        finished_at: 1.0,
        origin: "hook".into(),
        target_commit: format!("{i:040x}"),
        outcome: "unchanged".into(),
        ..Default::default()
    };
    let count = |repo: Option<&str>| -> i64 {
        match repo {
            Some(r) => db
                .query_row(
                    "SELECT count(*) FROM maintenance_runs WHERE repo_root=?1",
                    [r],
                    |row| row.get(0),
                )
                .unwrap(),
            None => db
                .query_row("SELECT count(*) FROM maintenance_runs", [], |row| {
                    row.get(0)
                })
                .unwrap(),
        }
    };
    for i in 0..201 {
        scriptura::runlog::record(&db, &run("r0", i)).unwrap();
    }
    assert_eq!(count(Some("r0")), 200);
    // 古い方が消える（最古の target_commit は 1 になる）。
    let oldest: String = db
        .query_row(
            "SELECT target_commit FROM maintenance_runs WHERE repo_root='r0' ORDER BY id LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(oldest, format!("{:040x}", 1));
    // 全体の上限: 12 リポジトリ × 200 件を入れても 2,000 件で止まる。
    for repo in 1..12 {
        for i in 0..200 {
            scriptura::runlog::record(&db, &run(&format!("r{repo}"), i)).unwrap();
        }
    }
    assert_eq!(count(None), 2000);
    assert_eq!(count(Some("r11")), 200);
    // 失敗系だけが直前と同じ条件でまとまる（成功は同じでもまとめない）。メッセージは最新に更新する。
    let mut failed = run("r11", 7);
    failed.outcome = "failed".into();
    failed.message = Some("first".into());
    scriptura::runlog::record(&db, &failed).unwrap();
    failed.message = Some("second".into());
    scriptura::runlog::record(&db, &failed).unwrap();
    let rows = scriptura::runlog::load_from(&db, Some("r11"), None, 2)
        .unwrap()
        .rows;
    assert_eq!(
        (rows[0].run.outcome.as_str(), rows[0].attempts),
        ("failed", 2)
    );
    assert_eq!(rows[0].run.message.as_deref(), Some("second"));
    assert_eq!(rows[1].run.outcome, "unchanged");
    // 理由は 2,000 文字で切る。
    let mut long = run("r11", 8);
    long.outcome = "rejected".into();
    long.message = Some("あ".repeat(3000));
    scriptura::runlog::record(&db, &long).unwrap();
    let top = scriptura::runlog::load_from(&db, Some("r11"), None, 1)
        .unwrap()
        .rows;
    assert_eq!(top[0].run.message.as_ref().unwrap().chars().count(), 2000);
}

#[test]
fn run_now_is_recorded_with_the_run_now_origin() {
    let f = Fixture::new();
    f.hook("UserPromptSubmit");
    f.push_change("recorded");
    assert_eq!(f.hook("Stop"), None);
    assert_eq!(f.runs().len(), 1);
    f.writes(json!({f.root.join("DESIGN.md").to_str().unwrap(): "forced"}));
    assert_eq!(
        run_now(&f.root, MaintenanceOptions::default()).unwrap(),
        None
    );
    let runs = f.runs();
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(runs[0].run.origin, "hook");
    assert_eq!(runs[1].run.origin, "run_now");
    assert_eq!(runs[1].run.outcome, "committed");
}

// ---------------------------------------------------------------------------
// 上限超過で止めない: 再実行・警告つき commit・出力の破棄・止まった記録の解消・分割
// ---------------------------------------------------------------------------

impl Fixture {
    /// worker の起動回数（スタブが数える）。
    fn worker_runs(&self) -> usize {
        fs::read_to_string(self.base.join("prompt.runs"))
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    }
    /// n 回目（1 始まり）の worker のプロンプト。
    fn prompt_of_run(&self, n: usize) -> String {
        let raw = fs::read(self.base.join(format!("prompt.json.{n}"))).unwrap();
        let v: Value = serde_json::from_slice(&raw).unwrap();
        v["prompt"].as_str().unwrap().to_owned()
    }
    fn readme_path(&self) -> String {
        self.root.join("README.md").to_str().unwrap().to_owned()
    }
}

fn readme_limit_config(permission: Permission, max_chars: u64) -> Config {
    let mut config = Config::legacy_all_push();
    config.worker.permission = permission;
    config.defaults.documents = vec![Document::Readme];
    config.kinds.insert(
        "readme".into(),
        config::KindSettings {
            max_chars: Some(max_chars),
            ..Default::default()
        },
    );
    config
}

#[test]
fn a_rerun_with_numbers_fixes_a_limit_violation_and_commits_without_a_warning() {
    let config = kinds_config(|c| arch(c).max_chars = Some(10));
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("docs/ARCHITECTURE.md", "short");
    f.hook("UserPromptSubmit");
    f.push_change("limit");
    let head = f.head();
    // 1 回目は 20 文字（超過）、2 回目で 8 文字に収める。
    f.writes(json!([
        {f.arch_path(): "x".repeat(20)},
        {f.arch_path(): "x".repeat(8)},
    ]));
    assert_eq!(f.hook("Stop"), None, "警告なしで完了するはず");
    assert_eq!(f.worker_runs(), 2);
    let second = f.prompt_of_run(2);
    assert!(
        second.contains("docs/ARCHITECTURE.md: currently 20 characters, but at most 10")
            && second.contains("Remove at least 10 characters")
            && second.contains("Fix only these violations"),
        "{second}"
    );
    assert_ne!(f.head(), head);
    assert_eq!(
        fs::read_to_string(f.root.join("docs/ARCHITECTURE.md")).unwrap(),
        "x".repeat(8)
    );
    let state = f.state();
    assert_eq!(state["phase"], "complete");
    assert_eq!(state["fixups"], 1);
    assert!(state.get("limit_warning").is_none());
    assert!(!f.kind_runs().is_empty());
    let last = f.runs().pop().unwrap().run;
    assert_eq!(
        (last.outcome.as_str(), last.detail.fixups),
        ("committed", 1)
    );
}

#[test]
fn a_violation_that_survives_the_reruns_is_committed_with_a_warning() {
    for permission in [Permission::Bypass, Permission::Restricted] {
        let config = kinds_config(|c| {
            c.worker.permission = permission;
            arch(c).max_chars = Some(10);
        });
        let f = Fixture::with_config(Some(config));
        f.commit_push_file("docs/ARCHITECTURE.md", "short");
        f.hook("UserPromptSubmit");
        f.push_change("limit");
        let head = f.head();
        f.writes(json!({f.arch_path(): "x".repeat(20)}));
        let message = f.hook("Stop").expect("警告の通知があるはず");
        assert!(
            message.contains("上限") && message.contains("docs/ARCHITECTURE.md"),
            "{permission:?}: {message}"
        );
        // 最初の起動と再実行 2 回の合計 3 回。警告つきで commit・push して完了し、位置も進む。
        assert_eq!(f.worker_runs(), 3, "{permission:?}");
        assert_ne!(f.head(), head, "{permission:?}");
        assert_eq!(f.git(&["rev-parse", "origin/main"]).trim(), f.head());
        let state = f.state();
        assert_eq!(state["phase"], "complete", "{permission:?}");
        assert!(state["warning"].as_str().unwrap().contains("上限"));
        assert_eq!(state["fixups"], 2);
        assert!(!f.kind_runs().is_empty(), "{permission:?}");
        let last = f.runs().pop().unwrap().run;
        assert_eq!(last.outcome, "committed", "{permission:?}");
        assert!(last.message.unwrap().contains("上限"));
        assert_eq!(last.detail.fixups, 2);
        // 次の push はそのまま処理される（止まり続けない）。
        f.push_change("next");
        f.writes(json!({f.arch_path(): "x".repeat(9)}));
        assert_eq!(f.hook("Stop"), None, "{permission:?}");
    }
}

#[test]
fn a_document_already_over_its_limit_is_fixed_even_if_the_worker_did_not_touch_it() {
    let config = readme_limit_config(Permission::Bypass, 20);
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("README.md", &"r".repeat(100));
    f.hook("UserPromptSubmit");
    f.push_change("later");
    // 1 回目は何も書かない（元から超過している README を触らない）。2 回目で収める。
    f.writes(json!([{}, {f.readme_path(): "short"}]));
    assert_eq!(f.hook("Stop"), None);
    assert_eq!(f.worker_runs(), 2);
    assert!(
        f.prompt_of_run(2)
            .contains("README.md: currently 100 characters"),
        "{}",
        f.prompt_of_run(2)
    );
    assert_eq!(
        fs::read_to_string(f.root.join("README.md")).unwrap(),
        "short"
    );
}

#[test]
fn the_first_prompt_demands_the_limit_and_names_the_split_files() {
    let config = readme_limit_config(Permission::Bypass, 50);
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("README.md", "# 題\n## 概要\n本文\n");
    f.hook("UserPromptSubmit");
    f.push_change("later");
    assert_eq!(f.hook("Stop"), None);
    let prompt = f.prompt_text();
    for needle in [
        "bringing it within the limit is itself a required part of this task",
        "files under `docs/readme/`",
        "docs/readme/section-01.md",
    ] {
        assert!(prompt.contains(needle), "{needle}\n{prompt}");
    }
}

#[test]
fn split_files_are_allowed_committed_and_checked_per_file() {
    let config = readme_limit_config(Permission::Restricted, 40);
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("README.md", "# 題\n## 概要\n本文\n");
    f.hook("UserPromptSubmit");
    f.push_change("later");
    let head = f.head();
    let part = f.root.join("docs/readme/section-01.md");
    f.writes(json!({
        f.readme_path(): "# 題\n[概要](docs/readme/section-01.md)\n",
        part.to_str().unwrap(): "詳細な説明".repeat(5),
    }));
    assert_eq!(f.hook("Stop"), None);
    assert_ne!(f.head(), head);
    let files = f.head_files();
    assert!(
        files.contains(&"README.md".to_string())
            && files.contains(&"docs/readme/section-01.md".to_string()),
        "{files:?}"
    );
    // restricted の許可規則に、分割先の新規ファイルが完全一致で入る。
    assert!(
        f.allowed_rules()
            .iter()
            .any(|r| r.contains("docs/readme/section-01.md")),
        "{:?}",
        f.allowed_rules()
    );
    assert_eq!(f.state()["phase"], "complete");
}

#[test]
fn a_file_in_the_split_dir_that_was_not_listed_is_a_worker_violation() {
    let config = readme_limit_config(Permission::Restricted, 1000);
    let f = Fixture::with_config(Some(config));
    f.commit_push_file("README.md", "# 題\n## 概要\n本文\n");
    f.hook("UserPromptSubmit");
    f.push_change("later");
    let head = f.head();
    let other = f.root.join("docs/readme/other.md");
    f.writes(json!({other.to_str().unwrap(): "列挙されていない名前"}));
    let message = f.hook("Stop").expect("許可パス外の変更で失敗するはず");
    assert!(message.contains("docs/readme/other.md"), "{message}");
    assert_eq!(f.head(), head);
    assert_eq!(f.state()["phase"], "worker_failed");
}

#[test]
fn an_agents_file_over_30000_bytes_is_discarded_and_maintenance_completes() {
    let f = Fixture::with_config(Some(Config::legacy_all_push()));
    f.hook("UserPromptSubmit");
    f.push_change("big agents");
    let head = f.head();
    f.writes(json!({agents_path(&f): "a".repeat(30000)}));
    let message = f.hook("Stop").expect("破棄の通知があるはず");
    assert!(
        message.contains("サイズ超過") && message.contains("破棄"),
        "{message}"
    );
    // 再実行は 1 回だけ行う。2 回目の起動でも超過量が縮まない（毎回同じ内容を書く）ので、
    // 続けても進まないと見て打ち切り、worker の出力を破棄して完了にする。
    assert_eq!(f.worker_runs(), 2);
    assert!(
        f.prompt_of_run(2)
            .contains("AGENTS files must stay under 30000 bytes"),
        "{}",
        f.prompt_of_run(2)
    );
    assert_eq!(f.head(), head, "commit しない");
    assert_eq!(
        fs::read_to_string(f.root.join("AGENTS.md")).unwrap(),
        "instructions",
        "worker の出力は元に戻す"
    );
    let state = f.state();
    assert_eq!(state["phase"], "complete");
    assert!(state["error"].is_null());
    let last = f.runs().pop().unwrap().run;
    assert_eq!(last.outcome, "rejected");
    // 棄却は完了に数えない。位置は進むので、次の push は止まらず処理される。
    assert!(f.kind_runs().is_empty());
    f.push_change("next");
    f.writes(json!({f.readme_path(): "documented"}));
    assert_eq!(f.hook("Stop"), None);
    assert_ne!(f.head(), f.git(&["rev-parse", "HEAD~1"]).trim());
    assert_eq!(
        f.git(&["show", "--name-only", "--format=", "HEAD"]),
        "README.md\n"
    );
}

fn chain_fixture() -> Fixture {
    // モノレポの実例: ルート 22,392 + 子 8,247 = 30,639 バイトで、HEAD の時点で既に階層の上限を超えている。
    let f = Fixture::with_config(Some(Config::legacy_all_push()));
    f.commit_push_file("AGENTS.md", &"a".repeat(22_392));
    f.commit_push_file("apps/hikyaku/AGENTS.md", &"b".repeat(8_247));
    f
}

fn child_agents(f: &Fixture) -> String {
    f.root
        .join("apps/hikyaku/AGENTS.md")
        .to_str()
        .unwrap()
        .to_owned()
}

#[test]
fn an_existing_chain_excess_is_shrunk_by_the_rerun_and_committed_under_the_limit() {
    let f = chain_fixture();
    f.hook("UserPromptSubmit");
    f.commit_push_file("apps/hikyaku/notes.txt", "change");
    // 1 回目はまだ足りない（30,247 バイト）、再実行で上限内（28,000 バイト）に収める。
    f.writes(json!([
        {agents_path(&f): "a".repeat(22_000)},
        {agents_path(&f): "a".repeat(20_000), child_agents(&f): "b".repeat(8_000)}
    ]));
    let head = f.head();
    assert_eq!(f.hook("Stop"), None);
    assert_eq!(f.worker_runs(), 2);
    // 最初のプロンプトは「既に超過している」ことと内訳・削減量を伝える。
    let first = f.prompt_of_run(1);
    for needle in [
        "ALREADY exceed",
        "AGENTS.md: 22392 bytes (editable)",
        "apps/hikyaku/AGENTS.md: 8247 bytes (editable)",
        "Remove at least 640 bytes in total",
    ] {
        assert!(first.contains(needle), "{needle}\n{first}");
    }
    // 再実行のプロンプトは、測り直した現在のバイト数と残りの超過量を渡す。
    let second = f.prompt_of_run(2);
    for needle in [
        "total 30247 bytes",
        "AGENTS.md: 22000 bytes (editable)",
        "Remove at least 248 bytes in total",
    ] {
        assert!(second.contains(needle), "{needle}\n{second}");
    }
    assert_ne!(f.head(), head, "commit・push まで完了する");
    assert_eq!(f.state()["phase"], "complete");
    // 作業ツリーの実バイトと commit ツリーの両方が上限内。
    let dirs = [f.root.join("apps/hikyaku")];
    assert_eq!(agents_size_error(&f.root, &dirs, true), None);
    assert_eq!(agents_tree_size_error(&f.root, "HEAD", &dirs), None);
    assert_eq!(f.git(&["status", "--porcelain"]), "");
    assert_eq!(
        f.git(&["rev-parse", "HEAD"]),
        f.git(&["rev-parse", "origin/main"])
    );
}

#[test]
fn a_chain_excess_is_fixed_with_only_the_editable_members_when_they_can_absorb_it() {
    let f = chain_fixture();
    f.hook("UserPromptSubmit");
    f.commit_push_file("apps/hikyaku/notes.txt", "change");
    // 子は利用者が編集中（許可パス外）。ルートだけで吸収できる。
    fs::write(f.root.join("apps/hikyaku/AGENTS.md"), "b".repeat(8_300)).unwrap();
    f.writes(json!({agents_path(&f): "a".repeat(20_000)}));
    let head = f.head();
    assert_eq!(f.hook("Stop"), None);
    assert_ne!(f.head(), head);
    let first = f.prompt_of_run(1);
    assert!(
        first.contains("apps/hikyaku/AGENTS.md: 8300 bytes (NOT editable this time)"),
        "{first}"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("apps/hikyaku/AGENTS.md")).unwrap(),
        "b".repeat(8_300),
        "利用者の編集は触らない"
    );
    assert_eq!(
        f.git(&["show", "--name-only", "--format=", "HEAD"]),
        "AGENTS.md\n"
    );
}

#[test]
fn an_unfixable_chain_excess_is_not_committed_and_the_reason_names_the_files_and_bytes() {
    let f = Fixture::with_config(Some(Config::legacy_all_push()));
    f.commit_push_file("AGENTS.md", &"a".repeat(29_990));
    f.commit_push_file("apps/hikyaku/AGENTS.md", &"b".repeat(300));
    f.hook("UserPromptSubmit");
    f.commit_push_file("apps/hikyaku/notes.txt", "change");
    // ルートは利用者が編集中で許可パス外。編集できる子（300 バイト）では 300 バイトの超過を吸収できない。
    fs::write(f.root.join("AGENTS.md"), "a".repeat(29_999)).unwrap();
    let head = f.head();
    let message = f.hook("Stop").expect("理由つきの通知があるはず");
    assert_eq!(f.head(), head, "commit しない");
    assert_eq!(f.worker_runs(), 1, "吸収できない超過は再実行しない");
    for needle in [
        "破棄",
        "300 bytes 以上",
        "AGENTS.md (29999)",
        "編集できなかったファイル",
    ] {
        assert!(message.contains(needle), "{needle}\n{message}");
    }
    assert!(!message.contains("##"), "{message}");
    assert_eq!(f.state()["phase"], "complete");
}

#[test]
fn a_discard_leaves_an_lf_file_clean_even_when_autocrlf_converts_the_restore() {
    let f = Fixture::with_config(Some(Config::legacy_all_push()));
    f.commit_push_file("AGENTS.md", "a\nb\n");
    f.hook("UserPromptSubmit");
    f.push_change("autocrlf");
    // 以降の `git restore` は CRLF に変換して書き戻す（作業ツリーの元の内容は LF）。
    f.git(&["config", "core.autocrlf", "true"]);
    f.writes(json!({agents_path(&f): "line\n".repeat(7_000)}));
    let message = f.hook("Stop").expect("破棄の通知があるはず");
    assert!(message.contains("破棄"), "{message}");
    assert!(!message.contains("戻していないパス"), "{message}");
    assert_eq!(fs::read(f.root.join("AGENTS.md")).unwrap(), b"a\nb\n");
    assert_eq!(f.git(&["status", "--porcelain"]), "");
}

#[test]
fn a_ready_record_with_an_editable_agents_excess_resumes_with_a_rerun() {
    let f = Fixture::with_config(Some(Config::legacy_all_push()));
    f.commit_push_file("AGENTS.md", &"a".repeat(30_500));
    let mut state = f.checkpoint("ready");
    f.save(&mut state);
    f.writes(json!({agents_path(&f): "short"}));
    let head = f.head();
    assert_eq!(f.maintain(), None);
    assert_eq!(f.worker_runs(), 1);
    let prompt = f.prompt_of_run(1);
    assert!(prompt.contains("Remove at least 501 bytes"), "{prompt}");
    assert_ne!(f.head(), head);
    assert_eq!(f.state()["phase"], "complete");
    assert_eq!(agents_tree_size_error(&f.root, "HEAD", &[]), None);
}

/// ルート 23,500 + 子 8,247 を HEAD に持ち、子は利用者が 7,000 バイトへ縮める変更を未コミットで持つ
/// （子は編集できない）。作業ツリーの合計は 30,500 だが、commit されるツリーでは子は 8,247 のまま。
fn uncommitted_shrunk_child_fixture() -> Fixture {
    let f = Fixture::with_config(Some(Config::legacy_all_push()));
    f.commit_push_file("AGENTS.md", &"a".repeat(23_500));
    f.commit_push_file("apps/hikyaku/AGENTS.md", &"b".repeat(8_247));
    f.hook("UserPromptSubmit");
    f.commit_push_file("apps/hikyaku/notes.txt", "change");
    fs::write(f.root.join("apps/hikyaku/AGENTS.md"), "b".repeat(7_000)).unwrap();
    f
}

#[test]
fn a_non_editable_member_is_counted_at_its_committed_size_and_never_exceeds_in_the_commit() {
    // 作業ツリーでは 29,900 だが commit されるツリーでは 31,147 になる出力は、commit せず破棄する。
    let f = uncommitted_shrunk_child_fixture();
    f.writes(json!({agents_path(&f): "a".repeat(22_900)}));
    let head = f.head();
    let message = f.hook("Stop").expect("破棄の通知があるはず");
    assert!(message.contains("破棄"), "{message}");
    assert_eq!(f.head(), head, "commit しない");
    assert_eq!(
        f.git(&["rev-parse", "HEAD"]),
        f.git(&["rev-parse", "origin/main"])
    );
    assert_eq!(
        fs::read_to_string(f.root.join("AGENTS.md")).unwrap().len(),
        23_500
    );
    assert_eq!(f.state()["phase"], "complete");
    // 編集できない子は HEAD の大きさで数え、必要削減量は 1,748 バイト（31,747 - 29,999）になる。
    let first = f.prompt_of_run(1);
    assert!(first.contains("Remove at least 1748 bytes"), "{first}");
    assert!(
        first.contains("apps/hikyaku/AGENTS.md: 8247 bytes (NOT editable this time)"),
        "{first}"
    );
}

#[test]
fn a_rerun_that_brings_the_committed_tree_under_the_limit_is_committed() {
    let f = uncommitted_shrunk_child_fixture();
    f.writes(json!([
        {agents_path(&f): "a".repeat(22_900)},
        {agents_path(&f): "a".repeat(21_000)}
    ]));
    let head = f.head();
    assert_eq!(f.hook("Stop"), None);
    assert_eq!(f.worker_runs(), 2);
    assert_ne!(f.head(), head);
    assert_eq!(
        agents_tree_size_error(&f.root, "HEAD", &[f.root.join("apps/hikyaku")]),
        None
    );
    assert_eq!(
        f.git(&["show", "--name-only", "--format=", "HEAD"]),
        "AGENTS.md\n"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("apps/hikyaku/AGENTS.md")).unwrap(),
        "b".repeat(7_000),
        "利用者の未コミットの変更は触らない"
    );
}

#[test]
fn a_first_run_that_grows_an_existing_excess_still_gets_the_dedicated_reruns() {
    let f = Fixture::with_config(Some(Config::legacy_all_push()));
    f.commit_push_file("AGENTS.md", &"a".repeat(30_500));
    f.hook("UserPromptSubmit");
    f.push_change("grow");
    // 最初の起動は超過を 100 バイト増やす（保守そのもの）。再実行で縮めれば commit まで完了する。
    f.writes(json!([
        {agents_path(&f): "a".repeat(30_600)},
        {agents_path(&f): "short"}
    ]));
    let head = f.head();
    assert_eq!(f.hook("Stop"), None);
    assert_eq!(f.worker_runs(), 2);
    assert_ne!(f.head(), head);
    assert_eq!(agents_tree_size_error(&f.root, "HEAD", &[]), None);
}

#[test]
fn agents_reruns_stop_after_three_even_while_the_excess_keeps_shrinking() {
    let f = Fixture::with_config(Some(Config::legacy_all_push()));
    f.commit_push_file("AGENTS.md", &"a".repeat(31_000));
    f.hook("UserPromptSubmit");
    f.push_change("slow");
    // スタブへは環境変数で渡す。Linux は 1 つの環境変数を 128 KiB（MAX_ARG_STRLEN）までしか渡せないので、
    // 起動 4 回分（最初の起動 + 再実行 3 回）の 4 件だけにして、合計を 128 KiB 未満に収める。
    let sizes: Vec<Value> = [30_500, 30_400, 30_300, 30_200]
        .iter()
        .map(|n| json!({agents_path(&f): "a".repeat(*n)}))
        .collect();
    f.writes(Value::Array(sizes));
    let head = f.head();
    let message = f.hook("Stop").expect("破棄の通知があるはず");
    assert!(message.contains("破棄"), "{message}");
    // 最初の起動 + 再実行 3 回。
    assert_eq!(f.worker_runs(), 4);
    assert_eq!(f.head(), head);
    assert_eq!(
        fs::read_to_string(f.root.join("AGENTS.md")).unwrap().len(),
        31_000
    );
}

#[test]
fn a_ready_record_returns_to_the_rerun_only_once() {
    let f = Fixture::with_config(Some(Config::legacy_all_push()));
    f.commit_push_file("AGENTS.md", &"a".repeat(30_500));
    let mut state = f.checkpoint("ready");
    state["agents_resumed"] = json!(true);
    f.save(&mut state);
    let head = f.head();
    assert_eq!(f.maintain(), None);
    assert_eq!(f.worker_runs(), 0, "2 回目の再開では worker を起動しない");
    assert_eq!(f.head(), head);
    assert_eq!(f.state()["phase"], "complete");
}

#[test]
fn a_personal_override_in_the_chain_is_not_editable_and_the_rest_absorbs_the_excess() {
    let f = Fixture::with_config(Some(Config::legacy_all_push()));
    f.commit_push_file("AGENTS.md", &"a".repeat(20_000));
    f.commit_push_file("apps/x/AGENTS.override.md", &"o".repeat(10_500));
    f.hook("UserPromptSubmit");
    f.commit_push_file("apps/x/notes.txt", "change");
    f.writes(json!({agents_path(&f): "a".repeat(19_000)}));
    let head = f.head();
    assert_eq!(f.hook("Stop"), None);
    assert_ne!(f.head(), head);
    let first = f.prompt_of_run(1);
    assert!(
        first.contains("apps/x/AGENTS.override.md: 10500 bytes (personal override, NOT editable)"),
        "{first}"
    );
    // override は単体の超過として「削れ」とも言わない（階層側と矛盾しない）。
    assert!(!first.contains("AGENTS.override.md: currently"), "{first}");
    assert_eq!(
        agents_tree_size_error(&f.root, "HEAD", &[f.root.join("apps/x")]),
        None
    );
}

#[test]
fn a_chain_excess_is_measured_at_the_deepest_directory() {
    let f = Fixture::with_config(Some(Config::legacy_all_push()));
    f.commit_push_file("AGENTS.md", &"a".repeat(20_000));
    f.commit_push_file("a/AGENTS.md", &"b".repeat(10_500));
    f.commit_push_file("a/b/AGENTS.md", &"c".repeat(5_000));
    f.hook("UserPromptSubmit");
    f.commit_push_file("a/b/notes.txt", "change");
    f.writes(json!({agents_path(&f): "a".repeat(14_000)}));
    let head = f.head();
    assert_eq!(f.hook("Stop"), None);
    assert_ne!(f.head(), head);
    // 浅い `a`（30,500）ではなく、最も深い `a/b`（35,500）の必要削減量で伝える。
    let first = f.prompt_of_run(1);
    assert!(first.contains("applied to `a/b`"), "{first}");
    assert!(first.contains("Remove at least 5501 bytes"), "{first}");
    assert!(!first.contains("applied to `a`"), "{first}");
}

#[test]
fn released_changelog_changes_are_discarded_without_a_rerun() {
    let f = Fixture::with_config(Some(changelog_config(
        Permission::Bypass,
        vec![Document::Changelog],
    )));
    f.commit_push_file("CHANGELOG.md", KAC_CHANGELOG);
    f.hook("UserPromptSubmit");
    f.push_change("rewrite released");
    let head = f.head();
    f.writes(json!({f.changelog_path(): KAC_CHANGELOG.replace("- 初版", "- 初版!")}));
    let message = f.hook("Stop").expect("破棄の通知があるはず");
    assert!(
        message.contains("リリース済み") && message.contains("破棄"),
        "{message}"
    );
    assert_eq!(f.worker_runs(), 1, "リリース済みの節の変更は再実行しない");
    assert_eq!(f.head(), head);
    assert_eq!(
        fs::read_to_string(f.root.join("CHANGELOG.md")).unwrap(),
        KAC_CHANGELOG
    );
    assert_eq!(f.state()["phase"], "complete");
    assert_eq!(f.runs().pop().unwrap().run.outcome, "rejected");
}

#[test]
fn a_stuck_ready_state_is_discarded_and_a_new_push_is_processed() {
    let config = readme_limit_config(Permission::Bypass, 1000);
    let f = Fixture::with_config(Some(config));
    f.hook("UserPromptSubmit");
    // 旧版が残した「違反で ready のまま止まった」記録を作る（worker の出力 README が未コミットで残っている）。
    let mut state = f.checkpoint("ready");
    fs::write(f.root.join("README.md"), "stale worker output").unwrap();
    state["after"]["README.md"] = json!(document_hash(&f.root, "README.md").unwrap());
    state["error"] = json!("old error");
    f.save(&mut state);
    f.push_change("new push moves HEAD");
    f.writes(json!({f.readme_path(): "fresh output"}));
    // 古い出力を破棄した旨が通知される。
    let message = f.hook("Stop").expect("破棄の通知があるはず");
    assert!(message.contains("前回の保守の出力を破棄"), "{message}");
    // 古い出力は破棄され、新しい push の保守として commit される。
    assert_eq!(
        fs::read_to_string(f.root.join("README.md")).unwrap(),
        "fresh output"
    );
    assert_ne!(f.head(), state["head"].as_str().unwrap());
    assert_eq!(
        f.git(&["show", "--name-only", "--format=", "HEAD"]),
        "README.md\n"
    );
    assert_eq!(f.state()["phase"], "complete");
}

#[test]
fn a_stuck_state_never_touches_files_that_no_longer_match_the_record() {
    let config = readme_limit_config(Permission::Bypass, 1000);
    let f = Fixture::with_config(Some(config));
    f.hook("UserPromptSubmit");
    let mut state = f.checkpoint("ready");
    fs::write(f.root.join("README.md"), "worker output").unwrap();
    state["after"]["README.md"] = json!(document_hash(&f.root, "README.md").unwrap());
    f.save(&mut state);
    // 記録のあとに利用者が README を書き換えた（記録と一致しない）。
    fs::write(f.root.join("README.md"), "the user's own edit").unwrap();
    f.push_change("new push moves HEAD");
    // 利用者の変更が README に残っているので、README は編集を許す対象にならず、触られない。
    let _ = f.hook("Stop");
    assert_eq!(
        fs::read_to_string(f.root.join("README.md")).unwrap(),
        "the user's own edit"
    );
}
