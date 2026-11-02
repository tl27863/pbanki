# E-ink logs

The backend writes `[eink +<ms>]` lines through the `log` crate with the target `eink`. They go to the session log `pbanki/logs/pbanki-YYYYMMDD-HHMMSS.log` (see `logging.md`). Each line has the local time. The `+<ms>` value is the time from backend start.

## Startup lines

In this order:

1. `[device] …`
2. The backend header: size, bytes per pixel, scale factor, dpi, screen scale.
3. `[inkview] framebuffer WxH depth=… stride=… dpi=… scale=…`
4. `update policy: …`
5. `waveform times (ms, by waveform id): […]`
6. `panel mode=system height=… (raw …) fb_offset=… show_reader=…`

About the panel line:

- It shows only when the panel is enabled. No line means `PanelMode::Hidden`.
- `height=0` means that `PanelHeight()` returned a value ≤ 0 or ≥ the screen height. Then the backend keeps no rows free. `raw` shows the value that InkView returned.

## Runtime lines

| Line | Meaning |
|---|---|
| `request reason=…` | A full present was requested. `coalesced into …` means that it merged with a pending request. |
| `issued reason=… after=… waited=… attempts=… busy=… blit=…` | The full present was sent. |
| `verify issued …` | The verification present was sent. |
| `panel busy→idle after …ms` | The panel became idle. The backend checks every 50 ms while busy. |
| `first full frame rendered in …ms` | First render after start. |
| `slow render: …` | A render took a long time. |
| `update partial\|dynamic\|finalize WxH+X+Y` | One content update. |
| `damage … deferred to finalize (panel busy)` | Damage waits for the finalizer. |
| `damage … held for pending present` | Damage waits for a full present. |
| `pointer down\|up x,y`, `key down …` | Input. |
| `waking InkView loop after updates … rc=…` | The first cross-thread wake (logged once). |
| `finalize fallback: full …` | The panel seemed stuck. This is rare. |

- Update rectangles are in content coordinates, below the panel if there is one.
- `evt:` lines (also `PointerMove`) show only with `INKVIEW_SLINT_TRACE_EVENTS`.
- The `[job N]` lines of the card jobs are always on: `worker finished` and `delivering result to UI`. The profile build also logs `worker start delay`.

## How to find problems

- A content update must not log `request reason=App`.
- A dropped update shows as a full-screen `update …`, then `panel busy→idle` within about 100 ms. Nothing shows until the next `pointer down`.
- For a stale screen, the log tells if the screen was never rendered, never sent, or dropped by the panel.
