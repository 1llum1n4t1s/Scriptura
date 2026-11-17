//! xtask の CLI。ロジックは lib.rs。使い方は `usage()` を参照。

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use xtask::*;

fn usage() -> &'static str {
    "使い方: cargo run --manifest-path xtask/Cargo.toml --locked -- <command>\n\
     \n\
     stage [--target <triple>] [--binary <absolute-path>]  本体を同梱先へコピー\n\
     checksums                                            SHA256SUMS を生成\n\
     package [--out <absolute-dir>]                        再現可能な zip を作成（既定 <OS の一時フォルダー>/scriptura-release）\n\
     marketplace-entry --url <https-url> --sha256 <hex>    archive ソースの項目 JSON を出力\n"
}

/// `--name value` 形式のオプションを取り出す。未知のオプションや値欠落はエラー。
fn parse_opts(args: &[String], allowed: &[&str]) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if !allowed.contains(&a.as_str()) {
            return Err(format!("不明なオプションです: {a}"));
        }
        let v = it.next().ok_or_else(|| format!("{a} に値がありません"))?;
        out.push((a.clone(), v.clone()));
    }
    Ok(out)
}

fn opt<'a>(opts: &'a [(String, String)], name: &str) -> Option<&'a str> {
    opts.iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn run(args: &[String]) -> Result<()> {
    // xtask/ の親（= リポジトリのルート）。コンパイル時に固定される。
    let root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("リポジトリのルートを特定できません")?
        .to_path_buf();
    let (cmd, rest) = args.split_first().ok_or_else(|| usage().to_string())?;
    match cmd.as_str() {
        "stage" => {
            let o = parse_opts(rest, &["--target", "--binary"])?;
            let dest = stage(
                &root,
                opt(&o, "--target"),
                opt(&o, "--binary").map(Path::new),
            )?;
            println!("同梱しました: {}", dest.display());
        }
        "checksums" => {
            parse_opts(rest, &[])?;
            let p = write_checksums(&root)?;
            println!("書き込みました: {}", p.display());
        }
        "package" => {
            let o = parse_opts(rest, &["--out"])?;
            let out = match opt(&o, "--out") {
                Some(d) => {
                    let d = PathBuf::from(d);
                    if !d.is_absolute() {
                        return Err("--out は絶対パスで指定してください".into());
                    }
                    d
                }
                None => default_out_dir(),
            };
            let (zip, sha) = package(&root, &out)?;
            eprintln!("作成しました: {}", zip.display());
            // 標準出力は SHA-256 のみ（後続の marketplace-entry --sha256 へ渡しやすくする）。
            println!("{sha}");
        }
        "marketplace-entry" => {
            let o = parse_opts(rest, &["--url", "--sha256"])?;
            let url = opt(&o, "--url").ok_or("--url が必要です")?;
            let sha = opt(&o, "--sha256").ok_or("--sha256 が必要です")?;
            println!("{}", marketplace_entry(&root, url, sha)?);
            // JSON にコメントは書けないため、未確認事項は標準エラーへ出す。
            eprintln!(
                "注意: 形式は公式ドキュメントで確認済み。未確認: 展開後の実行ビット保持、ダウンロード上限の具体値。"
            );
        }
        _ => return Err(usage().to_string()),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("エラー: {e}");
            ExitCode::FAILURE
        }
    }
}
