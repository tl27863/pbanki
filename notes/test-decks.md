# Test decks and the card render bench

Tools to measure what CSS and media cost in the card renderer, on the host and on the PB710. They give the numbers for the plan in `deep-research/card-page-cache.md`. The log lines are described in `blitz-card-render.md`, "Log lines".

## Files

- `scripts/test-decks/make-css-bench.py` builds `target/test-decks/pbanki-css-bench.apkg`: one card per CSS or media feature (shadows, gradients, clips, text, images in several formats, Yap card variants). It also writes `pbanki-css-bench.manifest.json` (card id → variant, group, description). It needs the Anki Python package and Pillow, and it uses a new collection in a temp folder.
- `crates/core/examples/render_deck.rs` imports an `.apkg` into a temp collection and renders every card as `next_card` does (`study::card_documents`, then `CardRenderer::render`), then every other page. It logs the device `[card]` lines plus one `[bench] card=… variant=… run=…` line before each render.
- `scripts/test-decks/analyze-render-log.py` reads a host log or a device session log and prints one Markdown row per variant (median of the renders), with a Δ column against `01-plain`.

## Host run

```sh
python3 -I scripts/test-decks/make-css-bench.py
cargo run --profile host-bench -p pbanki-core --example render_deck -- \
    target/test-decks/pbanki-css-bench.apkg --runs 5 --dump /tmp/pbanki-css-dump 2> /tmp/render-deck.log
python3 -I scripts/test-decks/analyze-render-log.py /tmp/render-deck.log --skip-first
```

- `host-bench` (root `Cargo.toml`) is the release profile without fat LTO, so it builds in seconds.
- `--dump DIR` writes the pages of the first run as PGM. Use it to check that a variant renders what it should (for example `magick montage`).
- `--skip-first` drops the first render of each card (cold caches).
- The image cache keeps images between runs and cards. With `--runs` above 1, the later runs of an image card are cache hits. `--no-image-cache` turns the cache off (the cost before the cache). Shadows are off as in the app; `--shadows` is `card_shadows = true` (use it to measure the shadow variants).
- `--lookahead MS`: after each card, send the next two as lookahead and wait MS (reading time), as the app does. The next card is then a lookahead hit.
- The analyzer's `wait` column is the time the card job waited for the first pages; `*` marks a lookahead hit. `total` is the render work, also when it ran ahead. Logs before render-ahead show `—`.
- In the deck, `48`, `49`, `4a`, `4d`, `50`–`54` show the WebP of `40`, `55` the JPEG of `41`, and `56` the BMP of `44`. With the cache, they are cache hits in a run in deck order.

## Device run

Set `card_shadows = true` in the config to measure the shadow variants (the default removes them).

1. Copy `pbanki-css-bench.apkg` into the collection directory. pbAnki imports it on the next start (`collection-and-config.md`).
2. Study the deck from the start. Show the answer of every card and go through all pages. Rate Good.
3. Run the analyzer on the session log from `pbanki/logs/`. The device has no `[bench]` lines, so the analyzer finds the card from the `[card] lookahead` line of the same job. It uses manifest order for the first card after a deck entry (marked `*`).

The variant names start with a number, and the card due order follows the numbers. The deck has its own preset: no daily limits and no learning steps, so all cards show once, in order. To run it again, delete the deck or forget its cards first: a re-import updates the notes but keeps the scheduling. `00-warmup` takes the cold start. `99-plain-last` repeats `01-plain` at the end to show drift over the session.

## Notes on the deck

- Anki packs a `_` media file only when a note type's templates or CSS reference it. So `36-webfont` has its own note type, with the `@font-face` rule in its CSS.
- The poster pictures are random noise with a tint. The WebP is about the size of a Yap poster (30 KB against 24 KB).

## Host results (2026-11-05, Gray8 1144×1053@3, ms, median)

Device times are about 30–100× higher. Use the host numbers to compare variants, not to predict device times.

- Image decode is the largest cost: WebP 400×600 8.0, lossless WebP 5.8, PNG 2.4, GIF 2.0, JPEG 1.1, BMP 0.6, JPEG 1600×2400 16.
- No decode is reused between documents: `48-thumb-webp-again` decodes again (8.0). Inside one document, Blitz decodes a file once (`49-thumb-webp-x3`: 7.9).
- `56-yap-bmp-poster` (what a decoded-image cache hit costs, A1): decode 0.6 against 8.0, total 6.0 against 14.
- Blurred shadows cost raster time on each page: Yap's shadow +2.4 per page, blur 64 +4, inset +4, three shadows +4.7. A shadow without blur, `text-shadow`, and `filter: drop-shadow` cost almost nothing.
- `52-yap-no-poster` against `50-yap`: the poster is 8 of the 14. `51-yap-no-shadow`: the shadow is about 2.3 of the first page.
- Gradients, opacity, transforms, clips, borders, tables, ruby, and inline SVG each add less than 2.

## PB710 results (2026-11-05, `pbanki-20261105-115405.log`, ms)

One render per card, so differences under about 20 ms are noise (`01-plain` 147, `02-plain-again` 159, `99-plain-last` 153). The decode column adds up both documents of a card (question and answer).

- **Fixed cost per page:** about 62 for a near-empty page: record 16–23, raster 27, convert 17. A plain card (two pages) is about 150.
- **Image decode** (two documents; one decode is half): WebP 400×600 186, lossless WebP 217, gray JPEG 235, PNG 105, JPEG 81, GIF 65, BMP 38, JPEG 1600×2400 1226. No reuse across cards (`48-thumb-webp-again` 180). A CSS background is fetched once (`4d`: 87).
- **The large JPEG** also adds about 190 record time per page: the 1600×2400 RGBA image is processed on every page. Images scaled in the page cost raster time: `4a-webp-full` +110 per page, twelve icons +80 per page.
- **Blurred shadows** cost raster time on every page that shows them, and the cost grows with the blur radius: blur 4 +75, Yap's 24 +160–180, blur 64 +250–330, inset or three shadows +330–350. Eight small shadowed chips +170. A shadow without blur, `text-shadow`, and `filter: drop-shadow` cost little (+3–20).
- **Other CSS:** radial gradient on the whole page +80–110 per page, linear gradient +35, opacity or rounded clip +20, dashed borders +30 record. Ruby (32 words) +200 per card (resolve 55, record 75 per page). Inline SVG +100 resolve. 200 styled spans +100 record per page.
- **Yap:** `50-yap` 619; without the shadow 447 (−172); with a BMP poster 490 (−129); without shadow and poster 194 (the floor for A1 + A2).
- **Cold start:** the first card is about 110 slower.

## PB710 with the image cache (2026-11-05, `pbanki-20261105-122856.log`, shadows on)

- First card with an image (question misses, answer hits): WebP 186 → 118, PNG 105 → 74, JPEG 81 → 59, gray JPEG 235 → 141, BMP 38 → 32.
- Later cards with a cached image: 14–16 for two documents, about 7 per hit (`48-thumb-webp-again` 180 → 14, total 374 → 200). CSS background 87 → 7.
- Twelve icons: 18 → 15. They are small; the cost is per file, not per pixel.
- `47-thumb-jpeg-large` got worse (1226 → 1505): its 15 MB of RGBA was over the 6 MB limit, so it was converted but not kept. Fixed after this run: larger images go to Blitz unchanged, and the limit is now 16 MB (budget 32 MB), so this image is kept.
- Yap: `50-yap` 619 → 463, `51-yap-no-shadow` 447 → 257. With `card_shadows = false`, a Yap card should be about 260, below the 300 ms rating overlay delay.
- No change where no image is involved (shadows, text, paint), as expected.

## PB710, 16 MB limit and `card_shadows` (2026-11-05, `pbanki-20261105-124101.log` on, `pbanki-20261105-124436.log` off)

- `47-thumb-jpeg-large` with the 16 MB limit: decode 1505 → 878 (the answer is a hit), total 2052 → 1421.
- Shadows off: all shadow variants cost the same as `10-panel-flat` (152–176 against 163). `text-shadow` is unchanged (+23–29).
- Yap with cache and shadows off: `50-yap` 257 (layout 60, pages 89 / 107 / 84), against 619 before the cache and 461 with shadows on. Below the 300 ms rating overlay delay.
- What is left on a Yap card is mostly the fixed cost of a page (about 62–70: record about 20–30, raster about 30–40, convert 18) and layout (about 60).

