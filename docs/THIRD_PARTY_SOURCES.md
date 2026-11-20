# Third-party source archives

The release asset `third-party-sources.zip` contains corresponding source distributions for the unmodified Cygwin and WinDivert binaries embedded in Relay. Their binaries were matched by SHA-256 against the official Cygwin 3.4.10-1 and WinDivert 2.2.2-A binary distributions.

- **Cygwin-3.4.10-1-src.tar.xz**: [upstream archive](https://ftp.cvut.cz/mirrors/cygwin.com/x86_64/release/cygwin/cygwin-3.4.10-1-src.tar.xz), SHA-256 `ac70af0d4e644732f74946f55f7bafe010bab9b9da39a9327c3f0367f1fa43a2`.
- **WinDivert-v2.2.2-source.zip**: [upstream archive](https://github.com/basil00/WinDivert/archive/refs/tags/v2.2.2.zip), SHA-256 `65ec79c9e6afa99f648a3f4d1f6db794640b40d0b65bd438770ea503ee14ecb7`.

Cygwin build/package instructions and the upstream source tree are in its source distribution. WinDivert source includes its README and build support. These components remain separate dynamic files after Relay unpacks them. Modified compatible binaries can be substituted by rebuilding the embedded archive and manifest with the supplied scripts; Relay does not require publisher signatures for local rebuilds. Keep applicable licenses with redistributed files.

The Rust Telegram port is pinned in Cargo.toml/Cargo.lock; its source is available at https://github.com/valnesfjord/tg-ws-proxy-rs/tree/53f392280a370199ffe5c6ef0ac88808ed17c575. The Flowseal bundle and additional upstream sources are attributed in THIRD_PARTY.md.
