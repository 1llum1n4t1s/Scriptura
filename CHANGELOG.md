# Changelog

## [Unreleased]

## [0.1.1] - 2026-11-15

### Added

- Windows 向けの Setup（Velopack）を追加した。ユーザー単位で導入でき、管理者権限は要らない。導入版は Claude Code の Stop hook へ自動で登録され、既定で自動更新する。更新はログオン時に、署名者を検証してから適用する。
- 表示言語を 9 言語（ja・en・zh-CN・zh-TW・ko・es・fr・de・pt-BR）に対応した。CLI・hook・Setup 版の案内は `SCRIPTURA_LANGUAGE` / `LC_ALL` / `LANG` / OS の表示言語、管理画面は言語の選択欄（Cookie）と `Accept-Language` で決まる。

### Changed

- README は英語の `README.md` を正本にし、日本語版を `README.ja.md` へ移して 8 言語版を追加した。worker へのプロンプトとプラグインのスキルは英語に統一した（文書の `language` 設定の挙動は変わらない）。

### Fixed

- プラグインを有効にしているとき、settings.json に Scriptura 自身の Stop hook が残っていると保守が二重に動く問題を直した。Setup 版が、残っていた Scriptura 自身の hook だけを外す（`agents-maintenance` など他の hook は残す）。
