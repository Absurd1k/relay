# Third-party components

Relay is independent of these projects; names identify provenance and imply no endorsement.

## Telegram Rust library

**tg-ws-proxy-rs 2.7.0**, MIT, Copyright (c) 2026 valnesfjord. [Pinned source](https://github.com/valnesfjord/tg-ws-proxy-rs/tree/53f392280a370199ffe5c6ef0ac88808ed17c575). Integrated unmodified through a pinned Cargo git dependency. Its design derives from **Flowseal/tg-ws-proxy**, MIT, Copyright (c) 2026 Flowseal. Both full licenses are embedded under `licenses/` in `assets/engines.zip` and extracted at runtime. Python is not shipped.

## zapret Windows bundle

**zapret-discord-youtube 1.10.3**, Copyright (c) 2024–2026 Flowseal; **zapret**, Copyright (c) 2016–2026 bol-van, MIT. [Official release](https://github.com/Flowseal/zapret-discord-youtube/releases/tag/1.10.3). Archive SHA-256: `244314ae1c24538a0d751601da8e0c925c843371eec4456eb15f14c4fd6b7058`.

**WinDivert**, distributed unmodified under LGPLv3 (dual-licensed LGPLv3/GPLv2). Full texts are embedded at `zapret/LICENSE-WinDivert.txt`. [Corresponding source and build instructions](https://github.com/basil00/WinDivert). DLL and driver remain separate after extraction. Rebuilding the embedded archive and manifest permits replacement/relinking; there is no signing lock.

**Cygwin runtime 3.4.10**, distributed unmodified from the upstream archive. [Cygwin licensing and source access](https://cygwin.com/licensing.html), [source repository](https://cygwin.com/git/?p=newlib-cygwin.git). Its license text is included in `zapret/LICENSE-Cygwin.txt`. The runtime remains a separate DLL. When publishing binary distributions, retain all applicable license texts and corresponding source access required by upstream licenses.

## Rust dependencies

Corresponding source packages for the redistributed Cygwin and WinDivert binaries are included as the release asset `third-party-sources.zip`; their official binary packages were checked against the embedded DLL/driver hashes. Source archive URLs and hashes are listed in [docs/THIRD_PARTY_SOURCES.md](docs/THIRD_PARTY_SOURCES.md).

Dependencies are fixed in `Cargo.lock`. Major components include Tokio (MIT), RustCrypto AES/CTR/SHA (MIT OR Apache-2.0), ureq (MIT OR Apache-2.0), rustls (Apache-2.0 OR ISC OR MIT), ring (ISC and component-specific notices), and windows-sys (MIT OR Apache-2.0). Original package license files are available from their crates.io source distributions. See `docs/RUST_LICENSES.txt` for included notices.

## AI provider list

**dns.malw.link**, MIT, [source commit 062b1a8687e6c2606c0bd4b06e833e65cfaefb29](https://github.com/ImMALWARE/dns.malw.link/tree/062b1a8687e6c2606c0bd4b06e833e65cfaefb29). A filtered snapshot containing only supported AI domains is embedded as `assets/ai-hosts.txt`. Its full license is embedded as `licenses/dns.malw.link-MIT.txt`. Relay does not operate the public relay infrastructure.

## Research links

GeoFree and EuroDNS were examined for research; their code is not included. Relay uses its own Rust hosts-editing implementation and the separately attributed dns.malw.link domain/IP list.
