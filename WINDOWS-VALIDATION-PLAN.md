# Windows validation plan

Use this runbook from an x86-64 Windows 10 or Windows 11 machine. Run the
commands in PowerShell from the repository root.

## Prerequisites

- Rustup
- Visual Studio Build Tools with the **Desktop development with C++** workload
- A Windows SDK
- A browser and, for the online smoke test, a non-sensitive EVE test character

The application is unsigned, so Windows may display a SmartScreen warning.

## 1. Verify and build the checkout

```powershell
rustup toolchain install 1.97.1 --profile minimal --component clippy,rustfmt

cargo +1.97.1 fmt --check
cargo +1.97.1 check --locked
cargo +1.97.1 clippy --locked --all-targets --all-features -- -D warnings
cargo +1.97.1 test --locked --all-features
cargo +1.97.1 test --locked --all-features windows_credential_manager_round_trip -- --ignored
cargo +1.97.1 build --locked --release
```

All commands must succeed without warnings. The ignored Credential Manager
test creates, reads, and removes a temporary credential. Confirm that
`target\release\ekmp.exe` exists and starts.

## 2. Build and inspect the Windows package

```powershell
$Metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
$Version = ($Metadata.packages | Where-Object name -eq "ekmp").version
$ArtifactDir = Join-Path $env:TEMP "ekmp-windows-artifacts-$PID"
$ExtractDir = Join-Path $env:TEMP "ekmp-windows-extracted-$PID"

.\scripts\package-windows.ps1 -Version $Version -OutputDirectory $ArtifactDir

$Zip = Get-ChildItem $ArtifactDir -Filter "*.zip" | Select-Object -First 1
Get-FileHash $Zip.FullName -Algorithm SHA256
Expand-Archive $Zip.FullName -DestinationPath $ExtractDir

$PackagedExe = Get-ChildItem $ExtractDir -Filter "ekmp.exe" -Recurse |
    Select-Object -ExpandProperty FullName -First 1
$PackageDirectory = Split-Path $PackagedExe

if (-not (Test-Path $PackagedExe)) {
    throw "Packaged executable is missing"
}
if (-not (Test-Path (Join-Path $PackageDirectory "README.md"))) {
    throw "Packaged README is missing"
}
if (-not (Test-Path (Join-Path $PackageDirectory "LICENSE"))) {
    throw "Packaged license is missing"
}

Get-AuthenticodeSignature $PackagedExe
```

Confirm that:

- The ZIP is named `ekmp-vVERSION-x86_64-pc-windows-msvc.zip`.
- It contains `ekmp.exe`, `README.md`, and `LICENSE`.
- Authenticode reports `NotSigned` unless signing has intentionally been added.
- Explorer shows the executable icon.
- Running the executable shows the window and taskbar icons.

For a downloaded release ZIP, compare its `Get-FileHash` result with its entry
in the release's `SHA256SUMS` before running it.

## 3. Test persistence in isolated application data

This redirects only the test process away from your normal `%APPDATA%` data.
Keep this PowerShell window open until cleanup so the original value can be
restored.

```powershell
$OriginalAppData = $env:APPDATA
$SmokeAppData = Join-Path $env:TEMP "ekmp-windows-appdata-$PID"
New-Item -ItemType Directory -Path $SmokeAppData | Out-Null
$env:APPDATA = $SmokeAppData

& $PackagedExe
```

In the first launch:

1. Enable **Show protected killmails**.
2. Close the application.

Validate the saved configuration:

```powershell
$Config = Join-Path $SmokeAppData "ekmp\ekmp.json"
if (-not (Test-Path $Config)) {
    throw "Configuration was not created"
}

$State = Get-Content $Config -Raw | ConvertFrom-Json
if (-not $State.show_protected_killmails) {
    throw "Preference was not persisted"
}
if (Test-Path "$Config.tmp") {
    throw "Temporary file was not cleaned up"
}
```

### Malformed-state protection

```powershell
$ValidConfig = [System.IO.File]::ReadAllBytes($Config)
Set-Content -LiteralPath $Config -Value "{broken" -NoNewline
$BrokenHash = (Get-FileHash $Config -Algorithm SHA256).Hash

& $PackagedExe
```

Confirm that the application displays a red load error and disables
authentication and other persisted-state controls. Close it, then confirm it
did not overwrite the malformed file:

```powershell
$AfterHash = (Get-FileHash $Config -Algorithm SHA256).Hash
if ($AfterHash -ne $BrokenHash) {
    throw "Malformed configuration was overwritten"
}

[System.IO.File]::WriteAllBytes($Config, $ValidConfig)
```

### Interrupted-replacement recovery

Simulate the state left if Windows was interrupted while replacing the main
file:

```powershell
Move-Item -LiteralPath $Config -Destination "$Config.bak"
& $PackagedExe
```

Confirm that **Show protected killmails** is still enabled, proving the backup
was loaded. Disable the preference and close the application. Reopen it,
enable the preference again, and close it:

```powershell
& $PackagedExe
```

Validate the recovered state:

```powershell
Get-Content $Config -Raw | ConvertFrom-Json | Out-Null

if (Test-Path "$Config.tmp") {
    throw "Temporary file remains after recovery"
}
if (Test-Path "$Config.bak") {
    throw "Recovery backup remains after a subsequent normal replacement"
}
```

## 4. Online application smoke test

Continue using the isolated `%APPDATA%` directory and a test EVE character.

- Complete browser SSO and confirm no client secret is requested.
- Confirm the character and its corporation are automatically protected.
- Close and reopen the application; confirm authentication metadata and the
  preference persist.
- Load recent killmails, restart, and confirm the cached snapshot appears.
- Add and remove manually protected victim character and corporation IDs.
- Confirm protected victims are hidden by default and revealed by the
  persisted checkbox.
- Confirm reported killmails disappear from the recent list while successful
  explicit submissions remain in the session-only results panel.
- Confirm unknown or expired statuses do not expose an individual posting
  button.
- Confirm **Post anyway** is available only for a protected victim still
  confirmed as unreported.
- Open bulk confirmation, protect one pending victim before confirming, and
  confirm that killmail is revalidated out of the submission.
- Confirm no posting occurs without an individual or confirmed bulk action.
- Test a zKillboard hyperlink in the default browser.
- Remove the authenticated character and confirm its credential and unshared
  cached killmails are removed.

Only perform live posting with a killmail and test character explicitly
approved for publication. If suitable data is unavailable, stop before the
confirmation and record live posting as not tested.

## 5. Cleanup

Remove every authenticated test character through the application first so
its Credential Manager entry is deleted. Restore the original environment and
remove only the temporary directories created above:

```powershell
$env:APPDATA = $OriginalAppData
Remove-Item -LiteralPath $SmokeAppData -Recurse -Force
Remove-Item -LiteralPath $ArtifactDir -Recurse -Force
Remove-Item -LiteralPath $ExtractDir -Recurse -Force
```

The validation passes when compilation, Clippy, tests, Credential Manager,
packaging, icons, normal persistence, malformed-file protection, backup
recovery, SSO, caching, protected-victim policy, and character removal all
behave as described above.
