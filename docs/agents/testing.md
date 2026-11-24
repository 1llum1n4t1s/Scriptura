# Setup 版の検証の隔離

- 検証で本物の導入・`~/.claude/settings.json`・HKCU Run に触れない。テスト用 packId・外部の `--installto`・`SCRIPTURA_HOME`・`CLAUDE_CONFIG_DIR`・`TEMP`/`TMP` で隔離し、終了後に痕跡を外す。レジストリの単体テストは専用ハイブを使う。macOS・Linux の cfg コードは Windows で未コンパイル（CI・実機で確認）。
