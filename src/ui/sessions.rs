// 常駐（`--tray`）の管理画面のログインを、プロセスの再起動・PC の再起動をまたいで保つための保存と照合。
//
// 保存するのはセッショントークンの SHA-256（16 進小文字 64 桁）の一覧だけ（最大 16 件）。状態フォルダーの
// `ui-sessions-<port>.json` に置く。生のトークンは保存・ログ・標準出力に出さない（読めても復元できない）。
// 壊れている・大きすぎる・ポート違い・ハッシュの形が違うファイルは「保存なし」として扱い（失敗側に倒す）、
// 起動時には書き換えない。次のログインで作り直す。書き込みは `config::write_atomic`。状態フォルダーが無ければ作らない。
// ファイルは終了しても消さない。利用者がログインをすべて無効にするには、終了してからこのファイルを削除する。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{guard, handoff};

/// 保存する件数の上限（超えたら最も古いものを捨てる）。
pub(crate) const MAX_SESSIONS: usize = 16;
/// 読み込む上限（これを超えるファイルは壊れているものとして捨てる）。
const MAX_BYTES: usize = 4096;
/// Cookie の有効期間（400 日。ブラウザーが受け付ける上限）。
pub(crate) const COOKIE_MAX_AGE: u64 = 34_560_000;

/// ファイルの中身。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    /// 待ち受けるポート（一致しなければ使わない）。
    port: u16,
    /// セッショントークンの SHA-256（古い順）。
    sessions: Vec<String>,
}

/// 状態フォルダーに置くファイル名。
pub(crate) fn file_name(port: u16) -> String {
    format!("ui-sessions-{port}.json")
}

/// 常駐の Cookie の名前（通常起動の Cookie と名前を分け、同じポートの通常起動が上書きしないようにする）。
pub fn resident_cookie_name(port: u16) -> String {
    format!("scriptura_resident_{port}")
}

/// 常駐の Set-Cookie の値。
pub(crate) fn cookie_header(port: u16, token: &str) -> String {
    format!(
        "{}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={COOKIE_MAX_AGE}",
        resident_cookie_name(port)
    )
}

/// 提示されたセッショントークンから導く CSRF トークン（16 進）。再起動の前に描画したページの POST が、
/// 再起動の後も通るように、保存せず毎回導く。
pub(crate) fn csrf_for(token: &str) -> String {
    handoff::digest(&format!("scriptura-csrf\0{token}"))
}

fn well_formed(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// 有効なセッションのハッシュの一覧（メモリー上の写し）。
pub(crate) struct Sessions {
    path: PathBuf,
    port: u16,
    hashes: Vec<String>,
    /// ファイルへ書いてよいか。無い（NotFound）・壊れた中身なら true（次のログインで作り直す）。それ以外の
    /// 読み取りエラー（権限・ロックなど、中身が分からない）では false で、その起動はメモリーだけで続ける。
    writable: bool,
}

impl Sessions {
    /// ファイルを読む。無い・壊れている場合は空。読めない（NotFound 以外のエラー）ときは空で、書き込みも無効にする。
    /// どちらも起動時にファイルへは書かない。
    pub(crate) fn load(state_dir: &Path, port: u16) -> Self {
        let path = state_dir.join(file_name(port));
        let (hashes, writable) = match fs::read(&path) {
            Ok(bytes) => (Self::parse(&bytes, port).unwrap_or_default(), true),
            Err(e) => (Vec::new(), e.kind() == std::io::ErrorKind::NotFound),
        };
        Self {
            path,
            port,
            hashes,
            writable,
        }
    }

    fn parse(bytes: &[u8], port: u16) -> Option<Vec<String>> {
        let bytes = (bytes.len() <= MAX_BYTES).then_some(bytes)?;
        let record: Record = serde_json::from_slice(bytes).ok()?;
        (record.port == port
            && record.sessions.len() <= MAX_SESSIONS
            && record.sessions.iter().all(|h| well_formed(h)))
        .then_some(record.sessions)
    }

    /// 提示されたトークンが有効か。すべての項目と比べる（一致した位置で止めない）。
    pub(crate) fn contains(&self, token: &str) -> bool {
        let digest = handoff::digest(token);
        self.hashes
            .iter()
            .fold(false, |found, h| guard::same(h, &digest) | found)
    }

    /// 新しいトークンを発行して一覧へ足す（上限を超えたら最も古いものを捨てる）。ファイルへ書けなくても
    /// メモリーには足す（書けなかったログインは、次の再起動までしか保たれない）。
    pub(crate) fn issue(&mut self) -> String {
        let token = guard::random_token();
        self.hashes.push(handoff::digest(&token));
        let extra = self.hashes.len().saturating_sub(MAX_SESSIONS);
        self.hashes.drain(..extra);
        let _ = self.save();
        token
    }

    /// 状態フォルダーが無ければ作らずに失敗する。
    fn save(&self) -> std::io::Result<()> {
        if !self.writable || !self.path.parent().is_some_and(Path::is_dir) {
            return Err(std::io::ErrorKind::NotFound.into());
        }
        let record = Record {
            port: self.port,
            sessions: self.hashes.clone(),
        };
        let bytes = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
        crate::config::write_atomic(&self.path, &bytes)
    }
}
