# 配信 CI（Setup 版）

- **配信 CI**（`.github/workflows/release-velopack.yml`）は windows・macos・linux（x64・arm64）の 4 job が独立。main の軽量タグ `v<x.y.z>` の push だけで起動し（`git push --tags` は使わない）、試走は `dryrun/<任意>` を dispatch する（配信は publish のときだけ。タグの形・Cargo.toml の版・main の祖先を確かめ、配信済みは飛ばす）。やり直しは `gh workflow run release-velopack.yml --ref v<x.y.z>`。Linux は glibc 2.39 以下を検査。
