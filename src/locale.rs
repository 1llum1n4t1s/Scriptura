// 人が読む固定文言（CLI の出力・ヘルプ・エラー・フックの通知・Setup 版の案内・管理画面）を、
// 利用者の言語に合わせて出す。共通仕様 `D:\Dev\APP-DISTRIBUTION.md` の「8. ローカライズ」の実装。
//
// 判定順（CLI・フック・Setup 版の案内）:
// 1. SCRIPTURA_LANGUAGE（明示の上書き。テストや、OS と別の言語で使いたい利用者向け）
// 2. POSIX の LC_ALL → LC_MESSAGES → LANG（空・C・POSIX は「未設定」とみなして次へ進む）
//    Windows でも Claude Code のフックは Git Bash 経由で起動されることがあり、その場合は LANG が入る。
// 3. OS の表示言語（Windows は GetUserDefaultUILanguage、macOS は AppleLanguages の先頭）
// 4. どれも判定できない・未対応の言語なら英語
//
// 管理画面（feature "ui"）は要求ごとに Cookie → Accept-Language → 英語で決め（`ui::locale`）、
// その要求の処理中だけ `current()` がその言語を返すよう tokio の task-local で包む。
// spawn_blocking へ渡す処理は task-local が引き継がれないため、`scoped` で同じ言語を明示する。
//
// 辞書は言語ごとの JSON（`src/locales/<タグ>.json`）。キーは安定した識別子（`cli.status.title` など）。
// 辞書に無いキーは英語、英語にも無ければキーそのものを返す（落ちない）。プレースホルダーは `{name}`。
// AI が読む文言（worker へのプロンプト）・保存データ・識別子・技術的なエラー詳細は訳さない。
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Japanese,
    English,
    ChineseSimplified,
    ChineseTraditional,
    Korean,
    Spanish,
    French,
    German,
    PortugueseBrazil,
}

impl Language {
    /// 対応する全言語。辞書ファイルの一覧・整合テスト・画面の言語の選択欄はこの並びを基準にする。
    pub const ALL: [Language; 9] = [
        Language::Japanese,
        Language::English,
        Language::ChineseSimplified,
        Language::ChineseTraditional,
        Language::Korean,
        Language::Spanish,
        Language::French,
        Language::German,
        Language::PortugueseBrazil,
    ];

    /// 辞書ファイル名・`<html lang>`・Cookie の値にも使う言語タグ。
    pub fn tag(self) -> &'static str {
        match self {
            Language::Japanese => "ja",
            Language::English => "en",
            Language::ChineseSimplified => "zh-CN",
            Language::ChineseTraditional => "zh-TW",
            Language::Korean => "ko",
            Language::Spanish => "es",
            Language::French => "fr",
            Language::German => "de",
            Language::PortugueseBrazil => "pt-BR",
        }
    }

    /// 言語の選択欄に出す、その言語自身での名前（訳さない）。
    pub fn native_name(self) -> &'static str {
        match self {
            Language::Japanese => "日本語",
            Language::English => "English",
            Language::ChineseSimplified => "简体中文",
            Language::ChineseTraditional => "繁體中文",
            Language::Korean => "한국어",
            Language::Spanish => "Español",
            Language::French => "Français",
            Language::German => "Deutsch",
            Language::PortugueseBrazil => "Português (Brasil)",
        }
    }

    /// 言語タグ（`ja` / `zh-CN` など、`tag()` と完全一致）から言語を返す。Cookie の値の検証に使う。
    pub fn from_tag(tag: &str) -> Option<Language> {
        Language::ALL.into_iter().find(|l| l.tag() == tag)
    }

    /// 辞書の JSON 原文（コンパイル時に埋め込む）。
    pub fn dictionary_source(self) -> &'static str {
        match self {
            Language::Japanese => include_str!("locales/ja.json"),
            Language::English => include_str!("locales/en.json"),
            Language::ChineseSimplified => include_str!("locales/zh-CN.json"),
            Language::ChineseTraditional => include_str!("locales/zh-TW.json"),
            Language::Korean => include_str!("locales/ko.json"),
            Language::Spanish => include_str!("locales/es.json"),
            Language::French => include_str!("locales/fr.json"),
            Language::German => include_str!("locales/de.json"),
            Language::PortugueseBrazil => include_str!("locales/pt-BR.json"),
        }
    }

    fn index(self) -> usize {
        Language::ALL
            .iter()
            .position(|language| *language == self)
            .expect("every language is listed")
    }
}

/// プロセスの言語（環境変数・OS から 1 回だけ判定する。macOS は子プロセスを使うため使い回す）。
static DETECTED: OnceLock<Language> = OnceLock::new();

thread_local! {
    /// `scoped` で一時的に指定した言語（管理画面の spawn_blocking の中など）。
    static SCOPED: Cell<Option<Language>> = const { Cell::new(None) };
}

#[cfg(feature = "ui")]
tokio::task_local! {
    /// 管理画面の要求 1 件の言語（`ui::locale` のミドルウェアが設定する）。
    static REQUEST: Language;
}

/// 現在の言語。管理画面の要求処理中はその要求の言語、`scoped` の中は指定した言語、
/// それ以外はプロセスの言語（初回呼び出しで判定）。
pub fn current() -> Language {
    #[cfg(feature = "ui")]
    if let Ok(language) = REQUEST.try_with(|l| *l) {
        return language;
    }
    if let Some(language) = SCOPED.with(Cell::get) {
        return language;
    }
    *DETECTED.get_or_init(detect)
}

/// `f` の実行中だけ、このスレッドの言語を `language` にする（戻ったら元に戻す。panic でも戻す）。
pub fn scoped<R>(language: Language, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Language>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SCOPED.with(|cell| cell.set(self.0));
        }
    }
    let _restore = Restore(SCOPED.with(|cell| cell.replace(Some(language))));
    f()
}

/// 管理画面の要求 1 件を `language` で処理する（tokio の task-local。await をまたいでも保たれる）。
#[cfg(feature = "ui")]
pub async fn scope_request<F: std::future::Future>(language: Language, fut: F) -> F::Output {
    REQUEST.scope(language, fut).await
}

fn detect() -> Language {
    detect_with(|name| std::env::var(name).ok(), os_language)
}

/// 判定の本体（環境変数の読み方と OS の言語を差し替えられる純粋な形。テストはこちらを使う）。
/// 明示の上書きと POSIX のロケール変数は、空・C・POSIX 以外なら最初に見つかった値で決める
/// （未対応の言語なら、その時点で英語に決まる。後ろの変数へは進まない）。
pub fn detect_with(
    env: impl Fn(&str) -> Option<String>,
    os: impl FnOnce() -> Option<Language>,
) -> Language {
    for name in ["SCRIPTURA_LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(language) = env(name).as_deref().and_then(parse) {
            return language;
        }
    }
    os().unwrap_or(Language::English)
}

/// ロケール識別子（`ja`・`ja_JP.UTF-8`・`zh-Hant-TW`・`pt_BR`・`Japanese_Japan.932` など）を
/// 対応言語に照合する。未対応の言語は英語。空文字・`C`・`POSIX`（`C.UTF-8` を含む）は判定材料に
/// ならないため None を返す。
pub fn parse(value: &str) -> Option<Language> {
    let value = value.trim();
    let base = value
        .split(['.', '@'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if base.is_empty() || base == "c" || base == "posix" {
        return None;
    }
    Some(supported(&base).unwrap_or(Language::English))
}

/// 言語タグを対応言語に照合する（未対応なら None）。Accept-Language の照合はこちらを使い、
/// 未対応の言語を飛ばして次の候補へ進める。
///
/// 照合規則（共通仕様どおり）: `zh-Hans`・`zh-CN`・`zh-SG` は zh-CN、`zh-Hant`・`zh-TW`・`zh-HK`・
/// `zh-MO` は zh-TW、`pt`（`pt-BR` 以外の `pt-*` を含む）は pt-BR、それ以外は主言語（`ja-JP` → ja）。
/// 文字体系（Hans/Hant）は地域より優先し、`zh` だけの指定は簡体字とする。
pub fn supported(tag: &str) -> Option<Language> {
    let base = tag.trim().to_ascii_lowercase();
    let mut subtags = base.split(['_', '-']);
    let primary = subtags.next().unwrap_or_default();
    let rest: Vec<&str> = subtags.collect();
    Some(match primary {
        "ja" | "japanese" => Language::Japanese,
        "en" | "english" => Language::English,
        "zh" => {
            if rest.contains(&"hant") {
                Language::ChineseTraditional
            } else if rest.contains(&"hans") {
                Language::ChineseSimplified
            } else if rest.iter().any(|t| ["tw", "hk", "mo"].contains(t)) {
                Language::ChineseTraditional
            } else {
                Language::ChineseSimplified
            }
        }
        // Windows の英語名ロケール（`Chinese (Traditional)_Taiwan.950` など）。
        name if name.starts_with("chinese") => {
            if ["traditional", "taiwan", "hong kong", "macao", "macau"]
                .iter()
                .any(|marker| base.contains(marker))
            {
                Language::ChineseTraditional
            } else {
                Language::ChineseSimplified
            }
        }
        "ko" | "korean" => Language::Korean,
        "es" | "spanish" => Language::Spanish,
        "fr" | "french" => Language::French,
        "de" | "german" => Language::German,
        "pt" | "portuguese" => Language::PortugueseBrazil,
        _ => return None,
    })
}

#[cfg(windows)]
fn os_language() -> Option<Language> {
    // LANGID の下位10bitが主言語、上位6bitが副言語。中国語は副言語で簡体字・繁体字を分ける。
    let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
    Some(language_from_langid(id))
}

/// Windows の LANGID を対応言語へ変換する。未対応の言語は英語。
pub fn language_from_langid(id: u16) -> Language {
    match id & 0x3ff {
        0x11 => Language::Japanese,
        0x12 => Language::Korean,
        0x0a => Language::Spanish,
        0x0c => Language::French,
        0x07 => Language::German,
        // pt-PT（0x0816）も pt-BR として扱う。
        0x16 => Language::PortugueseBrazil,
        0x04 => {
            // 副言語: 0x01 台湾・0x03 香港・0x05 マカオ・0x1f（zh-Hant 0x7c04）は繁体字。
            match id >> 10 {
                0x01 | 0x03 | 0x05 | 0x1f => Language::ChineseTraditional,
                _ => Language::ChineseSimplified,
            }
        }
        _ => Language::English,
    }
}

#[cfg(target_os = "macos")]
fn os_language() -> Option<Language> {
    // GUI から起動された場合などは LANG が無いことがあるため、OS の言語順の先頭を読む。
    // 出力は `(\n    "ja-JP",\n    "en-JP"\n)` の形式。
    let output = std::process::Command::new("/usr/bin/defaults")
        .args(["read", "-g", "AppleLanguages"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text
        .split(['(', ')', ',', '\n'])
        .map(|part| part.trim().trim_matches('"'))
        .find(|part| !part.is_empty())?;
    parse(first)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn os_language() -> Option<Language> {
    None
}

type Dictionary = HashMap<String, String>;

/// 言語ごとの辞書。初回参照時に JSON を読み、以後は使い回す（壊れた辞書は空として扱い、英語へ落とす。
/// 壊れた辞書は整合テストが検出する）。
fn dictionary(language: Language) -> &'static Dictionary {
    static DICTIONARIES: [OnceLock<Dictionary>; 9] = [const { OnceLock::new() }; 9];
    DICTIONARIES[language.index()]
        .get_or_init(|| serde_json::from_str(language.dictionary_source()).unwrap_or_default())
}

/// 指定した言語でキーの文言を返す。辞書に無ければ英語、英語にも無ければキーそのもの。
pub fn tr_in(language: Language, key: &'static str) -> &'static str {
    dictionary(language)
        .get(key)
        .or_else(|| dictionary(Language::English).get(key))
        .map_or(key, String::as_str)
}

/// 現在の言語でキーの文言を返す。
pub fn tr(key: &'static str) -> &'static str {
    tr_in(current(), key)
}

/// `{name}` 形式のプレースホルダーを値に置き換える。1 回の走査で置き換えるので、値の中に `{x}` が
/// あっても再展開しない（利用者のパスや設定値をそのまま出す）。値の無い `{...}` はそのまま残す。
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut rest = template;
    let mut result = String::with_capacity(template.len());
    while let Some((prefix, token)) = rest.split_once('{') {
        result.push_str(prefix);
        match token.split_once('}') {
            Some((name, suffix)) => {
                if let Some((_, value)) = values.iter().find(|(field, _)| *field == name) {
                    result.push_str(value);
                } else {
                    result.push('{');
                    result.push_str(name);
                    result.push('}');
                }
                rest = suffix;
            }
            None => {
                result.push('{');
                rest = token;
            }
        }
    }
    result.push_str(rest);
    result
}

/// 現在の言語でキーの文言を返し、プレースホルダーを置き換える。
pub fn tr_with(key: &'static str, values: &[(&str, &str)]) -> String {
    fill(tr(key), values)
}

/// 文言の参照。`t!("key")` は `&'static str`、`t!("key", name = 値, ...)` は `String`
/// （値は `Display` で文字列にする）。整合テストはソースの `t!("...")` を走査してキーの存在を確かめるので、
/// キーは必ず文字列リテラルで書く。
#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::locale::tr($key)
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::locale::tr_with(
            $key,
            &[$((stringify!($name), ($value).to_string().as_str())),+],
        )
    };
}

/// テンプレート用: プレースホルダー 1 つの文言（askama の式から呼ぶ。キーは文字列リテラルで書く）。
pub fn tr1(key: &'static str, name: &str, value: impl std::fmt::Display) -> String {
    fill(tr(key), &[(name, value.to_string().as_str())])
}

/// テンプレート用: プレースホルダー 2 つの文言。
pub fn tr2(
    key: &'static str,
    name1: &str,
    value1: impl std::fmt::Display,
    name2: &str,
    value2: impl std::fmt::Display,
) -> String {
    fill(
        tr(key),
        &[
            (name1, value1.to_string().as_str()),
            (name2, value2.to_string().as_str()),
        ],
    )
}
