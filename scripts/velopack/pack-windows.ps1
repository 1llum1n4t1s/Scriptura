# Scriptura の Windows 版 Setup（Velopack、ユーザー単位、%LOCALAPPDATA%\ScripturaApp へ導入）を梱包する。
# 実行環境: Windows x64（PowerShell 7+、Rust、Velopack CLI vpk 1.2.161）。手元または GitHub Actions（windows-latest）。
# 配信先: Cloudflare R2 バケット scriptura-updates の直下（公開 URL https://scriptura-updates.kagayoi.com）。
#
# 出力（packages\）は自動更新の更新元として R2 へ置く一式（Setup・releases.win.json・full nupkg など）。
# 配信は scripts/velopack/publish-r2.ps1 が行う。共通仕様は D:\Dev\APP-DISTRIBUTION.md。
#
# -Sign は Sigilune（Certum SimplySign のクラウド鍵）で署名する。手元は `sigilune setup` で登録済みであること。
# CI は .github/workflows/release-velopack.yml が Environment の Secrets を SIGILUNE_EMAIL / SIGILUNE_TOTP_URI で渡す。
# アプリは適用前に、パッケージ内の scriptura.exe の署名者 CN を src/app_update/trust.rs の値と照合する。
#
# 検証用（本番では使わない）: -PackId / -PackVersion / -Binary を指定すると、別の packId・版・ビルド済みの
# 実行ファイルで未署名の一式を作れる。-Sign と併用はできない（テスト用のビルドに本番の署名をしないため）。
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $WorkDirectory,
    [switch] $Sign,
    # 省略時は作業ディレクトリ内の target。CI は署名用 Secrets を渡さない前段の step で同じ条件の cargo build を済ませ、
    # その出力先を渡す（このスクリプトの cargo build は再コンパイルしない）。
    [string] $CargoTargetDirectory,
    [string] $PackId = 'ScripturaApp',
    # 省略時は リポジトリ直下の Cargo.toml の版。
    [string] $PackVersion,
    # ビルド済みの scriptura.exe（検証用）。指定すると cargo build を行わない。
    [string] $Binary
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
if (!$IsWindows -or [Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') {
    throw 'Pack the installer on Windows x64.'
}
if ($Sign -and ($PackId -ne 'ScripturaApp' -or $PackVersion -or $Binary)) {
    throw '-Sign cannot be combined with -PackId, -PackVersion or -Binary (those are for isolated validation packages).'
}
if ($PackId -notmatch '^[A-Za-z][A-Za-z0-9]*$') { throw 'PackId must be alphanumeric.' }
if (![IO.Path]::IsPathFullyQualified($WorkDirectory)) {
    throw 'Specify an absolute path to an empty work directory.'
}
$work = Get-Item -LiteralPath $WorkDirectory -Force
if (!$work.PSIsContainer -or
    (Get-ChildItem -LiteralPath $work.FullName -Force | Select-Object -First 1)) {
    throw 'The work directory must be an empty, ordinary directory.'
}
for ($folder = $work; $null -ne $folder; $folder = $folder.Parent) {
    if ($folder.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "The work path contains a reparse point: $($folder.FullName)"
    }
}
# GitHub Actions では vpk が色付け用のエスケープシーケンスを出し、`\b` の照合が外れるため取り除いてから比べる。
$vpkHelp = (vpk --help | Out-String) -replace "`e\[[0-9;]*m", ''
if ($vpkHelp -notmatch '\bVelopack CLI 1\.2\.161\b') {
    throw 'Velopack CLI 1.2.161 is required.'
}

$signing = @()
# 本人の他アプリと共通のコード署名証明書（Certum）。thumbprint は年次更新で変わるため、署名者は CN で照合する。
$signerCommonName = 'Open Source Developer Yuichiro Shinozaki'
if ($Sign) {
    . (Join-Path $PSScriptRoot 'Sigilune.Common.ps1')
    $sigilune = Resolve-SigiluneExe
    # ビルド前にログインし、認証の失敗を早く検出する（有効なセッションは再利用する）。
    Connect-Sigilune -SigiluneExe $sigilune
    # Velopack はファイルごとに署名テンプレートを実行する。1 ファイルずつ直列に署名し、
    # Sigilune が署名後に有効性・署名者・タイムスタンプを検証する。--replace は既存の署名を置き換える。
    $signing = @(
        '--signParallel', '1',
        '--signTemplate', "`"$sigilune`" sign --replace {{file}}"
    )
}

$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$build = if ($CargoTargetDirectory) {
    if (![IO.Path]::IsPathFullyQualified($CargoTargetDirectory)) { throw 'Specify an absolute path for the Cargo target directory.' }
    $CargoTargetDirectory
} else {
    Join-Path $work.FullName 'target'
}
$payload = Join-Path $work.FullName 'payload'
$packages = Join-Path $work.FullName 'packages'
$temp = Join-Path $work.FullName 'temp'
New-Item -ItemType Directory -Path $payload, $packages, $temp | Out-Null

$previousTemp = $env:TEMP
$previousTmp = $env:TMP
Push-Location -LiteralPath $repo
try {
    # Rust と Velopack の一時ファイルを、梱包専用のディレクトリに閉じる。
    $env:TEMP = $temp
    $env:TMP = $temp
    if (!$PackVersion) {
        $metadata = cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json
        $PackVersion = ($metadata.packages | Where-Object name -eq 'scriptura').version
        if (!$PackVersion) { throw 'Could not read the scriptura version from Cargo.toml.' }
    }
    if ($Binary) {
        if (![IO.Path]::IsPathFullyQualified($Binary) -or !(Test-Path -LiteralPath $Binary -PathType Leaf)) {
            throw 'Binary must be an absolute path to an existing scriptura.exe.'
        }
        Copy-Item -LiteralPath $Binary -Destination (Join-Path $payload 'scriptura.exe')
    } else {
        # 本番の梱包は既定の機能だけでビルドする（update-feed-test などのテスト用の機能を含めない）。
        cargo build --locked --release --target x86_64-pc-windows-msvc --target-dir $build --bin scriptura
        Copy-Item -LiteralPath (Join-Path $build 'x86_64-pc-windows-msvc/release/scriptura.exe') -Destination $payload
    }
    # 同梱する文書。Scriptura には LICENSE・CHANGELOG が無いため、存在するものだけを入れる（README.md は必須）。
    Copy-Item -LiteralPath (Join-Path $repo 'README.md') -Destination $payload
    foreach ($optional in 'LICENSE', 'CHANGELOG.md') {
        $candidate = Join-Path $repo $optional
        if (Test-Path -LiteralPath $candidate -PathType Leaf) { Copy-Item -LiteralPath $candidate -Destination $payload }
    }
    # packId は作業フォルダー %LOCALAPPDATA%\remnifold と衝突しない名前にする（Velopack の導入先は
    # %LOCALAPPDATA%\<packId>）。従来の install の導入先（Programs\Scriptura）とも別。
    # 本番はスタートメニュー直下（StartMenuRoot）。テスト用の packId の梱包は None にして、
    # 検証でインストールしても利用者の実プロファイルへ .lnk を作らない（vpk 1.2.161 は `--shortcuts None` を受け付ける）。
    $shortcuts = if ($PackId -eq 'ScripturaApp') { 'StartMenuRoot' } else { 'None' }
    $packOptions = @(
        '--packId', $PackId,
        '--packTitle', 'Scriptura',
        '--packAuthors', 'Kagayoi',
        '--packVersion', $PackVersion,
        '--channel', 'win',
        '--runtime', 'win-x64',
        '--instLocation', 'PerUser',
        '--mainExe', 'scriptura.exe',
        '--packDir', $payload,
        '--outputDir', $packages,
        '--shortcuts', $shortcuts,
        '--noPortable',
        '--delta', 'None'
    )
    vpk pack @packOptions @signing
    $setup = Join-Path $packages "$PackId-win-Setup.exe"
    if (!(Test-Path -LiteralPath $setup -PathType Leaf)) { throw 'Setup executable was not generated.' }
    # 自動更新の更新元（R2）には、Setup のほか releases.win.json と full nupkg が必要。
    # Setup だけでは、アプリ側から新版を見つけられない。既定のチャンネル（win）の full nupkg は `<id>-<版>-full.nupkg`。
    foreach ($required in @('releases.win.json', "$PackId-$PackVersion-full.nupkg")) {
        if (!(Test-Path -LiteralPath (Join-Path $packages $required) -PathType Leaf)) { throw "Update feed file was not generated: $required" }
    }
    # 版付きの不変名の Setup も置く（固定名 Setup は最新版を指す URL として毎回置き換える）。
    Copy-Item -LiteralPath $setup -Destination (Join-Path $packages "$PackId-$PackVersion-win-Setup.exe")
    if ($Sign) {
        $signature = Get-AuthenticodeSignature -LiteralPath $setup
        if ($signature.Status -ne 'Valid' -or
            $signature.SignerCertificate.Subject -notlike "CN=$signerCommonName*" -or
            !$signature.TimeStamperCertificate) {
            throw 'Setup failed Authenticode verification (signer and timestamp).'
        }
        # アプリ（src/app_update/trust.rs）は適用前に、nupkg 内の全ての PE（.exe・.dll）の署名者を同じ CN で
        # 照合し、主実行ファイルと更新器（Squirrel.exe）がちょうど1つずつあることも要求する。
        # 配信前に同じ検査を通し、ここで通らない一式は配信しない（配信すると、全利用者の更新が適用前に拒否される）。
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $nupkg = Join-Path $packages "$PackId-$PackVersion-full.nupkg"
        $archive = [IO.Compression.ZipFile]::OpenRead($nupkg)
        try {
            $names = @($archive.Entries | ForEach-Object { $_.FullName })
            if (@($names | Where-Object { $_ -ieq 'lib/app/scriptura.exe' }).Count -ne 1) { throw 'The full nupkg must contain exactly one lib/app/scriptura.exe.' }
            if (@($names | Where-Object { $_ -ieq 'lib/app/Squirrel.exe' }).Count -ne 1) { throw 'The full nupkg must contain exactly one lib/app/Squirrel.exe.' }
            $verified = 0
            foreach ($entry in @($archive.Entries | Where-Object { $_.FullName -match '(?i)\.(exe|dll)$' })) {
                $extracted = Join-Path $temp ("verify-{0}.bin" -f [Guid]::NewGuid().ToString('N'))
                [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $extracted)
                $inner = Get-AuthenticodeSignature -LiteralPath $extracted
                Remove-Item -LiteralPath $extracted -Force
                if ($inner.Status -ne 'Valid' -or $inner.SignerCertificate.Subject -notlike "CN=$signerCommonName*") {
                    throw "$($entry.FullName) inside the full nupkg failed Authenticode verification ($($inner.Status))."
                }
                $verified++
            }
            Write-Host "Verified the signer of $verified executable(s) inside the full nupkg."
        } finally {
            $archive.Dispose()
        }
    }
    Get-FileHash -LiteralPath $setup -Algorithm SHA256
} finally {
    $env:TEMP = $previousTemp
    $env:TMP = $previousTmp
    Pop-Location
}
