# Windows Terminal

CFVirtual is a native Windows console application. Windows Terminal is supported
through the same `crossterm` keyboard and resize handling used on other platforms;
no WSL, MSYS2, or Unix shell is required to run the Windows binary.

## Install from source

Install Rust with `rustup` and the Visual Studio C++ Build Tools with the Desktop
C++ workload. The C++ tools are needed because SQLite is bundled and compiled
locally. Then run these commands in PowerShell from the checkout:

```powershell
cd path\to\cfvirtual
cargo install --locked --path .
cfvirtual
```

The pinned `rust-toolchain.toml` selects the supported stable toolchain. Rustup
normally adds `%USERPROFILE%\.cargo\bin` to the user `PATH`. If the command is
not found in an already-open Windows Terminal window, restart that window or run:

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
```

Bare `cfvirtual` starts the offline demo. Use `cfvirtual --help` to see the
complete command list.

## Prebuilt binary

Release archives use the `x86_64-pc-windows-msvc` target and contain
`cfvirtual.exe`. Extract the archive, then either put its directory on `PATH` or
invoke it with a full path. For example, in PowerShell:

```powershell
$env:Path = "C:\Tools\cfvirtual;$env:Path"
cfvirtual
```

Verify a downloaded archive with its SHA-256 sidecar before extracting:

```powershell
Get-FileHash .\cfvirtual-<tag>-x86_64-pc-windows-msvc.zip -Algorithm SHA256
```

Compare the displayed hash with the value in the matching `.sha256` file.

## Data locations

Windows uses the standard local application directories:

- Configuration: `%APPDATA%\cfvirtual\config.toml`
- Data/database/cache: `%LOCALAPPDATA%\cfvirtual\`

The application never stores Codeforces passwords, API keys, cookies, or room
session tokens in these locations.

## Rooms and links

The same room commands work in PowerShell. Quote URLs and passwords when needed:

```powershell
cfvirtual room create --handle your_handle --listen 0.0.0.0 --port 7331
cfvirtual join --server 'ws://<lan-host>:7331' --code '<room-code>' --handle your_handle
```

Allow the chosen TCP port through Windows Defender Firewall only on the intended
private network. Problem links use Windows' default browser; if browser or
clipboard integration is unavailable, CFVirtual prints the URL for manual use.
