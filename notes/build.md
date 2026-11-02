# Build, deploy, and test

## Workspace

- `default-members` is core and desktop. A plain `cargo build`, `cargo test`, or `cargo run` builds the core tests and the desktop binary. It never builds the InkView crate.
- The ARM build always passes `-p pbanki`. Thus the winit features of the desktop never unify into the ARM build.

## Toolchain

- `rust-toolchain.toml` pins Rust `1.99.1`. It lists the components (`rust-src`, `clippy`, `rustfmt`, `rust-analyzer`) and both armv7 targets. Rustup installs them on first use.
- The pin follows anki `main`, which pins `1.99.0`. Bump the pin together with anki updates, not alone. Keep `cargo update` out of the same change.
- After a toolchain bump, do these checks:
  1. Run `make release` and `make release-rk3566`.
  2. Make sure that the glibc version is 2.38 or lower (`readelf -V`).
  3. Make sure that the float ABI is correct (`readelf -h`): soft-float for base, hard-float for RK3566.
  4. Measure the PB710 timing baselines again (see `performance.md`).
- Slint 1.17+ needs Rust 1.92 or newer.
- `rustc` and `cargo` must be rustup proxies. Pacman `rustup` puts them in `/usr/bin`. The rustup-init install puts them in `~/.cargo/bin`.
- A distro `rust` or `cargo` package ignores `rust-toolchain.toml` and has no armv7 std. ARM builds then fail with `can't find crate for core`. To check, run `rustc --print sysroot`. The result must be under `~/.rustup/toolchains/1.99.1-...`.

## Zig and cargo-zigbuild

- `mise.toml` pins Zig 0.14.1 and `cargo-zigbuild` 0.24.0. Run `mise install` once in the repository.
- The Makefile and `scripts/sdk-emulator/build.sh` add `mise bin-paths` to `PATH`. You do not need `mise activate`.
- If `make` cannot find `zig` or `cargo-zigbuild`, run `mise install`. Change versions in `mise.toml`, not by hand. Distro packages ship a newer Zig.

## PocketBook builds

Both builds are for firmware 6.10+ only. The glibc floor is 2.38, which the 6.10 and 6.11 `libinkview.so` need.

| Device | Float ABI | Make targets | Cargo target and feature | Bundle |
|---|---|---|---|---|
| B288/B300 (PB710) | soft | `make` (dev), `make release`, `make release-profile` | `armv7-unknown-linux-gnueabi.2.38`, `sdk-6-10` | `target/pocketbook/` |
| RK3566 | hard | `make rk3566`, `make release-rk3566`, `make release-rk3566-profile` | `armv7-unknown-linux-gnueabihf.2.38`, `sdk-6-11` | `target/pocketbook-rk3566/` |

All targets use `cargo zigbuild -p pbanki`. The two bundles have the same file names. The user installs the bundle for the device.

## Release bundle

- `pbanki.app` is a shell launcher (`scripts/pocketbook/pbanki.app`). `pbanki.bin` is the ARM binary.
- The launcher does these steps:
  - It sets `GLIBC_TUNABLES=glibc.rtld.execstack=2` for firmware 6.11.1480+ (the same workaround as KOReader).
  - It keeps one instance through `/tmp/pbanki.pid`. A second launch focuses the running instance with `iv2sh SetActiveTask`. It writes this to the log of the running instance (the second line of the pidfile).
  - It writes stdout and stderr of each launch to a new file, `pbanki/logs/pbanki-YYYYMMDD-HHMMSS.log`. It keeps the newest 20 files.
  - It moves the single `pbanki.log` of older launchers to `pbanki/logs/pbanki-legacy.log` once. Pruning does not delete this file.
  - If `pbanki/logs/` is not writable, it logs to `/tmp/pbanki-YYYYMMDD-HHMMSS-<pid>.log`.
  - It logs the start and the exit status. On exit code 126 or 127 it logs which bundle the device needs. If `/lib/ld-linux-armhf.so.3` exists, the device is RK3566.
- To deploy, copy both files to `/mnt/ext1/applications/`. Do not copy the raw binary as `pbanki.app`.
- Close pbAnki before you replace `pbanki.bin`. `/mnt/ext1` is FAT. If you overwrite a running executable, it can stop with `SIGBUS`.
- A launch while pbAnki is open only focuses it. Package imports and config changes need a real restart.
- The SDK emulator runs the x86-64 binary directly. It does not test the launcher or ARM code generation. Launcher changes need a device check.

## Desktop run

- `cd crates/desktop && cargo run`. Paths are relative to the working directory: the collection is `./pbanki/collection/` and the config is `./pbanki/config.toml`.
- If the config holds the device collection path, `crates/desktop/src/main.rs` changes it to `./pbanki/collection`.
- `PBANKI_CONFIG` overrides the config path in `crates/pocketbook/src/main.rs` (for host tests). The default is `/mnt/ext1/applications/pbanki/config.toml`.
- Anki builds the queue much more slowly in the dev profile. Measure card-load time with `make release`.

## Tests

There is no CI. Unit tests are inline `#[cfg(test)] mod tests` at the end of the module that they test.

| Command | Tests |
|---|---|
| `cargo test -p pbanki-core` | config, font names, rating (`study.rs`), sync hooks (`frontend/sync.rs`) |
| `cargo test -p pbanki --features sdk-6-10` | Wi-Fi ownership, keyboard truncation |
| `cargo test -p pbanki-desktop --features eink-sim` | simulator |

- The font-name test compares the header-only `name` reader with a full parse of every font under `/usr/share/fonts`.
- The rating test makes a temporary collection in the temp directory.

Tests in `../inkview-rs`:

- Device and update names: `cargo test -p inkview --no-default-features --features sdk-6-10 --lib`. The doctests fail on bindgen comments. This is a known problem.
- Update policy: `cargo test -p inkview-slint --no-default-features --features sdk-6-10`.

## Clippy

`make lint` runs these four clippy commands, then the ast-grep rules (see `lint.md`):

```sh
cargo clippy -p pbanki-core -p pbanki-desktop --all-targets
cargo clippy -p pbanki-desktop --features eink-sim --all-targets
cargo clippy -p pbanki --features sdk-6-10 --all-targets
cargo clippy -p pbanki --features sdk-6-11 --all-targets
```

Core has known warnings (collapsible `if`, `CardState` clones).

## Blitz bench

`bench/blitz-render/` is a headless Blitz card render benchmark. It is phase 0 of `deep-research/blitz-on-inkview.md`.

- It is not a workspace member. It has its own `Cargo.lock`, and it builds in `target/blitz-bench/`. Blitz brings its own versions of parley, fontique and skrifa, and the root `Cargo.lock` must not change. `make lint` does not cover it. Run `cargo clippy --manifest-path bench/blitz-render/Cargo.toml --all-targets`.
- The Blitz crates are git-pinned to the revision that the research note read. Change the revision in all four lines together.
- Stylo's build script needs `python3` on the host.
- Desktop run:

  ```sh
  CARGO_TARGET_DIR=target/blitz-bench cargo run --release --manifest-path bench/blitz-render/Cargo.toml -- \
      --out /tmp/blitz-bench --font <latin.ttf> --font <japanese.otf>
  ```

- Device: `make blitz-bench` (soft-float). Two variants go into the same bundle:
  - `make blitz-bench-softfp`: hardware VFP/NEON with the soft-float calling convention (see `performance.md`, "Float ABI of the PB710 build").
  - `make blitz-bench-mt`: vello_cpu's thread pool. It is slower on the PB710.
- `make blitz-bench-rk3566` makes a hard-float bundle.
- Copy the files of `target/pocketbook-bench/` to `/mnt/ext1/applications/` and start `blitz-bench.app`. The screen does not change. The launcher runs every `blitz-bench*.bin` next to it and writes `applications/blitz-bench-out/<binary>/blitz-bench.log` and PGM frames. Remove old variants from the device first, or it runs them too.
- Without `--font`, it loads Ryo Gothic and a Roboto from the firmware font directories. `--cards <dir>` adds `.html` files, for example real cards.
- Known render gaps at this revision: `<ruby>` is laid out inline. `<b>` needs a bold font file, because synthetic bold is behind Blitz's `font-embolden` feature.
