//! 互換の基準テスト（P0）。
//!
//! 従来 3 種（AGENTS・DESIGN・README）構成の保守プロンプト全文と、worker の引数列を、
//! 変更前の実装の出力そのままで `tests/golden/` に固定する。文書種類のデータ化・CHANGELOG 対応などで
//! 実装を拡張しても、従来構成の出力は 1 バイトも変えてはならない（変わったらこのテストが落ちる）。
//!
//! - プロンプトは相対パスだけで組み立てられ、検知 sha は固定文字列を渡すため環境に依存しない。
//!   ignore 判定だけは実際の `git check-ignore` を使うので、`.gitignore` を置いた使い捨てリポジトリを作る。
//! - 期待値の再生成は意図した契約変更のときだけ `SCRIPTURA_GOLDEN_UPDATE=1` で行う（ローカライズで worker のプロンプトを英語に統一したときに 1 回再生成した）。
//! - fixture の置き場は `SCRIPTURA_TEST_BASE`（未指定なら `validation/`）。固定名のフォルダーを再利用し、
//!   実行ごとにゴミを増やさない。
use scriptura::config::{Document, Permission, Worker};
use scriptura::maintenance::{Scope, build_maintenance_reason_for, worker_args};
use std::{env, fs, path::PathBuf, process::Command};

const PREVIOUS: &str = "1111111111111111111111111111111111111111";
const PUSHED: &str = "2222222222222222222222222222222222222222";

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
}

/// 期待値ファイルと比較する。CRLF 変換（autocrlf）は読み込み側で吸収し、それ以外は完全一致。
fn check(name: &str, actual: &str) {
    let path = golden_dir().join(name);
    if env::var_os("SCRIPTURA_GOLDEN_UPDATE").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, actual).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("基準ファイルを読めません {}: {e}", path.display()))
        .replace("\r\n", "\n");
    assert_eq!(actual, expected, "基準 {name} と一致しません");
}

/// `.gitignore` 付きの使い捨てリポジトリ（テストごとに別フォルダーにして、並列実行でも競合させない）。ignore 判定の文面（status と detail）を再現するために必要。
fn repo(name: &str, gitignore: &str) -> PathBuf {
    let parent = env::var_os("SCRIPTURA_TEST_BASE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("validation"));
    let root = parent.join("compat-golden").join(name);
    fs::create_dir_all(&root).unwrap();
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(status.success());
    fs::write(root.join(".gitignore"), gitignore).unwrap();
    root
}

fn present_scope(root: &PathBuf) -> Scope {
    Scope {
        agents: vec![root.join("AGENTS.md"), root.join("sub").join("AGENTS.md")],
        design: vec![root.join("DESIGN.md")],
        readme: vec![root.join("README.md")],
        size_only: vec![root.join("other").join("AGENTS.md")],
        other: Default::default(),
    }
}

fn prompt(root: &PathBuf, scope: &Scope, deleted: &[PathBuf], docs: &[Document]) -> String {
    build_maintenance_reason_for(root, scope, deleted, PREVIOUS, PUSHED, docs)
}

#[test]
fn legacy_prompts_are_byte_identical() {
    let root = repo("plain", "");
    let present = present_scope(&root);
    check(
        "prompt_all_present.txt",
        &prompt(&root, &present, &[], &Document::ALL),
    );
    // 3 種とも欠落（新規作成の指示）。
    check(
        "prompt_all_missing.txt",
        &prompt(&root, &Scope::default(), &[], &Document::ALL),
    );
    // 削除された対象がある（「前回存在した」の指示と削除の注記）。
    let deleted = vec![root.join("AGENTS.md"), root.join("DESIGN.md")];
    check(
        "prompt_all_deleted.txt",
        &prompt(&root, &Scope::default(), &deleted, &Document::ALL),
    );
    // 対象を絞ったときの文面（README のみは AGENTS・DESIGN が出ない契約）。
    check(
        "prompt_readme_only.txt",
        &prompt(&root, &present, &[], &[Document::Readme]),
    );
    check(
        "prompt_agents_only.txt",
        &prompt(&root, &present, &[], &[Document::Agents]),
    );
    check(
        "prompt_design_readme.txt",
        &prompt(&root, &present, &[], &[Document::Design, Document::Readme]),
    );
    check(
        "prompt_design_only_missing.txt",
        &prompt(&root, &Scope::default(), &[], &[Document::Design]),
    );
}

#[test]
fn legacy_prompt_with_ignored_documents_is_byte_identical() {
    let root = repo("ignored", "README.md\n/DESIGN.md\n");
    check(
        "prompt_all_ignored.txt",
        &prompt(&root, &present_scope(&root), &[], &Document::ALL),
    );
}

fn lines(args: &[String]) -> String {
    let mut out = args.join("\n");
    out.push('\n');
    out
}

#[test]
fn legacy_worker_args_are_identical() {
    let allowed: Vec<String> = [
        "AGENTS.md",
        "sub/AGENTS.md",
        "other/AGENTS.md",
        "DESIGN.md",
        "README.md",
        ".gitignore",
        "sub/.gitignore",
    ]
    .map(String::from)
    .to_vec();
    let exclude: Vec<String> =
        scriptura::config::effective_exclude(&[&["docs/private/**".to_string()]]);

    let bypass = Worker {
        permission: Permission::Bypass,
        ..Worker::default()
    };
    check(
        "args_bypass.txt",
        &lines(&worker_args(&bypass, &allowed, &exclude)),
    );

    let restricted = Worker::default();
    check(
        "args_restricted.txt",
        &lines(&worker_args(&restricted, &allowed, &exclude)),
    );
}
