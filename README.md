# Drift

Drift is a remote desktop client for Mac, iPhone and iPad that connects to
[GNOME Remote Desktop](https://gitlab.gnome.org/GNOME/gnome-remote-desktop) (RDP). It is
written in Rust on [Tauri](https://tauri.app), with a vendored fork of
[IronRDP](https://github.com/Devolutions/IronRDP) for the protocol, hardware H.264 decoding and a
Metal compositor.

It supports both GNOME modes: **Remote Login** (the GDM greeter, then your session) and
**headless** sessions. It can also set up a Linux machine for you over SSH, either from inside
the app or with `host/drift-gnome-setup`.

## Build

You need macOS with Xcode, plus:

- Rust (the toolchain pinned in `rust-toolchain.toml` installs itself through `rustup`)
- [bun](https://bun.sh) for the web UI
- `cargo install tauri-cli`
- `git lfs` for the test fixtures (`git lfs install --local && git lfs pull`)

```sh
cargo xtask check          # fmt, clippy, tests
cargo xtask ci             # the full gate
cargo tauri dev            # run the app
```

`docs/team-conventions.md` lists every `cargo xtask` command. Design decisions are recorded as
ADRs in `docs/adr/`.

## Signing and distribution

The Apple team ID (`TEAMID0000`), the signing identity hashes and the bundle ID
(`com.example.drift`) in this repository are placeholders. To sign, notarize or ship your own
builds, replace them with your Apple Developer account's values; `xtask/src/store.rs` lists
where they go.

## Setting up a GNOME host

See `docs/gnome-host-setup.md`.

## License

MIT, see [LICENSE](LICENSE). Vendored code in `third_party/` keeps its own licenses
(IronRDP: MIT or Apache-2.0; tao: MIT or Apache-2.0).
