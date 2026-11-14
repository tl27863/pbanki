# Sync and Wi-Fi

## Sync

- `crates/core/src/sync/` does normal merge, full upload, full download, and media sync.
- Full sync and card loads both take the collection out of `LearnSession.collection` (an `Option`) for a short time. Then they open or put back the collection. Both use `LearnSession.busy`.
- Sync runs on a worker thread as a collection job (`spawn_collection_job` in `frontend/card_jobs.rs`, see `card-loading.md`). The UI thread stays free, so the sync icon can animate. There is no progress or abort.
- `sync_now` and `full_sync` get the collection (`&mut Option<Collection>`), a copy of the config, and the `SyncManager` (an `Arc` in `LearnSession`). They do not save the config. They return `SyncOutcome { result, changes }`; the UI thread calls `apply_config` to write a new token or a moved endpoint and save.
- While a sync runs, `busy` is true: the app ignores deck entry, Settings open and back, and Save in Settings.
- If the worker panics, the result is an error ("Sync stopped unexpectedly") and the collection is lost until a restart.
- `sync_on_exit` means a sync after you leave a study session (LearnScreen to Home). It does not mean a sync when the application stops.
- The sync settings are in `crates/core/ui/screens/settings_sync.slint` (Settings, Sync tab). The settings button on Home opens them. The sync button on Home starts a sync and is also the indicator: it steps through two frames while the sync runs, then shows the done icon or the error icon. Home has no status text. The message is on the Sync tab, and a tap on the error icon opens it. A media warning shows the error icon too. A finished result ends at the next screen change (deck entry, Home, Stats, Settings, or back; `end_finished_sync` in `frontend/sync.rs`, `SyncState::after_screen_change`): Home shows the sync icon again, and the Sync tab does not show "Sync complete" or "Settings saved" from another screen. A running sync, an error, a media warning, and a required full sync stay until the next sync.
- When a full sync is required, the Sync tab shows the notice, the confirm toggle, and Upload and Download in place of the form (both do not fit on a 357×482 window), until a full sync or another sync result.
- `[ankiweb].endpoint` is blank for AnkiWeb. `sync_media` is true by default.
- Build `SyncState` through its constructors in `frontend/models.rs`. The lint rule `pbanki-sync-state-constructor` checks this.

## Sync hooks

`pbanki_core::frontend` has `FrontendHooks { sync: SyncHooks, full_present, full_refresh, page_format }`. The frontend passes it to `app::start`.

`SyncHooks { before_sync, after_sync }` runs for each sync path: manual, full upload, full download, auto-sync at startup, and `sync_on_exit`.

- `before_sync` and `after_sync` run on the UI thread. The sync runs on the worker between them. `before_sync` must be fast and idempotent.
- `before_sync` can show a platform prompt (for example Wi-Fi setup). It must not block for an unlimited time by itself.
- If `before_sync` fails, the app sets a `SyncState` error and does not sync.
- `after_sync` runs exactly once if `before_sync` returned `Ok`. This includes a failed sync and a worker that panics: the `SyncGuard` lives in the finish closure of the job, and the runner delivers a disconnected channel after a panic.
- `after_sync` never runs after a failed `before_sync`.
- The desktop frontend passes no-op hooks.

## Wi-Fi (PocketBook)

`crates/pocketbook/src/network.rs` owns Wi-Fi.

`ensure_connected()`:

1. It checks `QueryNetwork() & NET_CONNECTED`.
2. If there is no connection, it checks `QueryNetwork() & NET_WIFIREADY`. If the radio is off, it starts the radio with `WiFiPower(1)` and records that pbAnki powered it.
3. Then it shows the system `NetConnect(NULL)` dialog. The user can enable and select Wi-Fi without leaving the app.
4. After the dialog, it calls `inkview_slint::request_full_present()`. The dialog drew over the frame, and InkView does not always send `Foreground` after it (no event when the radio was already on). The request is consumed when the timer callback that started the sync returns, so the present shows the syncing state. The sync result arrives later with an explicit redraw.
5. If the connection fails and pbAnki powered the radio, it powers the radio off again.

Failures give actionable messages. They do not open the sync settings.

Do not use `NetConnectSilent`. On the PB710 a cold radio can block for more than a minute. A call from another thread can return `NET_ABORTED`. The lint rule `pbanki-no-netconnect-silent` checks this.

`disconnect_after_sync()`:

- It disconnects only connections that the app made.
- After it connects, the app records the active SSID with `NetInfo()`. If the SSID changed during the sync, it does not call `NetDisconnect()`. Thus a network that the user selected during the sync stays connected.
- If Wi-Fi was connected before the sync, the app leaves it connected.
- If pbAnki powered the radio, it calls `WiFiPower(0)` after `NetDisconnect()`. `NetDisconnect()` alone leaves the radio on, and the system then reconnects by itself. If the radio was on before (connected or idle), it stays on.

Other InkView network calls: `NetConnect` (interactive dialog), `NetConnect2`, `NetConnectAsync`, `NetConnect3` (6.8+), `GetNetState`, `GetLastNetConnectionError`, `NetInfo`, `EnumWirelessNetworks`, and `NetMgrPing`. `inkview::Event` does not show `EVT_NET_CONNECTED` and `EVT_NET_DISCONNECTED`. A network status would need polling.

## Sync log lines

Each sync logs these lines (see `logging.md`):

| Line | Level |
|---|---|
| `[sync] <kind>: started (preparation N ms)` | info |
| `[sync] <kind>: done in N ms, <action>: <message>` | info |
| `[sync] <kind>: full sync required after N ms, …` | warn |
| `[sync] <kind>: failed after N ms: <message>` | error |
| `[sync] <kind>: not started: <error>` (for example Wi-Fi failed) | warn |
| `[sync] <kind>: ignored, collection busy` or `sync already running` | info |
| `[sync] media: done in N ms`, `media: off`, or `<kind>: Media sync failed: …` | info, warn |
| `[sync] backed up the local collection before download` | info |
| `[sync] server moved this account to <endpoint>` | info |

- `<kind>` is `manual`, `startup`, `after study`, `full upload`, or `full download`.
- The preparation time includes the Wi-Fi dialog. The sync time does not.
- `[wifi]` lines show if the network was already connected, the radio state and raw `QueryNetwork` flags, if pbAnki connected it (time and status), and if pbAnki disconnected it or powered the radio off after the sync. They do not show network names (SSIDs).
- anki logs its sync steps at `debug`. To see them, set `PBANKI_LOG=info,anki=debug`.

## Tests

`crates/core/src/sync/tests.rs` runs `sync_now` and `full_sync` against anki's `SimpleServer` (the code of `anki-sync-server`). The tests start it in-process on a free port, one server per test. They need no Docker. The layout follows anki's `rslib/src/sync/collection/tests.rs`.

- Covered: login and the saved token, a wrong password, a token for another endpoint, full upload and download (with the backup), normal sync both ways, media, and the `308` endpoint move (a small redirector thread).
- Tests are plain `#[test]`: `sync_now` makes its own runtime. The server runs on its own thread.
- `frontend/sync.rs` tests the guard: prepare failure, panic on the UI thread, a worker panic, and a worker result (the last two through the job runner, without an event loop).
- Each test config sets `Config.path` to a temp dir. The default path is the device path.
- `dev_server_roundtrip` (ignored) runs against the dev sync server. It checks anki `main` against the pinned server image. It **replaces the account's collection** on that server:

  ```sh
  make -C ../pbanki-sync-server up
  PBANKI_SYNC_TEST_ENDPOINT=http://127.0.0.1:27701/ cargo test -p pbanki-core dev_server_roundtrip -- --ignored
  ```

## Dev sync server

- It is an external Docker Compose project at `../pbanki-sync-server` (not in this repository).
- It uses `zorrn/anki-sync-server:25.09.2`, pinned by digest. Data is in `./data`. The dev credentials are `pbanki`/`devpass` in its `.env`.
- Commands: `make -C ../pbanki-sync-server up`. Also `down`, `logs`, and `smoke` (check).
- Endpoint: `http://127.0.0.1:27701/` for the desktop and the host. The SDK emulator needs `EMU_NET=host`, because the server binds 127.0.0.1.
- For LAN or device access, set `SYNC_BIND_ADDR=0.0.0.0` in the `.env` of the server.
- The client supports `[ankiweb].endpoint` and full upload and download. You can start a new server from the full-sync prompt of the UI. The server starts empty: select Upload on the first sync.
