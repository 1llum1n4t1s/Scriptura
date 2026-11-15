//! ローカライズの整合テスト（共通仕様 `D:\Dev\APP-DISTRIBUTION.md` の 8 節）。
//!
//! - 全言語の辞書が同じキー集合・同じプレースホルダー集合を持つこと。
//! - コード・テンプレートが参照するキーが辞書（英語）にあること、辞書に使われないキーが無いこと。
//! - 言語タグの照合・CLI の言語判定の順序・Windows の言語 ID。
//! - 管理画面の言語判定（Cookie → Accept-Language → 英語）と、言語の選択欄（POST /language）。
use scriptura::locale::{self, Language};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn dictionary(language: Language) -> BTreeMap<String, String> {
    serde_json::from_str(language.dictionary_source())
        .unwrap_or_else(|e| panic!("{} の辞書が JSON として読めません: {e}", language.tag()))
}

/// `{name}` 形式のプレースホルダーの集合（名前は小文字・数字・アンダースコア）。
fn placeholders(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else {
            break;
        };
        let name = &rest[start + 1..start + len];
        if !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            found.insert(name.to_owned());
        }
        rest = &rest[start + len + 1..];
    }
    found
}

#[test]
fn every_dictionary_has_the_same_keys_and_placeholders_as_english() {
    let english = dictionary(Language::English);
    assert!(english.len() > 500, "英語の辞書が小さすぎます");
    for language in Language::ALL {
        let dict = dictionary(language);
        let missing: Vec<_> = english.keys().filter(|k| !dict.contains_key(*k)).collect();
        let extra: Vec<_> = dict.keys().filter(|k| !english.contains_key(*k)).collect();
        assert!(
            missing.is_empty(),
            "{} に無いキー: {missing:?}",
            language.tag()
        );
        assert!(
            extra.is_empty(),
            "{} だけにあるキー: {extra:?}",
            language.tag()
        );
        for (key, value) in &dict {
            assert!(
                !value.trim().is_empty(),
                "{} の {key} が空です",
                language.tag()
            );
            assert_eq!(
                placeholders(value),
                placeholders(&english[key]),
                "{} の {key} のプレースホルダーが英語と違います",
                language.tag()
            );
        }
    }
}

/// ソースを再帰で集める。
fn files_under(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, ext, out);
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

/// `marker("key"` の形でソースに書かれたキーを集める（キーは必ず文字列リテラルで書く規約）。
fn referenced_keys() -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let mut sources = Vec::new();
    files_under(&root().join("src"), "rs", &mut sources);
    files_under(&root().join("templates"), "html", &mut sources);
    for path in sources {
        // locale.rs はマクロ・関数の定義とその説明（例のキー）を含むので、参照の走査から外す。
        if path.ends_with("locale.rs") && path.parent().is_some_and(|p| p.ends_with("src")) {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        for marker in [
            "t!(",
            "locale::tr(",
            "locale::tr1(",
            "locale::tr2(",
            "tr_in(",
        ] {
            let mut rest = text.as_str();
            while let Some(at) = rest.find(marker) {
                // `format!(` の `t!(` のように、別の識別子の末尾に一致したものは参照ではない。
                let is_suffix = rest[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
                rest = &rest[at + marker.len()..];
                if is_suffix {
                    continue;
                }
                // tr_in は第 1 引数が言語なので、最初の文字列リテラルまで読み飛ばす。
                let candidate = rest.trim_start();
                let literal = if marker == "tr_in(" {
                    candidate.find('"').map(|i| &candidate[i..])
                } else {
                    Some(candidate)
                };
                let Some(literal) = literal.and_then(|l| l.strip_prefix('"')) else {
                    continue;
                };
                if let Some(end) = literal.find('"') {
                    keys.insert(literal[..end].to_string());
                }
            }
        }
    }
    keys
}

#[test]
fn referenced_keys_exist_and_dictionary_has_no_unused_keys() {
    let english = dictionary(Language::English);
    let used = referenced_keys();
    let unknown: Vec<_> = used.iter().filter(|k| !english.contains_key(*k)).collect();
    assert!(
        unknown.is_empty(),
        "辞書に無いキーを参照しています: {unknown:?}"
    );
    let unused: Vec<_> = english.keys().filter(|k| !used.contains(*k)).collect();
    assert!(unused.is_empty(), "どこからも参照されないキー: {unused:?}");
}

#[test]
fn tags_are_unique_round_trip_and_cookie_values_are_validated() {
    let tags: BTreeSet<_> = Language::ALL.iter().map(|l| l.tag()).collect();
    assert_eq!(tags.len(), 9);
    for language in Language::ALL {
        assert_eq!(locale::parse(language.tag()), Some(language));
        assert_eq!(Language::from_tag(language.tag()), Some(language));
        assert!(!language.native_name().is_empty());
    }
    // Cookie の値は完全一致のタグだけ（`ja-JP` や任意の文字列は受け付けない）。
    assert_eq!(Language::from_tag("ja-JP"), None);
    assert_eq!(Language::from_tag("../x"), None);
}

#[test]
fn language_tags_follow_the_shared_matching_rules() {
    use Language::*;
    for (tag, expected) in [
        ("ja-JP", Japanese),
        ("en-GB", English),
        ("zh-Hans", ChineseSimplified),
        ("zh-CN", ChineseSimplified),
        ("zh-SG", ChineseSimplified),
        ("zh", ChineseSimplified),
        ("zh-Hant", ChineseTraditional),
        ("zh-TW", ChineseTraditional),
        ("zh-HK", ChineseTraditional),
        ("zh-MO", ChineseTraditional),
        ("zh-Hant-CN", ChineseTraditional),
        ("ko-KR", Korean),
        ("es-MX", Spanish),
        ("fr-CA", French),
        ("de-AT", German),
        ("pt", PortugueseBrazil),
        ("pt-BR", PortugueseBrazil),
        ("pt-PT", PortugueseBrazil),
    ] {
        assert_eq!(locale::supported(tag), Some(expected), "{tag}");
    }
    assert_eq!(locale::supported("it"), None);
    assert_eq!(locale::supported("*"), None);
    // ロケール変数: C・POSIX・空は判定材料にならず、未対応の言語は英語。
    for value in ["", "C", "C.UTF-8", "POSIX"] {
        assert_eq!(locale::parse(value), None, "{value:?}");
    }
    assert_eq!(locale::parse("it_IT.UTF-8"), Some(English));
    assert_eq!(locale::parse("Japanese_Japan.932"), Some(Japanese));
    assert_eq!(
        locale::parse("Chinese (Traditional)_Taiwan.950"),
        Some(ChineseTraditional)
    );
}

#[test]
fn cli_language_order_is_override_then_lc_all_then_lc_messages_then_lang_then_os_then_english() {
    use Language::*;
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    };
    let all = &[
        ("SCRIPTURA_LANGUAGE", "de"),
        ("LC_ALL", "fr"),
        ("LC_MESSAGES", "es"),
        ("LANG", "ko_KR.UTF-8"),
    ];
    assert_eq!(locale::detect_with(env(all), || Some(Japanese)), German);
    assert_eq!(
        locale::detect_with(env(&all[1..]), || Some(Japanese)),
        French
    );
    assert_eq!(
        locale::detect_with(env(&all[2..]), || Some(Japanese)),
        Spanish
    );
    assert_eq!(
        locale::detect_with(env(&all[3..]), || Some(Japanese)),
        Korean
    );
    // C・POSIX は次の変数へ進み、何も無ければ OS の表示言語、それも無ければ英語。
    let c_then_lang = &[("LC_ALL", "C"), ("LANG", "pt_BR.UTF-8")];
    assert_eq!(
        locale::detect_with(env(c_then_lang), || None),
        PortugueseBrazil
    );
    assert_eq!(locale::detect_with(env(&[]), || Some(Japanese)), Japanese);
    assert_eq!(locale::detect_with(env(&[]), || None), English);
    // 未対応の言語の指定は、その場で英語に決まる（後ろの変数へは進まない）。
    let unsupported = &[("SCRIPTURA_LANGUAGE", "it"), ("LANG", "ja_JP.UTF-8")];
    assert_eq!(
        locale::detect_with(env(unsupported), || Some(Japanese)),
        English
    );
}

#[test]
fn windows_language_ids_map_to_the_supported_languages() {
    use Language::*;
    for (id, expected) in [
        (0x0411, Japanese),
        (0x0409, English),
        (0x0804, ChineseSimplified),
        (0x0404, ChineseTraditional),
        (0x0c04, ChineseTraditional),
        (0x0412, Korean),
        (0x0c0a, Spanish),
        (0x040c, French),
        (0x0407, German),
        (0x0416, PortugueseBrazil),
        (0x0816, PortugueseBrazil),
        (0x0410, English),
    ] {
        assert_eq!(locale::language_from_langid(id), expected, "{id:#06x}");
    }
}

#[test]
fn missing_keys_fall_back_to_english_then_to_the_key_and_placeholders_fill_once() {
    assert_eq!(
        locale::tr_in(Language::French, "no.such.key"),
        "no.such.key"
    );
    assert_eq!(
        locale::fill("a {x} b {x} {y} {z}", &[("x", "1"), ("y", "{x}")]),
        "a 1 b 1 {x} {z}",
        "値の中の {{x}} は再展開しない・値の無い {{z}} はそのまま残す"
    );
    assert_eq!(
        locale::tr_in(Language::German, "ui.base.exit"),
        dictionary(Language::German)["ui.base.exit"]
    );
}

#[test]
fn scoped_language_applies_only_inside_the_closure() {
    let before = locale::current();
    let inside = locale::scoped(Language::Korean, locale::current);
    assert_eq!(inside, Language::Korean);
    assert_eq!(locale::current(), before);
}

#[cfg(feature = "ui")]
mod ui {
    use axum::http::{HeaderMap, HeaderValue, header};
    use scriptura::locale::Language;
    use scriptura::ui::locale::{accept_language, cookie_name, detect};

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, value) in pairs {
            h.append(
                header::HeaderName::from_static(name),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        h
    }

    #[test]
    fn accept_language_picks_the_best_supported_quality() {
        let h = headers(&[("accept-language", "it;q=0.9, fr;q=0.8, de;q=0.7")]);
        assert_eq!(accept_language(&h), Some(Language::French));
        let h = headers(&[("accept-language", "en;q=0.5, zh-Hant;q=0.9, ja;q=0.1")]);
        assert_eq!(accept_language(&h), Some(Language::ChineseTraditional));
        // 同じ q なら先に書かれたもの。q=0 と不正な q は使わない。
        let h = headers(&[("accept-language", "ko, es, de;q=0")]);
        assert_eq!(accept_language(&h), Some(Language::Korean));
        let h = headers(&[("accept-language", "de;q=0, ja;q=abc")]);
        assert_eq!(accept_language(&h), None);
        let h = headers(&[("accept-language", "pt-PT, *;q=0.1")]);
        assert_eq!(accept_language(&h), Some(Language::PortugueseBrazil));
        assert_eq!(accept_language(&HeaderMap::new()), None);
    }

    #[test]
    fn explicit_cookie_beats_accept_language_and_the_default_is_english() {
        let name = cookie_name(7468);
        let cookie = format!("other=1; {name}=de");
        let h = headers(&[("cookie", &cookie), ("accept-language", "ja")]);
        let d = detect(&h, 7468);
        assert_eq!(d.language, Language::German);
        assert_eq!(d.preference, Some(Language::German));
        // 別のポートの Cookie・不正な値は無視する。
        let other = format!("{}=de", cookie_name(9999));
        let h = headers(&[("cookie", &other), ("accept-language", "ja")]);
        assert_eq!(detect(&h, 7468).language, Language::Japanese);
        let bad = format!("{name}=xx");
        let h = headers(&[("cookie", &bad)]);
        let d = detect(&h, 7468);
        assert_eq!((d.language, d.preference), (Language::English, None));
        assert_eq!(detect(&HeaderMap::new(), 7468).language, Language::English);
    }
}
