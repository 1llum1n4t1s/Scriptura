// 危険な変更の二段階確認（設計書 9.2-8）と、表示用の行差分。
//
// 判定は設定の差分から行う純粋関数 `dangers(before, after)`。フォームごとのフラグにしないので、
// どの画面から同じ変更をしても（履歴の復元を含む）同じ規則で確認を挟む。
// 1 回目の POST では書き込まず、変更点の一覧と 1 回限りの nonce を返す。nonce は
// 「送信先・版・フォーム内容（csrf と confirm を除く）」の SHA-256 に結び付け、10 分で失効する。
// 2 回目の POST は同じ内容・同じ版でなければ通らない（内容の改変・nonce の使い回しを拒否）。
use std::time::Instant;

use sha2::{Digest, Sha256};

use super::{AppState, NONCE_TTL, guard};
use crate::{
    config::{Config, Mode, Permission, Policy, Repository, normalize_root},
    kinds,
};

/// 発行済みの nonce が結び付いている内容と期限。
pub(crate) struct Pending {
    digest: String,
    expires: Instant,
}

/// 設定の変更 `before → after` に含まれる危険な変更の説明（利用者向けの文）。空なら確認は不要。
///
/// 対象: push モード化（登録・既定）、propose での登録の追加、権限 bypass 化、自動登録の方針を all / remote-owner にする変更と
/// remote-owner の owner 追加、費用上限（max_budget_usd）を 0（上限なし）にする変更、カスタム種類の
/// 対象パスの追加、同意の記録、登録の解除。
pub fn dangers(before: &Config, after: &Config) -> Vec<String> {
    let windows = cfg!(windows);
    let effective = |c: &Config, r: &Repository| r.mode.unwrap_or(c.defaults.mode);
    let find = |c: &Config, root: &str| {
        let wanted = normalize_root(root, windows);
        c.repository
            .iter()
            .find(|r| normalize_root(&r.root, windows) == wanted)
            .cloned()
    };
    let mut out = Vec::new();
    for repo in &after.repository {
        let was_push =
            find(before, &repo.root).is_some_and(|r| effective(before, &r) == Mode::Push);
        if effective(after, repo) == Mode::Push && !was_push {
            out.push(format!(
                "{} を push モードにします（文書の変更を自動で commit し、remote へ push します）",
                repo.root
            ));
        } else if effective(after, repo) == Mode::Propose && find(before, &repo.root).is_none() {
            // 登録が増える（履歴の復元で解除済みの登録が戻るなど）。propose でも push のたびに worker が
            // 動いて利用料金が発生するので確認を挟む。有効化の画面は事前の写しに変更を当てないので
            // ここには来ない（有効化は同意と push の確認を別に持つ）。
            out.push(format!(
                "{} を登録します（propose モード。push のたびに worker が動き、更新案を作ります）",
                repo.root
            ));
        }
    }
    if after.defaults.mode == Mode::Push && before.defaults.mode != Mode::Push {
        out.push("既定のモードを push にします（個別にモードを指定していない登録がすべて push になります）".into());
    }
    if after.worker.permission == Permission::Bypass
        && before.worker.permission != Permission::Bypass
    {
        out.push(
            "worker の権限を bypass にします（worker がツールの許可確認なしで動きます）".into(),
        );
    }
    if after.enrollment.policy != before.enrollment.policy {
        match after.enrollment.policy {
            Policy::All => out.push(
                "自動登録の方針を all にします（登録していないリポジトリも保守の対象になります）"
                    .into(),
            ),
            Policy::RemoteOwner => out.push(
                "自動登録の方針を remote-owner にします（指定した owner のリポジトリが登録なしで対象になります）"
                    .into(),
            ),
            Policy::Explicit => {}
        }
    } else if after.enrollment.policy == Policy::RemoteOwner {
        let added: Vec<&String> = after
            .enrollment
            .owners
            .iter()
            .filter(|o| !before.enrollment.owners.contains(o))
            .collect();
        for owner in added {
            out.push(format!(
                "自動登録の owner に {owner} を追加します（その owner のリポジトリが登録なしで対象になります）"
            ));
        }
    }
    if after.worker.max_budget_usd == 0.0 && before.worker.max_budget_usd > 0.0 {
        out.push("worker の費用上限（max_budget_usd）を外します（0 = 上限なし）".into());
    }
    for (id, settings) in &after.kinds {
        if kinds::is_builtin_id(id) {
            continue;
        }
        let old: &[String] = before
            .kinds
            .get(id)
            .map(|k| k.paths.as_slice())
            .unwrap_or(&[]);
        for path in settings.paths.iter().filter(|p| !old.contains(p)) {
            out.push(format!(
                "カスタム種類 {id} に対象パス {path} を追加します（worker がこのパスを書き換えられるようになります）"
            ));
        }
    }
    if after.consent.is_some() && after.consent != before.consent {
        out.push("同意を記録します（worker が読んだリポジトリの内容があなたの Claude 認証で Anthropic へ送信され、実行ごとに利用料金が発生します）".into());
    }
    for repo in &before.repository {
        if find(after, &repo.root).is_none() {
            out.push(format!(
                "{} の登録を解除します（このリポジトリの文書保守を止めます）",
                repo.root
            ));
        }
    }
    out
}

/// 送信先とフォーム内容（csrf・confirm を除く）の SHA-256。名前と値の組を並べ替えてから求める。
pub(crate) fn form_digest(action: &str, pairs: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = pairs.iter().collect();
    sorted.sort();
    let mut h = Sha256::new();
    h.update(action.as_bytes());
    h.update([0]);
    for (k, v) in sorted {
        h.update(k.as_bytes());
        h.update([0]);
        h.update(v.as_bytes());
        h.update([0]);
    }
    format!("{:x}", h.finalize())
}

impl AppState {
    /// nonce を発行する（期限切れの nonce はついでに捨てる）。
    pub(crate) fn issue_nonce(&self, digest: String) -> String {
        let nonce = guard::random_token();
        if let Ok(mut map) = self.nonces.lock() {
            let now = Instant::now();
            map.retain(|_, p| p.expires > now);
            map.insert(
                nonce.clone(),
                Pending {
                    digest,
                    expires: now + NONCE_TTL,
                },
            );
        }
        nonce
    }

    /// nonce を使う。1 回限り（一致しなくても消す）。期限内で内容が一致したときだけ true。
    pub(crate) fn consume_nonce(&self, nonce: &str, digest: &str) -> bool {
        let Ok(mut map) = self.nonces.lock() else {
            return false;
        };
        match map.remove(nonce) {
            Some(p) => p.expires > Instant::now() && guard::same(&p.digest, digest),
            None => false,
        }
    }
}

/// 行単位の差分（LCS）。`'-'` は before だけ、`'+'` は after だけ、`' '` は共通の行。
/// 大きすぎる入力（行数の積が 400 万を超える）は、全削除＋全追加として返す（表示用なので正確さより上限を優先）。
pub fn line_diff(before: &str, after: &str) -> Vec<(char, String)> {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    if a.len().saturating_mul(b.len()) > 4_000_000 {
        return a
            .iter()
            .map(|l| ('-', l.to_string()))
            .chain(b.iter().map(|l| ('+', l.to_string())))
            .collect();
    }
    // dp[i][j] = a[i..] と b[j..] の LCS の長さ。
    let (n, m) = (a.len(), b.len());
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((' ', a[i].to_string()));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push(('-', a[i].to_string()));
            i += 1;
        } else {
            out.push(('+', b[j].to_string()));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| ('-', l.to_string())));
    out.extend(b[j..].iter().map(|l| ('+', l.to_string())));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_marks_changed_lines() {
        let d = line_diff("a\nb\nc", "a\nx\nc\nd");
        assert_eq!(
            d,
            vec![
                (' ', "a".into()),
                ('-', "b".into()),
                ('+', "x".into()),
                (' ', "c".into()),
                ('+', "d".into())
            ]
        );
    }

    #[test]
    fn digest_ignores_order_but_not_content() {
        let a = vec![("x".to_string(), "1".to_string()), ("y".into(), "2".into())];
        let b = vec![("y".to_string(), "2".to_string()), ("x".into(), "1".into())];
        let c = vec![("y".to_string(), "3".to_string()), ("x".into(), "1".into())];
        assert_eq!(form_digest("/a", &a), form_digest("/a", &b));
        assert_ne!(form_digest("/a", &a), form_digest("/a", &c));
        assert_ne!(form_digest("/a", &a), form_digest("/b", &a));
    }
}
