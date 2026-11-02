# Sync and Wi-Fi

## Sync

- `crates/core/src/sync/` does normal merge, full upload, full download, and media sync.
- Full sync and card loads both take the collection out of `LearnSession.collection` (an `Option`) for a short time. Then they open or put back the collection. Both use `LearnSession.busy`.
- Sync blocks the UI thread. There is no progress or abort. Add progress together with sync off the UI thread.
- `sync_on_exit` means a sync after you leave a study session (LearnScreen to Home). It does not mean a sync when the application stops.
- The sync settings are in `crates/core/ui/screens/settings.slint`. The gear on Home opens them. The reload icon starts a sync.
- `[ankiweb].endpoint` is blank for AnkiWeb. `sync_media` is true by default.
- Build `SyncState` through its constructors in `frontend/models.rs`. The lint rule `pbanki-sync-state-constructor` checks this.

## Sync hooks

`pbanki_core::frontend` has `FrontendHooks { sync: SyncHooks, full_present }`. The frontend passes it to `app::start`.

`SyncHooks { before_sync, after_sync }` runs for each sync path: manual, full upload, full download, auto-sync at startup, and `sync_on_exit`.

- `before_sync` runs on the UI thread. It must be fast and idempotent.
- `before_sync` can show a platform prompt (for example Wi-Fi setup). It must not block for an unlimited time by itself.
- If `before_sync` fails, the app sets a `SyncState` error and does not sync.
- `after_sync` runs exactly once if `before_sync` returned `Ok`. This includes a failed sync and a sync that panics.
- `after_sync` never runs after a failed `before_sync`.
- The desktop frontend passes no-op hooks.

## Wi-Fi (PocketBook)

`crates/pocketbook/src/network.rs` owns Wi-Fi.

`ensure_connected()`:

1. It checks `QueryNetwork() & NET_CONNECTED`.
2. If there is no connection, it starts the radio with `WiFiPower(1)`.
3. Then it shows the system `NetConnect(NULL)` dialog. The user can enable and select Wi-Fi without leaving the app.

Failures give actionable messages. They do not open the sync settings.

Do not use `NetConnectSilent`. On the PB710 a cold radio can block for more than a minute. A call from another thread can return `NET_ABORTED`. The lint rule `pbanki-no-netconnect-silent` checks this.

`disconnect_after_sync()`:

- It disconnects only connections that the app made.
- After it connects, the app records the active SSID with `NetInfo()`. If the SSID changed during the sync, it does not call `NetDisconnect()`. Thus a network that the user selected during the sync stays connected.
- If Wi-Fi was connected before the sync, the app leaves it connected.

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
- `[wifi]` lines show if the network was already connected, if pbAnki connected it (time and status), and if pbAnki disconnected it after the sync. They do not show network names (SSIDs).
- anki logs its sync steps at `debug`. To see them, set `PBANKI_LOG=info,anki=debug`.

## Dev sync server

- It is an external Docker Compose project at `../pbanki-sync-server` (not in this repository).
- It uses `zorrn/anki-sync-server:25.09.2`, pinned by digest. Data is in `./data`. The dev credentials are `pbanki`/`devpass` in its `.env`.
- Commands: `make -C ../pbanki-sync-server up`. Also `down`, `logs`, and `smoke` (check).
- Endpoint: `http://127.0.0.1:27701/` for the desktop and the host. The SDK emulator needs `EMU_NET=host`, because the server binds 127.0.0.1.
- For LAN or device access, set `SYNC_BIND_ADDR=0.0.0.0` in the `.env` of the server.
- The client supports `[ankiweb].endpoint` and full upload and download. You can start a new server from the full-sync prompt of the UI. The server starts empty: select Upload on the first sync.
