// 更新のインストールの後に、新しい版の常駐へ画面を自動で入り直させる「引き継ぎ鍵」の保存と読み出し。
//
// 流れ: 旧版が「更新をインストール」の適用を依頼する前に、乱数の鍵を作って SHA-256 と期限・待ち受けポートだけを
// 状態フォルダーの 1 ファイルへ書く（鍵そのものは JSON 応答にだけ載せ、保存・ログ・標準出力へ出さない）。
// 新版の常駐は起動時にこのファイルを読み、読んだ時点で必ず削除する（不正・期限切れでも）。期限内でポートが一致する
// ときだけ、ハッシュを `AppState` の引き継ぎ枠に持つ。ファイルを置けるのは状態フォルダーへ書ける同じ利用者だけで、
// 置かれた値はハッシュなので、読めても鍵は復元できない。
use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 状態フォルダーに置くファイル名。
pub(crate) const FILE: &str = "ui-handoff.json";
/// 引き継ぎ鍵の有効期間。
pub const TTL: Duration = Duration::from_secs(10 * 60);
/// 読み込む上限（これを超えるファイルは壊れているものとして捨てる）。
const MAX_BYTES: usize = 4096;

/// ファイルの中身。鍵そのものは含めない。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    /// 鍵の SHA-256（16 進小文字 64 桁）。
    sha256: String,
    /// 新版が待ち受けるポート（一致しなければ使わない）。
    port: u16,
    /// 発行の時刻（UNIX 秒）。
    issued_at: u64,
    /// 失効の時刻（UNIX 秒）。
    expires_at: u64,
}

/// 鍵の SHA-256（16 進小文字）。
pub(crate) fn digest(key: &str) -> String {
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    crate::maintenance::hex_lower(&h.finalize())
}

/// 今の UNIX 秒（時計が取れなければ 0）。
pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 引き継ぎのファイルを書く。状態フォルダーが無ければ作らずに失敗する（読み取りや非対応のビルドで状態フォルダーを
/// 作らないため）。
pub(crate) fn write(state_dir: &Path, key: &str, port: u16, now: u64) -> std::io::Result<()> {
    if !state_dir.is_dir() {
        return Err(std::io::ErrorKind::NotFound.into());
    }
    let record = Record {
        sha256: digest(key),
        port,
        issued_at: now,
        expires_at: now + TTL.as_secs(),
    };
    let bytes = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
    crate::config::write_atomic(&state_dir.join(FILE), &bytes)
}

/// 引き継ぎのファイルを消す（無ければ何もしない）。
pub(crate) fn remove(state_dir: &Path) {
    let _ = fs::remove_file(state_dir.join(FILE));
}

/// 自分が書いた引き継ぎのファイルだけを消す（中身のハッシュが `key` のものと一致するときだけ。後から別の要求が
/// 書いたものや、読めないものは消さない）。
pub(crate) fn remove_owned(state_dir: &Path, key: &str) {
    let path = state_dir.join(FILE);
    let Ok(bytes) = fs::read(&path) else { return };
    let owned = bytes.len() <= MAX_BYTES
        && serde_json::from_slice::<Record>(&bytes)
            .is_ok_and(|record| record.sha256 == digest(key));
    if owned {
        let _ = fs::remove_file(&path);
    }
}

/// 起動時に読む。読んだ時点（読めなくても）で必ず消す。有効なら、鍵のハッシュと残りの有効期間を返す。
/// 無効の条件: 壊れている・大きすぎる・ポート違い・期限切れ・発行が未来（時計の逆行）・期間が上限を超える。
pub(crate) fn take(state_dir: &Path, port: u16, now: u64) -> Option<(String, Duration)> {
    let read = fs::read(state_dir.join(FILE));
    remove(state_dir);
    let record: Record =
        serde_json::from_slice(&read.ok().filter(|b| b.len() <= MAX_BYTES)?).ok()?;
    let well_formed = record.sha256.len() == 64
        && record
            .sha256
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    let in_window = record.issued_at <= now
        && now < record.expires_at
        && record.expires_at.saturating_sub(record.issued_at) <= TTL.as_secs();
    (well_formed && in_window && record.port == port)
        .then(|| (record.sha256, Duration::from_secs(record.expires_at - now)))
}
