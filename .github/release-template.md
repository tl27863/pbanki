## Install

Needs PocketBook firmware 6.10 or later. Pick the zip for your device:

| Devices | Zip |
|---|---|
| B288/B300 (soft-float): PB700, PB710, 743G, 634, 629, ... | `pbanki-v__VERSION__-b288-b300.zip` |
| RK3566 (hard-float): 1030 | `pbanki-v__VERSION__-rk3566.zip` |

1. Close pbAnki on the device. Replacing `pbanki.bin` while it runs can crash it.
2. Unzip and copy `pbanki.app` and `pbanki.bin` to `applications/` on the device.
3. Start pbAnki from the applications list.

If the device has the wrong build, the log in `applications/pbanki/logs/` says which one it needs.
`SHA256SUMS` holds the checksums of the zips.
