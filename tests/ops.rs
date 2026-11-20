// 設定の保存経路（版・楽観的排他・変更履歴・設定ロック）と ops の操作のテスト。
//
// 隔離: 場所は `config::Store` で明示し、環境変数（SCRIPTURA_* など）は一切触らない。
// 実環境の設定・状態フォルダーは読み書きしない。fixture の置き場は `SCRIPTURA_TEST_BASE`
// （未指定なら cargo の CARGO_TARGET_TMPDIR）。
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use scriptura::config::{
    self, Config, Document, HISTORY_LIMIT, KindSettings, Loaded, MigratedFrom, Repository,
    Revision, Store, UpdateError,
};
use scriptura::ops::{self, EnablePlan};

fn base() -> PathBuf {
    let parent = std::env::var_os("SCRIPTURA_TEST_BASE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")));
    let dir = parent.join(format!("ops-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 設定ファイル（base/config/config.toml）と状態フォルダー（base/state）を分けた場所。
fn store() -> Store {
    let b = base();
    Store::new(b.join("config").join("config.toml"), b.join("state"))
}

fn repo(root: &str) -> Repository {
    Repository {
        root: root.into(),
        remote: None,
        branches: Vec::new(),
        documents: None,
        exclude: Vec::new(),
        paused: false,
        migrated_from: None,
    }
}

/// 登録 1 件を持つ設定を保存する（既存の save_to を使う。履歴は作らない）。
fn seed(store: &Store) -> Config {
    let mut c = Config::default();
    c.repository.push(repo("/w/one"));
    config::save_to(&store.config_file, &c).unwrap();
    c
}

fn current(store: &Store) -> Revision {
    config::read_at(&store.config_file).revision
}

fn history_ids(store: &Store) -> Vec<String> {
    config::history_in(store)
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect()
}

// ---------------------------------------------------------------------------
// Revision
// ---------------------------------------------------------------------------

#[test]
fn revision_is_the_sha256_of_the_file_bytes_or_missing() {
    let s = store();
    assert_eq!(current(&s), Revision::missing());
    assert!(current(&s).is_missing());
    assert_eq!(current(&s).as_str(), "missing");
    seed(&s);
    let bytes = fs::read(&s.config_file).unwrap();
    let rev = current(&s);
    assert_eq!(rev, Revision::of(Some(&bytes)));
    assert_eq!(rev.as_str().len(), 64);
    assert_eq!(rev.short(), &rev.as_str()[..12]);
    // 1 バイトでも違えば版も違う。
    let mut changed = bytes.clone();
    changed.push(b'\n');
    assert_ne!(Revision::of(Some(&changed)), rev);
    // 外部から受け取る値の解釈は厳密（形式外は None）。
    assert_eq!(Revision::parse(rev.as_str()), Some(rev.clone()));
    assert_eq!(Revision::parse("missing"), Some(Revision::missing()));
    for bad in [
        "",
        "MISSING",
        &rev.as_str()[..63],
        &rev.as_str().to_uppercase(),
        "../config.toml",
    ] {
        assert_eq!(Revision::parse(bad), None, "{bad}");
    }
    // serde でも同じ規則（フォームの値などをそのまま受けられる）。
    let json = serde_json::to_string(&rev).unwrap();
    assert_eq!(serde_json::from_str::<Revision>(&json).unwrap(), rev);
    assert!(serde_json::from_str::<Revision>("\"zz\"").is_err());
}

// ---------------------------------------------------------------------------
// update: 新規作成・版の照合・衝突・変更なし・検証
// ---------------------------------------------------------------------------

#[test]
fn update_creates_saves_and_reports_revisions() {
    let s = store();
    // 無い状態から作る（期待する版 missing と一致）。退避は無い。
    let u = config::update_in(&s, Some(&Revision::missing()), |c, existed| {
        assert!(!existed);
        assert_eq!(*c, Config::default());
        c.defaults.exclude = vec!["x/**".into()];
        Ok(7)
    })
    .unwrap();
    assert_eq!(u.value, 7);
    assert!(u.created && u.saved);
    assert_eq!(u.previous, Revision::missing());
    assert_eq!(u.backup, None);
    assert_eq!(u.revision, current(&s), "返す版は保存したファイルの版");
    assert_eq!(
        config::load_from(&s.config_file),
        Loaded::Ok(u.config.clone())
    );
    assert!(history_ids(&s).is_empty());

    // 期待する版つきの変更。直前の内容がそのまま退避される。
    let before = fs::read(&s.config_file).unwrap();
    let u2 = config::update_in(&s, Some(&u.revision), |c, existed| {
        assert!(existed);
        c.worker.timeout_secs = 300;
        Ok(())
    })
    .unwrap();
    assert!(u2.saved && !u2.created);
    assert_eq!(u2.previous, u.revision);
    let backup = u2.backup.clone().unwrap();
    assert_eq!(
        fs::read(&backup).unwrap(),
        before,
        "退避は生のバイト列そのもの"
    );
    assert!(backup.starts_with(s.history_dir()));
    let ids = history_ids(&s);
    assert_eq!(ids.len(), 1);
    assert!(ids[0].ends_with(&format!("-{}.toml", u.revision.short())));
    assert!(config::is_history_id(&ids[0]));
    // 設定フォルダーには config.toml だけが残る（履歴・ロックは状態フォルダー）。
    let names: Vec<_> = fs::read_dir(s.config_file.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("config.toml")]);
    assert!(s.lock_path().exists());
}

#[test]
fn stale_revision_is_a_conflict_and_writes_nothing() {
    let s = store();
    seed(&s);
    let stale = current(&s);
    // 他（外部の編集）で変更される。
    let mut c = match config::load_from(&s.config_file) {
        Loaded::Ok(c) => c,
        other => panic!("{other:?}"),
    };
    c.worker.timeout_secs = 600;
    config::save_to(&s.config_file, &c).unwrap();
    let after_external = fs::read(&s.config_file).unwrap();
    let mut called = false;
    let err = config::update_in(&s, Some(&stale), |c, _| {
        called = true;
        c.worker.timeout_secs = 900;
        Ok(())
    })
    .unwrap_err();
    assert!(!called, "衝突したら変更関数を呼ばない");
    assert_eq!(
        err,
        UpdateError::Conflict {
            expected: stale.clone(),
            actual: Revision::of(Some(&after_external)),
        }
    );
    assert!(err.to_string().contains("他で変更"));
    assert_eq!(fs::read(&s.config_file).unwrap(), after_external);
    assert!(history_ids(&s).is_empty());
    // 無いはずのファイルが在る場合も衝突（新規作成の競合）。
    let err = config::update_in(&s, Some(&Revision::missing()), |_, _| Ok(())).unwrap_err();
    assert!(matches!(err, UpdateError::Conflict { .. }));
    // ops の操作も同じ（ロック前の事前確認で止まる）。
    let err = ops::set_paused(&s, "/w/one", true, Some(&stale)).unwrap_err();
    assert!(matches!(err, UpdateError::Conflict { .. }));
    assert_eq!(fs::read(&s.config_file).unwrap(), after_external);
}

#[test]
fn unchanged_content_is_not_saved_and_not_backed_up() {
    let s = store();
    seed(&s);
    let before = fs::read(&s.config_file).unwrap();
    let rev = current(&s);
    let u = config::update_in(&s, Some(&rev), |_, _| Ok(())).unwrap();
    assert!(!u.saved && u.backup.is_none());
    assert_eq!(u.revision, rev);
    assert_eq!(fs::read(&s.config_file).unwrap(), before);
    // pause を繰り返しても履歴は 1 件しか増えない。
    assert!(ops::set_paused(&s, "/w/one", true, None).unwrap().changed);
    assert!(!ops::set_paused(&s, "/w/one", true, None).unwrap().changed);
    assert_eq!(history_ids(&s).len(), 1);
}

#[test]
fn invalid_result_and_rejection_write_nothing() {
    let s = store();
    seed(&s);
    let before = fs::read(&s.config_file).unwrap();
    let err = config::update_in(&s, None, |c, _| {
        c.worker.timeout_secs = 5;
        Ok(())
    })
    .unwrap_err();
    assert!(
        matches!(err, UpdateError::Invalid(ref r) if r.contains("timeout_secs")),
        "{err:?}"
    );
    let err = config::update_in(&s, None, |_, _| Err::<(), _>("拒否".to_string())).unwrap_err();
    assert_eq!(err, UpdateError::Rejected("拒否".into()));
    assert_eq!(fs::read(&s.config_file).unwrap(), before);
    assert!(history_ids(&s).is_empty());
    // 対象に挙げたカスタム種類の定義は消せない（検証に通らない）。
    let custom = KindSettings {
        paths: vec!["docs/ARCHITECTURE.md".into()],
        guidance: Some("構成を書く".into()),
        ..KindSettings::default()
    };
    ops::set_kind(&s, "architecture", Some(custom), None).unwrap();
    ops::edit(&s, None, |c| {
        c.repository[0].documents = Some(vec![
            Document::Readme,
            Document::Custom("architecture".into()),
        ]);
        Ok(())
    })
    .unwrap();
    let before = fs::read(&s.config_file).unwrap();
    let err = ops::set_kind(&s, "architecture", None, None).unwrap_err();
    assert!(
        matches!(err, UpdateError::Invalid(ref r) if r.contains("architecture")),
        "{err:?}"
    );
    assert_eq!(fs::read(&s.config_file).unwrap(), before);
    // id の形式外は拒否。
    assert!(matches!(
        ops::set_kind(&s, "../x", None, None),
        Err(UpdateError::Rejected(_))
    ));
}

// ---------------------------------------------------------------------------
// 壊れた設定は書き換えない
// ---------------------------------------------------------------------------

#[test]
fn broken_config_is_never_rewritten() {
    let s = store();
    fs::create_dir_all(s.config_file.parent().unwrap()).unwrap();
    let broken = "this = is = broken\n";
    fs::write(&s.config_file, broken).unwrap();
    // ops の書き込み系はロックを取る前に拒否する（状態フォルダーを作らない）。
    for err in [
        ops::set_paused(&s, "/w/one", true, None).unwrap_err(),
        ops::disable(&s, "/w/one", None).unwrap_err(),
    ] {
        assert!(matches!(err, UpdateError::Broken { .. }), "{err:?}");
        let message = err.to_string();
        assert!(message.contains("壊れている"), "{message}");
        // 理由は先頭行だけ（設定ファイルの内容を出力に写さない）。
        assert!(!message.contains('\n'), "{message}");
    }
    assert!(
        !s.state_dir.exists(),
        "拒否しただけで状態フォルダーを作らない"
    );
    // update_in は版が一致していても書き換えない。
    let rev = current(&s);
    let err = config::update_in(&s, Some(&rev), |c, _| {
        *c = Config::default();
        Ok(())
    })
    .unwrap_err();
    assert!(matches!(err, UpdateError::Broken { .. }));
    // 非 UTF-8 も同じ。
    assert_eq!(fs::read_to_string(&s.config_file).unwrap(), broken);
    fs::write(&s.config_file, [0xff, 0xfe, 0x00]).unwrap();
    assert!(matches!(
        ops::edit(&s, None, |_| Ok(())),
        Err(UpdateError::Broken { .. })
    ));
    assert_eq!(fs::read(&s.config_file).unwrap(), [0xff, 0xfe, 0x00]);
    assert!(history_ids(&s).is_empty());
}

#[test]
fn missing_config_or_unregistered_repo_is_refused_without_touching_state() {
    let s = store();
    for err in [
        ops::set_paused(&s, "/w/one", true, None).unwrap_err(),
        ops::disable(&s, "/w/one", None).unwrap_err(),
    ] {
        assert_eq!(
            err,
            UpdateError::Rejected("設定ファイルがありません（未登録です）".into())
        );
    }
    assert!(!s.state_dir.exists());
    assert!(!s.config_file.exists());
    seed(&s);
    let err = ops::disable(&s, "/w/other", None).unwrap_err();
    assert_eq!(err.to_string(), "登録されていないリポジトリです: /w/other");
    assert!(!s.state_dir.exists());
    // 登録済みなら解除できる（ほかの設定は残る）。
    let done = ops::disable(&s, "/w/one", None).unwrap();
    assert_eq!(done.remaining, 0);
    match config::load_from(&s.config_file) {
        Loaded::Ok(c) => assert!(c.repository.is_empty() && c.schema == 3),
        other => panic!("{other:?}"),
    }
}

#[test]
fn migrated_registrations_cannot_be_resumed_but_enable_clears_the_origin() {
    // 旧形式から移行で止めた登録は、一時停止の解除（再開）では動かせない。enable だけが解除する。
    let s = store();
    let mut c = Config::default();
    let mut stopped = repo("/w/one");
    stopped.paused = true;
    stopped.migrated_from = Some(MigratedFrom::Propose);
    c.repository.push(stopped);
    let mut plain = repo("/w/two");
    plain.paused = true;
    c.repository.push(plain);
    config::save_to(&s.config_file, &c).unwrap();
    let before = fs::read(&s.config_file).unwrap();
    match ops::set_paused(&s, "/w/one", false, None) {
        Err(UpdateError::Rejected(m)) => {
            assert!(m.contains("enable"), "{m}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(&s.config_file).unwrap(), before, "何も書かない");
    // 通常の一時停止は再開できる。止めたままにする（pause）のは移行由来でも問題ない。
    let u = ops::set_paused(&s, "/w/two", false, None).unwrap();
    assert!(u.changed);
    let u = ops::set_paused(&s, "/w/one", true, None).unwrap();
    assert!(!u.changed);
    // enable の適用は由来と一時停止を解除する（登録の他の項目は保つ）。
    let mut after = c.clone();
    let index = ops::apply_enable(&mut after, &plan());
    assert_eq!(index, 2, "新しい登録として足される");
    let plan_for_one = EnablePlan {
        root: "/w/one".into(),
        ..plan()
    };
    let index = ops::apply_enable(&mut after, &plan_for_one);
    assert_eq!(after.repository[index].root, "/w/one");
    assert!(!after.repository[index].paused);
    assert_eq!(after.repository[index].migrated_from, None);
}

// ---------------------------------------------------------------------------
// 変更履歴: 件数上限・退避内容・id の検証・復元
// ---------------------------------------------------------------------------

#[test]
fn history_keeps_the_newest_entries_up_to_the_limit() {
    let s = store();
    seed(&s);
    // 利用者が置いた別のファイルは刈り込みの対象外。
    fs::create_dir_all(s.history_dir()).unwrap();
    let foreign = s.history_dir().join("note.txt");
    fs::write(&foreign, "keep").unwrap();
    let total = HISTORY_LIMIT + 5;
    let mut previous_bytes = Vec::new();
    for i in 0..total {
        previous_bytes.push(fs::read(&s.config_file).unwrap());
        ops::edit(&s, Some(&current(&s)), |c| {
            c.defaults.exclude = vec![format!("p{i}/**")];
            Ok(())
        })
        .unwrap();
    }
    let entries = config::history_in(&s).unwrap();
    assert_eq!(entries.len(), HISTORY_LIMIT);
    // 新しい順。名前順＝時系列（連続した更新でも順序が崩れない）。
    let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.reverse();
    assert_eq!(ids, sorted);
    // 残るのは最新の 50 件で、各退避はその時点の変更前の内容そのもの。
    for (entry, expected) in entries.iter().zip(previous_bytes.iter().rev()) {
        assert_eq!(&fs::read(&entry.path).unwrap(), expected, "{}", entry.id);
        assert_eq!(entry.revision_prefix, Revision::of(Some(expected)).short());
        assert_eq!(entry.size, expected.len() as u64);
        assert!(entry.saved_at.ends_with('Z'));
        let (bytes, loaded) = ops::read_history(&s, &entry.id).unwrap();
        assert_eq!(&bytes, expected);
        assert!(matches!(loaded, Loaded::Ok(_)));
    }
    assert!(foreign.exists());
    // 一時ファイルが残らない。
    let leftovers: Vec<_> = fs::read_dir(s.history_dir())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn history_ids_are_strictly_validated() {
    let s = store();
    seed(&s);
    ops::set_paused(&s, "/w/one", true, None).unwrap();
    let id = history_ids(&s).remove(0);
    assert!(config::is_history_id(&id));
    // 設定ファイルを履歴フォルダーの隣に置いても、相対パスでは読めない。
    fs::copy(&s.config_file, s.state_dir.join("config.toml")).unwrap();
    for bad in [
        "../config.toml",
        "..\\config.toml",
        "",
        "note.txt",
        &format!("../{id}"),
        &format!("{id}/"),
        &id.replace(".toml", ".TOML"),
        &id.replace('T', ":"),
        "20261115T123456.123456Z-ABCDEF012345.toml",
        "20261115T123456.123456Z-abcdef01234.toml",
        "2026111XT123456.123456Z-abcdef012345.toml",
        "20261399T123456.123456Z-abcdef012345.toml",
    ] {
        assert!(!config::is_history_id(bad), "{bad}");
        assert!(ops::read_history(&s, bad).is_err(), "{bad}");
        assert!(ops::restore(&s, bad, None).is_err(), "{bad}");
    }
    // 形式は正しいが存在しない id。
    let absent = "20000101T000000.000000Z-000000000000.toml";
    assert!(config::is_history_id(absent));
    assert!(ops::read_history(&s, absent).is_err());
}

#[test]
fn restore_goes_through_update_and_keeps_the_pre_restore_content() {
    let s = store();
    seed(&s);
    let original = fs::read(&s.config_file).unwrap();
    ops::edit(&s, None, |c| {
        c.worker.timeout_secs = 300;
        Ok(())
    })
    .unwrap();
    let paused_bytes = fs::read(&s.config_file).unwrap();
    let id = history_ids(&s).remove(0);
    let (bytes, _) = ops::read_history(&s, &id).unwrap();
    assert_eq!(bytes, original);
    // 古い版を期待すると衝突。
    let stale = Revision::of(Some(&original));
    assert!(matches!(
        ops::restore(&s, &id, Some(&stale)),
        Err(UpdateError::Conflict { .. })
    ));
    // 現在の版を期待して復元する。
    let u = ops::restore(&s, &id, Some(&current(&s))).unwrap();
    assert!(u.saved);
    match config::load_from(&s.config_file) {
        Loaded::Ok(c) => assert_eq!(c.worker.timeout_secs, 1200),
        other => panic!("{other:?}"),
    }
    // 復元前の内容（変更後）も履歴に残る。
    let entries = config::history_in(&s).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(fs::read(&entries[0].path).unwrap(), paused_bytes);
    // 今の版で読めない履歴（壊れた退避）は復元しない。
    let bad_id = "20000101T000000.000000Z-000000000000.toml";
    fs::write(s.history_dir().join(bad_id), "broken = = =").unwrap();
    let before = fs::read(&s.config_file).unwrap();
    assert!(matches!(
        ops::restore(&s, bad_id, None),
        Err(UpdateError::Invalid(_))
    ));
    assert_eq!(fs::read(&s.config_file).unwrap(), before);
}

// ---------------------------------------------------------------------------
// 設定ロック
// ---------------------------------------------------------------------------

#[test]
fn concurrent_updates_do_not_lose_changes() {
    let s = Arc::new(store());
    seed(&s);
    const N: usize = 12;
    let threads: Vec<_> = (0..2)
        .map(|t| {
            let s = Arc::clone(&s);
            std::thread::spawn(move || {
                for i in 0..N {
                    // 期待する版なし（CLI と同じ）の read-modify-write。ロックが無いと後勝ちで消える。
                    ops::edit(&s, None, |c| {
                        c.defaults.exclude.push(format!("t{t}-{i}/**"));
                        Ok(())
                    })
                    .unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    match config::load_from(&s.config_file) {
        Loaded::Ok(c) => assert_eq!(c.defaults.exclude.len(), 2 * N, "{:?}", c.defaults.exclude),
        other => panic!("{other:?}"),
    }
    assert_eq!(history_ids(&s).len(), 2 * N);
}

#[test]
fn update_waits_for_the_config_lock() {
    let s = store();
    seed(&s);
    fs::create_dir_all(&s.state_dir).unwrap();
    // 別の書き手がロックを持っている状態を作り、少ししてから外す。
    let holder = rusqlite::Connection::open(s.lock_path()).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        drop(holder);
    });
    let started = Instant::now();
    ops::set_paused(&s, "/w/one", true, None).unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "ロックが外れるまで待つ: {:?}",
        started.elapsed()
    );
    release.join().unwrap();
}

// ---------------------------------------------------------------------------
// enable の適用（純粋関数）
// ---------------------------------------------------------------------------

fn plan() -> EnablePlan {
    EnablePlan {
        root: "/w/new".into(),
        branches: vec!["main".into()],
        remote: Some("github.com/me/new".into()),
        documents: None,
    }
}

#[test]
fn apply_enable_registers_and_keeps_other_settings() {
    let mut c = Config::default();
    c.defaults.exclude = vec!["x/**".into()];
    let index = ops::apply_enable(&mut c, &plan());
    let r = &c.repository[index];
    assert_eq!(r.root, "/w/new");
    assert_eq!(r.branches, vec!["main".to_string()]);
    assert_eq!(r.remote.as_deref(), Some("github.com/me/new"));
    assert!(!r.paused && r.migrated_from.is_none());
    assert_eq!(c.defaults.exclude, vec!["x/**".to_string()]);
    // 2 回目は同じ登録を更新する（増えない）。
    assert_eq!(ops::apply_enable(&mut c, &plan()), index);
    assert_eq!(c.repository.len(), 1);
    c.validate().unwrap();
}

#[test]
fn store_current_uses_the_state_db_folder() {
    // 環境変数は読むだけ（書き換えない）。状態 DB の親フォルダーが基点であることだけを確かめる。
    let s = Store::current();
    let db = scriptura::maintenance::state_db_path();
    assert_eq!(
        Some(s.state_dir.as_path()),
        db.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .or(Some(Path::new(".")))
    );
    assert_eq!(s.lock_path(), s.state_dir.join("config-lock.sqlite3"));
    assert_eq!(s.history_dir(), s.state_dir.join("config-history"));
}

// ---------------------------------------------------------------------------
// 履歴の復元: 有効な登録が増える・有効に変わる版の拒否（起点の記録は tests/ui.rs で実リポジトリを使って確かめる）
// ---------------------------------------------------------------------------

#[test]
fn restore_refuses_to_bring_back_or_activate_a_registration() {
    // (1) 一時停止中の登録が有効に変わる復元。
    let s = store();
    seed(&s);
    ops::edit(&s, None, |c| {
        c.repository[0].paused = true;
        Ok(())
    })
    .unwrap();
    ops::edit(&s, None, |c| {
        c.repository[0].paused = false;
        Ok(())
    })
    .unwrap();
    // 最新の退避（直前の内容 = 一時停止中の版）は、有効な状態からの復元なので問題ない。
    let paused_id = history_ids(&s).remove(0);
    ops::restore(&s, &paused_id, None).unwrap();
    // 逆に、有効だった版へ戻す（停止中 → 有効）ことは拒否する。
    let active_id = history_ids(&s).remove(0);
    let before = fs::read(&s.config_file).unwrap();
    match ops::restore(&s, &active_id, None) {
        Err(UpdateError::Rejected(m)) => {
            assert!(m.contains("enable") && m.contains("/w/one"), "{m}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(&s.config_file).unwrap(), before, "何も書かない");

    // (2) 解除済みの有効な登録が戻る復元（起点の記録より前に拒否する）。
    let s = store();
    seed(&s);
    ops::edit(&s, None, |c| {
        c.repository.push(repo("/w/two"));
        Ok(())
    })
    .unwrap();
    ops::edit(&s, None, |c| {
        c.repository.retain(|r| r.root != "/w/two");
        Ok(())
    })
    .unwrap();
    let with_two = history_ids(&s).remove(0);
    let before = fs::read(&s.config_file).unwrap();
    assert!(matches!(
        ops::restore(&s, &with_two, None),
        Err(UpdateError::Rejected(m)) if m.contains("/w/two")
    ));
    assert_eq!(fs::read(&s.config_file).unwrap(), before);

    // (3) 移行由来の停止が有効に変わる復元も拒否する。
    let s = store();
    seed(&s);
    ops::edit(&s, None, |c| {
        c.repository[0].paused = true;
        c.repository[0].migrated_from = Some(MigratedFrom::Off);
        Ok(())
    })
    .unwrap();
    ops::edit(&s, None, |c| {
        c.repository[0].paused = true;
        c.repository[0].migrated_from = None;
        Ok(())
    })
    .unwrap();
    let migrated = history_ids(&s).remove(0);
    // 移行由来の停止 → 一時停止だけ（有効ではない）なので拒否しない。
    ops::restore(&s, &migrated, None).unwrap();
    // 純粋関数の判定も同じ（画面の事前確認が使う）。
    let mut current = Config::default();
    current.repository.push(repo("/w/one"));
    let mut restored = current.clone();
    assert_eq!(ops::restore_refusal(&current, &restored), None);
    current.repository[0].paused = true;
    assert!(ops::restore_refusal(&current, &restored).is_some());
    current.repository[0].paused = false;
    current.repository[0].migrated_from = Some(MigratedFrom::Propose);
    assert!(ops::restore_refusal(&current, &restored).is_some());
    // 既に有効な登録が有効のまま戻るのは拒否しない。
    assert_eq!(ops::restore_refusal(&restored, &restored), None);
    // 停止中の登録が増える復元は拒否しない（起点の記録を伴う）。
    restored.repository[0].paused = true;
    restored.repository[0].migrated_from = Some(MigratedFrom::Off);
    current.repository.clear();
    assert_eq!(ops::restore_refusal(&current, &restored), None);
}
