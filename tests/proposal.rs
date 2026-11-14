//! propose モード（src/proposal.rs）の契約テスト。
//!
//! 隔離: 一時フォルダーとローカルの bare remote を使い、`SCRIPTURA_DB` / `SCRIPTURA_CONFIG` で状態と設定を
//! 差し替える（`SCRIPTURA_HOME` は外す）。worker は既存のスタブ（本物の Claude は呼ばない）。
//! fixture の置き場は環境変数 `SCRIPTURA_TEST_BASE` で変えられる（未指定なら既存テストと同じ `validation/`）。
#![cfg(feature = "test-support")]
use rusqlite::Connection;
use scriptura::config::{Config, Document, Mode, Permission, Worker};
use scriptura::maintenance::*;
use scriptura::proposal::{self, ProposeOutcome};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, MutexGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static ENV: Mutex<()> = Mutex::new(());

struct Fixture {
    _guard: MutexGuard<'static, ()>,
    base: PathBuf,
    root: PathBuf,
    remote: PathBuf,
    old_path: std::ffi::OsString,
}

impl Fixture {
    /// propose モード（全リポジトリ対象・同意あり・3 文書）の環境。
    fn new() -> Self {
        let guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let parent = env::var_os("SCRIPTURA_TEST_BASE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("validation"));
        let base = parent.join(format!(
            "proposal-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("repo");
        let remote = base.join("remote.git");
        let bin = base.join("bin");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_hook-test-stub"), bin.join("claude.exe")).unwrap();
        let old_path = env::var_os("PATH").unwrap_or_default();
        unsafe {
            env::set_var(
                "PATH",
                format!("{};{}", bin.display(), old_path.to_string_lossy()),
            );
            env::set_var("SCRIPTURA_DB", base.join("state/state.sqlite3"));
            env::set_var("HOOK_STUB_PROMPT", base.join("prompt.json"));
            env::set_var("SCRIPTURA_CONFIG", base.join("config.toml"));
            for key in [
                "HOOK_STUB_WRITES",
                "HOOK_STUB_EXIT",
                "HOOK_STUB_SLEEP",
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
        f.git(&["config", "user.name", "Proposal fixture"]);
        f.git(&["config", "core.autocrlf", "false"]);
        fs::write(f.root.join("source.txt"), "initial").unwrap();
        fs::write(f.root.join("AGENTS.md"), "instructions").unwrap();
        fs::write(f.root.join("DESIGN.md"), "design").unwrap();
        fs::write(f.root.join("README.md"), "readme").unwrap();
        f.git(&["add", "."]);
        f.git(&["commit", "-m", "initial"]);
        assert!(
            Command::new("git")
                .args(["init", "--bare", "-b", "main"])
                .arg(&f.remote)
                .output()
                .unwrap()
                .status
                .success()
        );
        f.git(&["remote", "add", "origin", f.remote.to_str().unwrap()]);
        f.git(&["push", "-u", "origin", "main"]);
        let mut config = Config::legacy_all_push();
        config.defaults.mode = Mode::Propose;
        scriptura::config::save_to(&f.base.join("config.toml"), &config).unwrap();
        f
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

    /// source.txt を変更して commit・push し、push した完全な sha を返す。
    fn push_change(&self, body: &str) -> String {
        fs::write(self.root.join("source.txt"), body).unwrap();
        self.git(&["add", "source.txt"]);
        self.git(&["commit", "-m", body]);
        self.git(&["push"]);
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn hook(&self, event: &str) -> Option<String> {
        handle_hook(&json!({"hook_event_name":event,"cwd":self.root})).unwrap()
    }

    /// スタブ worker の書込み。キーは cwd 相対（= 使い捨てクローン内）にする。
    fn writes(&self, value: Value) {
        unsafe { env::set_var("HOOK_STUB_WRITES", value.to_string()) }
    }

    /// poll_cursors の (fingerprint, pushed_commit, error)。
    fn cursor(&self) -> Option<(String, String, Option<String>)> {
        let db = Connection::open(state_db_path()).ok()?;
        db.query_row(
            "SELECT fingerprint,pushed_commit,error FROM poll_cursors",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()
    }

    fn remote_refs(&self) -> String {
        let r = Command::new("git")
            .arg("-C")
            .arg(&self.remote)
            .args(["for-each-ref", "--format=%(refname) %(objectid)"])
            .output()
            .unwrap();
        String::from_utf8(r.stdout).unwrap()
    }

    fn options(&self, timeout_secs: u64) -> MaintenanceOptions {
        MaintenanceOptions {
            documents: Document::ALL.to_vec(),
            exclude: scriptura::config::effective_exclude(&[]),
            worker: Worker {
                permission: Permission::Bypass,
                timeout_secs,
                ..Worker::default()
            },
        }
    }

    /// `proposals/<hash16>` に残っている `work-*`（使い捨てクローン）。
    fn leftover_clones(&self) -> Vec<String> {
        fs::read_dir(proposal::proposals_dir(&self.root))
            .map(|it| {
                it.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with("work-"))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn proposal_files(&self) -> Vec<String> {
        fs::read_dir(proposal::proposals_dir(&self.root))
            .map(|it| {
                it.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| !n.starts_with("work-"))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            env::set_var("PATH", &self.old_path);
            for k in [
                "SCRIPTURA_DB",
                "SCRIPTURA_CONFIG",
                "HOOK_STUB_SLEEP",
                "HOOK_STUB_PROMPT",
                "HOOK_STUB_WRITES",
                "HOOK_STUB_EXIT",
            ] {
                env::remove_var(k);
            }
        }
        eprintln!("cleanup fixture: {}", self.base.display());
    }
}

/// フォルダー配下（`.git` を含む）の全ファイルの「相対パス + 内容」のハッシュ。
fn tree_hash(dir: &Path) -> String {
    let mut hasher = Sha256::new();
    let mut stack = vec![dir.to_path_buf()];
    let mut files = Vec::new();
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    for path in files {
        hasher.update(path.strip_prefix(dir).unwrap().to_string_lossy().as_bytes());
        hasher.update(fs::read(&path).unwrap());
    }
    format!("{:x}", hasher.finalize())
}

#[test]
fn propose_changes_nothing_of_the_user_repository_and_saves_a_document_only_patch() {
    let f = Fixture::new();
    f.hook("UserPromptSubmit");
    // 利用者の未追跡・無視・未コミット変更を用意する（これらは worker に見えず、前後で変わらない）。
    fs::write(f.root.join(".env"), "SECRET=1").unwrap();
    fs::write(f.root.join(".gitignore"), "ignored.log\n").unwrap();
    fs::write(f.root.join("ignored.log"), "noise").unwrap();
    f.git(&["add", ".gitignore"]);
    f.git(&["commit", "-m", "gitignore"]);
    let sha = f.push_change("push for propose");
    fs::write(f.root.join("DESIGN.md"), "dirty local edit").unwrap();
    let tree_before = tree_hash(&f.root);
    let remote_before = f.remote_refs();
    f.writes(json!({"README.md":"maintained by worker"}));

    let message = f.hook("Stop").expect("更新案の作成が通知される");
    assert!(message.contains("更新案"), "{message}");

    // 作業ツリー（.git を含む）と remote の ref は 1 バイトも変わらない。
    assert_eq!(tree_hash(&f.root), tree_before);
    assert_eq!(f.remote_refs(), remote_before);
    assert_eq!(fs::read_to_string(f.root.join(".env")).unwrap(), "SECRET=1");
    // パッチが作られ、内容は対象文書だけ。
    let patch_path = proposal::proposals_dir(&f.root).join(format!("{sha}.patch"));
    let patch = fs::read_to_string(&patch_path).unwrap();
    assert!(patch.contains("README.md") && patch.contains("maintained by worker"));
    assert!(!patch.contains("source.txt") && !patch.contains(".env"));
    assert!(!patch.contains("dirty local edit"));
    // メタ情報に秘密値・環境依存値は無い。
    let meta: Value = serde_json::from_slice(
        &fs::read(proposal::proposals_dir(&f.root).join(format!("{sha}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(meta["commit"], sha);
    assert_eq!(meta["documents"], json!(["README.md"]));
    assert_eq!(meta["worker"]["permission"], "bypass");
    let keys: Vec<_> = meta["worker"].as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "model",
            "effort",
            "permission",
            "timeout_secs",
            "max_budget_usd"
        ]
    );
    // カーソルは成功として進む。クローンは残らない。
    let cursor = f.cursor().unwrap();
    assert_eq!(cursor.1, sha);
    assert!(cursor.2.is_none());
    assert!(f.leftover_clones().is_empty());
    // propose では commit も push も行われない（HEAD は push 済みのまま）。
    assert_eq!(f.git(&["rev-parse", "HEAD"]).trim(), sha);
}

#[test]
fn prepared_clone_hides_untracked_files_and_cannot_reach_the_user_repository() {
    let f = Fixture::new();
    fs::write(f.root.join(".env"), "SECRET=1").unwrap();
    fs::create_dir_all(f.root.join("secrets")).unwrap();
    fs::write(f.root.join("secrets/token"), "x").unwrap();
    let sha = f.git(&["rev-parse", "HEAD"]).trim().to_string();
    let base = proposal::proposals_dir(&f.root);
    let guard = proposal::prepare_clone(&f.root, &sha, &base).unwrap();
    let work = guard.path().to_path_buf();
    // worker の cwd になるクローンには、追跡済みファイルだけがある。
    assert!(work.join("source.txt").is_file());
    assert!(!work.join(".env").exists());
    assert!(!work.join("secrets").exists());
    // origin は外されており、HEAD は対象コミットの detached。
    let remotes = Command::new("git")
        .arg("-C")
        .arg(&work)
        .arg("remote")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&remotes.stdout).trim().is_empty());
    let head = Command::new("git")
        .arg("-C")
        .arg(&work)
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .unwrap();
    assert!(!head.status.success(), "detached のはず");
    // 読み取り専用属性のファイルがあっても、Drop で消える。
    let readonly = work.join(".git/readonly-probe");
    fs::write(&readonly, "x").unwrap();
    let mut permissions = fs::metadata(&readonly).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&readonly, permissions).unwrap();
    drop(guard);
    assert!(!work.exists());
}

#[test]
fn worker_runs_inside_the_clone_not_the_user_worktree() {
    let f = Fixture::new();
    f.hook("UserPromptSubmit");
    fs::write(f.root.join(".env"), "SECRET=1").unwrap();
    f.push_change("cwd check");
    // 相対パスで書いたファイルがパッチに入る = worker の cwd はクローン。利用者の作業ツリーには書かれない。
    f.writes(json!({"README.md":"written in cwd"}));
    f.hook("Stop").unwrap();
    assert_eq!(
        fs::read_to_string(f.root.join("README.md")).unwrap(),
        "readme"
    );
    let patch = fs::read_to_string(&proposal::list(&f.root)[0].patch_path).unwrap();
    assert!(patch.contains("written in cwd"));
}

#[test]
fn clone_is_removed_on_success_failure_and_timeout() {
    let f = Fixture::new();
    let sha = f.push_change("cleanup cases");
    // 成功
    f.writes(json!({"README.md":"ok"}));
    let outcome = proposal::propose(&f.root, &sha, "", &f.options(60)).unwrap();
    assert!(matches!(outcome, ProposeOutcome::Created { .. }));
    assert!(f.leftover_clones().is_empty());
    // 失敗（worker の終了コード）
    let sha2 = f.push_change("cleanup failure");
    unsafe { env::set_var("HOOK_STUB_EXIT", "1") };
    let error = proposal::propose(&f.root, &sha2, "", &f.options(60)).unwrap_err();
    assert!(error.contains("終了コード"), "{error}");
    assert!(f.leftover_clones().is_empty());
    // タイムアウト（期限 1 秒・worker は 5 秒居座る）
    unsafe {
        env::remove_var("HOOK_STUB_EXIT");
        env::set_var("HOOK_STUB_SLEEP", "5");
    }
    let sha3 = f.push_change("cleanup timeout");
    let error = proposal::propose(&f.root, &sha3, "", &f.options(1)).unwrap_err();
    assert!(error.contains("タイムアウト"), "{error}");
    assert!(f.leftover_clones().is_empty());
    assert!(
        !proposal::proposals_dir(&f.root)
            .join(format!("{sha3}.patch"))
            .exists()
    );
    assert!(
        !proposal::proposals_dir(&f.root)
            .join(format!("{sha2}.json"))
            .exists()
    );
}

#[test]
fn out_of_scope_change_fails_without_a_patch_and_keeps_the_cursor() {
    let f = Fixture::new();
    f.hook("UserPromptSubmit");
    f.push_change("first");
    let before = f.cursor().unwrap();
    f.writes(json!({"README.md":"fine","source.txt":"worker touched source"}));
    let message = f.hook("Stop").expect("失敗が通知される");
    assert!(message.contains("source.txt"), "{message}");
    assert!(f.proposal_files().is_empty(), "{:?}", f.proposal_files());
    assert!(f.leftover_clones().is_empty());
    // 失敗は再試行できる形（位置は進めず error を記録）。
    let after = f.cursor().unwrap();
    assert_eq!((&after.0, &after.1), (&before.0, &before.1));
    assert!(after.2.is_some());
    // 次の hook で再試行され、worker が正しく直せば成功する。
    f.writes(json!({"README.md":"fine"}));
    assert!(f.hook("Stop").unwrap().contains("更新案"));
    assert!(f.cursor().unwrap().2.is_none());
}

#[test]
fn the_same_commit_is_never_proposed_twice() {
    let f = Fixture::new();
    let sha = f.push_change("once");
    f.writes(json!({"README.md":"first"}));
    assert!(matches!(
        proposal::propose(&f.root, &sha, "", &f.options(60)).unwrap(),
        ProposeOutcome::Created { .. }
    ));
    let _ = fs::remove_file(f.base.join("prompt.json"));
    f.writes(json!({"README.md":"second"}));
    assert_eq!(
        proposal::propose(&f.root, &sha, "", &f.options(60)).unwrap(),
        ProposeOutcome::AlreadyDone
    );
    assert!(
        !f.base.join("prompt.json").exists(),
        "worker を再実行しない"
    );
    let patch = fs::read_to_string(&proposal::list(&f.root)[0].patch_path).unwrap();
    assert!(patch.contains("first") && !patch.contains("second"));
    // 破棄しても再作成されない。
    proposal::discard(&f.root, &sha[..8]).unwrap();
    assert_eq!(
        proposal::propose(&f.root, &sha, "", &f.options(60)).unwrap(),
        ProposeOutcome::AlreadyDone
    );
    assert!(proposal::list(&f.root).is_empty());
}

#[test]
fn unchanged_documents_save_nothing_and_advance_the_cursor() {
    let f = Fixture::new();
    f.hook("UserPromptSubmit");
    let sha = f.push_change("nothing to maintain");
    assert_eq!(f.hook("Stop"), None);
    assert!(f.proposal_files().is_empty(), "{:?}", f.proposal_files());
    assert!(f.leftover_clones().is_empty());
    assert!(proposal::list(&f.root).is_empty());
    let cursor = f.cursor().unwrap();
    assert_eq!(cursor.1, sha);
    assert!(cursor.2.is_none());
}

#[test]
fn newly_created_documents_are_part_of_the_patch() {
    let f = Fixture::new();
    f.git(&["rm", "-q", "DESIGN.md"]);
    f.git(&["commit", "-m", "remove design"]);
    let sha = f.push_change("design is missing");
    f.writes(json!({"DESIGN.md":"brand new design"}));
    let outcome = proposal::propose(&f.root, &sha, "", &f.options(60)).unwrap();
    let ProposeOutcome::Created { documents, .. } = outcome else {
        panic!("新規文書の作成が変更なしと誤判定された: {outcome:?}");
    };
    assert_eq!(documents, ["DESIGN.md"]);
    let applied = proposal::apply(&f.root, "latest").unwrap();
    assert_eq!(applied.files, ["DESIGN.md"]);
    assert_eq!(
        fs::read_to_string(f.root.join("DESIGN.md")).unwrap(),
        "brand new design"
    );
}

#[test]
fn list_and_review_report_the_saved_proposals_newest_first() {
    let f = Fixture::new();
    assert!(proposal::list(&f.root).is_empty());
    assert!(proposal::review(&f.root, "latest").is_err());
    let mut shas = Vec::new();
    for body in ["one", "two"] {
        let sha = f.push_change(body);
        f.writes(json!({"README.md":body}));
        proposal::propose(&f.root, &sha, "", &f.options(60)).unwrap();
        shas.push(sha);
        std::thread::sleep(Duration::from_millis(20));
    }
    let listed = proposal::list(&f.root);
    assert_eq!(
        listed.iter().map(|p| p.sha.clone()).collect::<Vec<_>>(),
        [shas[1].clone(), shas[0].clone()]
    );
    assert!(listed.iter().all(|p| !p.applied));
    for alias in ["latest", "最新", ""] {
        assert_eq!(proposal::review(&f.root, alias).unwrap().sha, shas[1]);
    }
    let review = proposal::review(&f.root, &shas[0][..10]).unwrap();
    assert_eq!(review.sha, shas[0]);
    assert_eq!(review.documents, ["README.md"]);
    assert!(review.diffstat.contains("README.md"));
    assert!(
        fs::read_to_string(&review.patch_path)
            .unwrap()
            .contains("one")
    );
    assert!(proposal::review(&f.root, "ffffffff").is_err());
    assert!(proposal::review(&f.root, "not-a-sha").is_err());
}

#[test]
fn apply_refuses_dirty_documents_then_applies_without_staging() {
    let f = Fixture::new();
    let sha = f.push_change("apply");
    f.writes(json!({"README.md":"proposed readme"}));
    proposal::propose(&f.root, &sha, "", &f.options(60)).unwrap();

    // 作業ツリーが dirty なら拒否。
    fs::write(f.root.join("README.md"), "local edit").unwrap();
    let error = proposal::apply(&f.root, "latest").unwrap_err();
    assert!(error.contains("README.md"), "{error}");
    assert_eq!(
        fs::read_to_string(f.root.join("README.md")).unwrap(),
        "local edit"
    );
    // index だけが dirty でも拒否。
    f.git(&["add", "README.md"]);
    fs::write(f.root.join("README.md"), "readme").unwrap();
    assert!(proposal::apply(&f.root, "latest").is_err());
    f.git(&["reset", "-q", "README.md"]);
    f.git(&["checkout", "--", "README.md"]);
    assert!(!proposal::list(&f.root)[0].applied);

    // clean なら適用され、stage はされない。
    let outcome = proposal::apply(&f.root, &sha[..8]).unwrap();
    assert_eq!(outcome.files, ["README.md"]);
    assert_eq!(
        fs::read_to_string(f.root.join("README.md")).unwrap(),
        "proposed readme"
    );
    assert_eq!(f.git(&["diff", "--cached", "--name-only"]), "");
    assert_eq!(f.git(&["diff", "--name-only"]), "README.md\n");
    // 適用済みとして記録され、二重適用は拒否される。
    let listed = proposal::list(&f.root);
    assert!(listed[0].applied);
    assert!(
        proposal::apply(&f.root, "latest")
            .unwrap_err()
            .contains("適用済み")
    );
}

#[test]
fn apply_checks_the_patch_before_touching_the_worktree() {
    let f = Fixture::new();
    let sha = f.push_change("conflict");
    f.writes(json!({"README.md":"proposed"}));
    proposal::propose(&f.root, &sha, "", &f.options(60)).unwrap();
    // 対象文書は clean のまま、内容が提案の前提と食い違うようコミットで変える。
    fs::write(f.root.join("README.md"), "diverged upstream").unwrap();
    f.git(&["add", "README.md"]);
    f.git(&["commit", "-m", "diverge"]);
    let error = proposal::apply(&f.root, "latest").unwrap_err();
    assert!(error.contains("適用できません"), "{error}");
    assert_eq!(
        fs::read_to_string(f.root.join("README.md")).unwrap(),
        "diverged upstream"
    );
    assert!(!proposal::list(&f.root)[0].applied);
}

#[test]
fn stale_clones_are_swept_but_other_entries_are_kept() {
    let f = Fixture::new();
    let base = proposal::proposals_dir(&f.root);
    fs::create_dir_all(base.join("work-leftover/.git")).unwrap();
    fs::write(base.join("work-leftover/.git/pack"), "x").unwrap();
    fs::create_dir_all(base.join("keep-me")).unwrap();
    fs::write(base.join("abc.json"), "{}").unwrap();
    // 新しいものは期限内なので消さない。
    assert_eq!(proposal::sweep_stale_clones(Duration::from_secs(3600)), 0);
    assert!(base.join("work-leftover").exists());
    // 期限 0 なら古いものとして消す。work- 以外には触れない。
    assert_eq!(proposal::sweep_stale_clones(Duration::ZERO), 1);
    assert!(!base.join("work-leftover").exists());
    assert!(base.join("keep-me").exists() && base.join("abc.json").exists());
}

#[test]
fn sweep_does_not_follow_a_linked_proposals_directory() {
    // `proposals/<x>` 自体がリンク（Windows ではジャンクション）のとき、その先の `work-*` は消さない。
    let f = Fixture::new();
    let base = proposal::proposals_dir(&f.root);
    let outside = f.base.join("outside-target");
    fs::create_dir_all(outside.join("work-victim/.git")).unwrap();
    fs::write(outside.join("work-victim/keep.txt"), "x").unwrap();
    let parent = base.parent().unwrap();
    fs::create_dir_all(parent).unwrap();
    let link = parent.join("linked-repo");
    #[cfg(windows)]
    {
        let status = Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(status.status.success(), "mklink /J に失敗");
    }
    #[cfg(not(windows))]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    assert_eq!(proposal::sweep_stale_clones(Duration::ZERO), 0);
    assert!(outside.join("work-victim/keep.txt").exists());
    // リンクは fixture の一時フォルダー内に残す（削除は一時フォルダーの清掃でまとめて行う）。
}

#[test]
fn clone_checkout_failure_stops_and_leaves_no_clone() {
    let f = Fixture::new();
    // 存在しないコミットは checkout できない。止めてクローンを消す。
    let base = proposal::proposals_dir(&f.root);
    let bogus = "0123456789abcdef0123456789abcdef01234567";
    assert!(proposal::prepare_clone(&f.root, bogus, &base).is_err());
    assert!(f.leftover_clones().is_empty());
    // コミット ID の形式でないものは git を呼ぶ前に拒否する。
    assert!(proposal::prepare_clone(&f.root, "--upload-pack=x", &base).is_err());
}
