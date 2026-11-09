# Lint rules (ast-grep)

## Files

- `sgconfig.yml` is the configuration.
- `lint/rules/*.yml` holds one rule per file. Rule IDs start with `pbanki-`.
- `lint/rule-tests/*-test.yml` holds `valid` and `invalid` examples for each rule.

## Rules

The rules check invariants that clippy cannot check.

| Rule | Language | Check |
|---|---|---|
| `pbanki-no-repeated-timer` | Rust | No repeating Slint timers |
| `pbanki-no-eprintln` | Rust | No print macros outside `mod tests` and `build.rs`. Use `tracing` |
| `pbanki-no-wait-for-update-complete` | Rust | No `wait_for_update_complete` or `WaitForUpdateComplete` |
| `pbanki-no-netconnect-silent` | Rust | No `NetConnectSilent` |
| `pbanki-no-slint-font-env` | Rust | No `SLINT_FONT_PATH` or `SLINT_DEFAULT_FONT` |
| `pbanki-sync-state-constructor` | Rust | Build `SyncState` through its constructors outside `frontend/models.rs` |
| `pbanki-collection-busy-check` | Rust | Hint: collection borrows in `ui.on_*` closures without a `busy` check |
| `pbanki-show-new-card` | Rust | A new card goes through `show_new_card` (resets the page state first), not `set_current_card` |
| `pbanki-checkbox-font-size` | Slint | Each CheckBox sets `font-size` |
| `pbanki-no-focus-in-init` | Slint | No `focus()` in `init` |
| `pbanki-study-controls-no-states` | Slint | No `states`, `transitions`, `animate`, `.pressed`, or `.has-hover` in `plain_button.slint` and `rating_button.slint` |
| `pbanki-no-slint-color-literal` | Slint | No color literals outside `ui/theme.slint`. Use a `Theme` token |

## Run

`make lint` does these steps:

1. It builds the Slint parser if necessary (`make lint-parsers`). This clones `slint-ui/tree-sitter-slint` at the commit that the Makefile pins into `target/ast-grep/`. Then the tree-sitter CLI builds `slint.so`.
2. It runs the four clippy commands (see `build.md`).
3. It runs `ast-grep test`.
4. It runs `ast-grep scan`.

The scan exits with 1 on `error` rules. `warning` and `hint` rules only report.

Requirements:

- `cargo install ast-grep --locked` (0.51). `~/.cargo/bin` does not have to be on `PATH`, because the Makefile falls back to it.
- `npm i -g tree-sitter-cli`.

## Slint support

Slint is a custom language. ast-grep loads the native parser only with `--custom-languages allow`. The Makefile passes this flag. Without it, ast-grep skips the Slint rules and does not fail.

## Workflow for agents

- For structural search and codemods, use `ast-grep run -l rust -p '<pattern>'`. For Slint, use `-l slint` with `--custom-languages allow`.
- To review each change of a codemod, use `-r '<rewrite>' -i`.
- First check how a pattern parses with `--debug-query=cst`. Slint fragments must be inside a `component X { … }`. Bare elements parse as `ERROR`.
- For a new rule:
  1. Add its tests.
  2. Run `ast-grep test --custom-languages allow --skip-snapshot-tests`.
  3. Make sure that the scan stays clean.
- For paths, use `kind` plus `regex`, not a pattern. The pattern `TimerMode::Repeated` does not match `slint::TimerMode::Repeated`.
- To allow a deliberate exception, add `// ast-grep-ignore: <rule-id>` and a reason.
- Do not run `ast-grep scan` from a directory with the `sgconfig.yml` of another project. ast-grep reads it from the working directory.
- The `joshuadavidthomas/ast-grep-rules` Rust pack was evaluated and not used. Its hits here were deliberate designs: public data fields, the startup panic, bool parameters.
