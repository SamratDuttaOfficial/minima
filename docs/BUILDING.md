# Building Minima

## Requirements

* Rust (stable) from <https://rustup.rs>
* A C compiler for the target. `ring` (TLS) compiles a little C: MSVC build tools or MinGW on
  Windows, `build-essential` on Linux, Xcode command-line tools on macOS.

## One command

```
scripts/build.sh            # Linux, macOS, Git Bash
scripts\build.bat           # Windows
```

Options (`/flag` on Windows, `--flag` elsewhere): `target TRIPLE` (repeatable), `all`
(every target in `scripts/targets.txt` that can be built here), `no-installers`, `debug`,
`clean`. On Linux, `--static` builds against musl so the binary runs on very old distributions
(`rustup target add x86_64-unknown-linux-musl`, plus `musl-tools`).

Everything lands in `dist/` with a `SHA256SUMS` file.

## What each OS gets

| OS | Files in `dist/` | Needs |
|---|---|---|
| Windows | `.zip`, `-installer-cli.exe`, `-setup.exe` | Inno Setup 6 for `-setup.exe` (`winget install JRSoftware.InnoSetup`); skipped with a message if missing |
| Linux | `.tar.gz`, `-installer-cli`, `.deb`, `.rpm` | `rpmbuild` for `.rpm` (`apt install rpm`); skipped if missing |
| macOS | `.tar.gz`, `-installer-cli`, `.pkg`, `.dmg` | run on a Mac (`pkgbuild`, `hdiutil`). Set `MACOS_SIGN_APP` / `MACOS_SIGN_PKG` to sign |

The `.zip`, `.tar.gz`, `.deb` and checksums are made by `crates/xtask` (pure Rust, same on every
OS). The `-installer-cli` file is `crates/minima-installer` with the browser binary embedded
(`MINIMA_PAYLOAD`); it needs nothing else to run.

## Cross-compiling

Native installers (`.pkg`, `.dmg`, Inno Setup) can only be built on their own OS. Plain binaries,
archives, `.deb` and the CLI installer can be cross-built with
[`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) (`cargo install cargo-zigbuild`
plus `zig`), which `build.sh`/`build.bat` use automatically for non-host targets. A CI matrix
(one runner per OS running the script) is the most reliable way to produce all of them.

## Verification status

Built and exercised on Windows 11 (GNU toolchain): binary, zip, CLI installer
(install/uninstall), Inno Setup installer (built, not run). Type-checked for Linux:
installer and xtask. The `.deb` was generated and its structure inspected, but not installed.
macOS scripts and `.rpm` have not been run.

## Tests

```
cargo test --workspace
```
