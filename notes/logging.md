# Logging

## Output

- All code logs through `tracing`. `pbanki_core::logging::init` installs one subscriber. Both frontends call it first in `main`.
- anki logs through `tracing` too. inkview-rs logs through the `log` crate, and the subscriber bridges these events. Thus all logs go to one place.
- The subscriber writes to stderr. On the PocketBook, the launcher writes stdout and stderr to a new file for each launch: `pbanki/logs/pbanki-YYYYMMDD-HHMMSS.log`. It keeps the newest 20 files (see `build.md`).
- The launcher also writes panics, loader errors, and the output of InkView itself into the same file.

Line format:

```text
2026-11-02 15:26:56.727  INFO [eink +50] request reason=Launch
```

- The time is local time with milliseconds. It matches the time format of the launcher lines.
- The message keeps its prefix: `[eink +<ms>]`, `[job N]`, `[deck …]`, `[sync]`, `[wifi]`, `[font]`, `[device]`, `[profile]`, `[panic]`.

## Levels and filter

- `PBANKI_LOG` sets the filter in `EnvFilter` syntax. The default is `info`. An incorrect value gives a warning and the default.
- Examples:
  - `PBANKI_LOG=warn` — warnings and errors only.
  - `PBANKI_LOG=info,eink=off` — no e-ink backend lines.
  - `PBANKI_LOG=info,anki=debug` — debug lines from anki.
- Targets: `eink` for the e-ink backend (`inkview-slint`). Other events use their module path (`pbanki_core::…`, `inkview::…`, `anki::…`).
- anki compiles out `trace` events in release builds (`release_max_level_debug`).

| Level | Use |
|---|---|
| `error` | An operation failed: collection open, card answer, import, card load |
| `warn` | Degraded behavior: missing fonts, config problems, update fallbacks |
| `info` | Normal events: e-ink backend, card jobs, deck entry, imports, device |

## Rules

- Do not use `eprintln!`, `println!`, or `print!` outside `mod tests` and `build.rs`. The lint rule `pbanki-no-eprintln` checks this.
- One exception: `logging::init` uses `eprintln!` if the subscriber cannot be installed.
- In `tracing` macros, do not use a variable named `display` or `debug` as a format argument. The macros shadow these names. Bind the value to another name first.
- `profile_log!` logs at `info` with the `[profile]` prefix. It compiles to nothing without the `profile` feature.
