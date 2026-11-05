# Blitz card rendering (Blitz phase 2)

Status 2026-11-03: implemented, desktop-tested (eink-sim), both PocketBook targets build. The PB710 check (step 6) is open. "Implementation notes" below lists where the code differs from the plan.

This replaces the earlier phase 2 plan (an `inkview-blitz` shell with a Dioxus app port). That plan was a misunderstanding of the goal.

**User decision (2026-11-03):** Blitz renders only the card, and only in a study session. Slint stays the UI: screens, menu, buttons, loading screens, input, and the e-ink backend. This is the "Blitz for card HTML only" path of `deep-research/blitz-on-inkview.md`, with pagination added.

Phase 1 (`eink-presenter-refactor.md`) is done and stays. This plan does not need it.

Targets: black-and-white panels (`BB8` framebuffer, PB710) and color panels (`RGB24` framebuffer, see `inkview-backend.md`). Pages keep their colors on color panels.

## Goal

- The card question and answer show as Anki HTML with the note type CSS: formatting, tables, lists, colors, ruby, images. Today `text.rs` strips the HTML and pages plain text by a character count. That text path is removed, not kept as an option (git has it).
- Blitz lays out the card at the card area width and cuts it into pages of the card area height. No text line is cut at a page edge. Nothing is drawn outside the card area.
- Rendering runs off the UI thread. UI callbacks do not block (`AGENTS.md`).
- The e-ink update path does not change. Show answer, page turn and next card each give one update (`card-loading.md`, `eink-refresh.md`).

## Not in this plan (removed from the earlier plan)

- The `inkview-blitz` crate, the Blitz event loop, `Event`/waker/`ShellProvider`, the pacer, and the input mapping. Cards are static images. Slint handles all input.
- Dioxus and the tokio runtime on the UI thread.
- `EinkOptions::from_env` and moving the simulator to `inkview-eink`. You can do these later as cleanup. They do not block this plan.
- Animations. A page is one still frame. An override sheet (`html.rs`, `OVERRIDE_CSS`) sets `animation: none !important; transition: none !important`, so every element shows its end state. Resolving at `t = 0` alone is not enough: transitions do not start, but a CSS animation shows its *first* keyframe (see Implementation notes).
- Audio playback. It is planned for later. This plan keeps the AV tags for it (see Card HTML).
- JavaScript. Blitz has no JS. MathJax, and templates that use scripts, show their raw source (see Risks).

## What exists

| Part | Where | State |
|---|---|---|
| Card load on the worker, `LoadedCard` | `crates/core/src/study.rs` | Text pages: `strip_html_remove_brackets_and_paginate`, `paginate_text` |
| Page size input | `ui/main.slint` `chars-per-page` (from window size), passed through `deck_clicked` and `rate` | Character estimate only |
| Study screen | `ui/screens/learn.slint` | One `Text`, `current-page`, swipe and keys, `[n/m]` added to the text |
| Card HTML | `render_existing_card` → `RenderCardOutput { qnodes, anodes, css, … }` | `css` is the note type CSS. It is not used today. |
| Blitz frame cost on the PB710 | `bench/blitz-render`, `performance.md` | One new card page 83–98 ms (softfp) |
| RGBA→gray conversion | `bench/blitz-render/src/main.rs` (`convert_and_diff`) | About 30 ms full frame, NEON |
| Blitz pin | `d0f48bd` in the bench | APIs below checked in the source |

Blitz and parley APIs used here (checked at `d0f48bd`, parley 0.12.0):

- `BaseDocument::set_viewport_scroll(Point)`: it does not clamp. A negative or large value is accepted.
- `blitz-paint`: draws the document shifted by `viewport_scroll`. It culls elements outside the viewport (`render.rs`). Thus a page costs about one screen, also on a long card. The bench `multi-screen` fixture (about five screens) costs 195 ms, and `long-text` (one screen) costs 180 ms.
- `ElementData::inline_layout_data: Option<Box<TextLayout>>`. `TextLayout { layout: parley::Layout, block_offset, … }`. `block_offset` is in CSS px from the top of the content box.
- `parley::Layout::lines()` → `Line::metrics()` → `LineMetrics { block_min_coord, block_max_coord, content_block_min_coord, content_block_max_coord, … }`: the top and bottom of each line box.
- `Node::absolute_position(x, y)`, `Node::final_layout`, `Node::scroll_height()`, `BaseDocument::root_element()`.
- `BaseDocument::get_fragment_target("answer")`: the `<hr id=answer>` of Anki's default answer template.
- `ElementData::image_data()`: replaced `<img>` elements.

## Design

### Overview

```
UI thread (Slint)              card-job worker (collection)        card-render thread (Blitz)
deck_clicked / rate  ──spawn──▶ next_card / rate_card
                                 render_existing_card
                                 CardHtml ─────────── Render ─────▶ build 2 docs, resolve,
                                                                     paginate both sides,
                                 ◀─────────── FirstPages ──────────  render q page 1 + a page 1
◀── LoadedCard (pages) ───────── deliver (as today)
page turn ── Want (seq, side, n) ─────────────────────────────────▶
◀── PageReady (seq, side, n) ─────────────────── invoke_from_event_loop ── prefetch neighbor pages
```

### The card-render thread

One `std::thread` for the whole app run (`crates/core/src/card_render/`), started on the first deck entry. It owns:

- the `parley::FontContext` with the catalog fonts registered once (`fonts.rs`, as `register_catalog_fonts` does for Slint),
- the `VelloCpuImageRenderer`, the RGBA buffer and the scratch row (reused for each page),
- the two `HtmlDocument`s of the current card (question and answer).

Why a thread of its own:

- Stylo keeps thread-local state, and `HtmlDocument` is not `Send`. The card-job worker is a new `std::thread` for each job (`card_jobs.rs`), so it cannot keep a document for the prefetch of later pages.
- Fonts are registered once, not for each card.
- The collection worker returns as soon as the first pages are ready. The prefetch of later pages does not hold the collection or `busy`.

Messages (a `std::sync::mpsc` channel in each direction):

- `Render { seq, html: CardHtml, area: (w, h), scale }` → reply on a one-shot channel: `FirstPages`.
- When the prefetch is done, the thread waits for the next `Render`. A new `Render` stops the prefetch: the thread checks the channel between two pages.
- Prefetch results go to the UI with `slint::invoke_from_event_loop` and `seq`. The UI drops a page if its `seq` is not the current card. The UI then calls `request_redraw()` if that page shows now (the "explicit redraws" rule of `card-loading.md`).
- A panic in Blitz ends the render thread. The release profile has `panic = "abort"`, so on the device it ends the app. There is no `catch_unwind` (see Implementation notes).
- Prefetch window: the thread renders the pages next to the shown page (previous and next, for the question and from the answer start page), not all pages. On a page turn the UI sends `Want { seq, side, page }` for the new neighbor. The thread keeps the two documents of the current card, so it can render any page again. This keeps memory flat on long cards, also with color pages (see Page storage).
- Render ahead (2026-11-05, `deep-research/card-page-cache.md`, B): `next_card` sends `Render` without waiting (`start_render`), builds the documents of the next two queued cards, sends them as `Lookahead { cards, area }`, then waits for its pages. The thread keeps at most two lookahead cards. It works on them only when no message waits and the card on screen has no queued page. The work goes in units (layout, question page, answer start page), so a `Render` or `Want` waits at most one unit. A `Render` with the same `CardHtml` and area as a lookahead card answers from its work (`FirstPages::from_lookahead`); the documents become the card on screen. A new `Lookahead` list keeps the cards that are in both lists and drops the others. A deck entry with `set_deck` (also the first one after a sync) and an empty queue send an empty list, so pages from before a sync are not shown. Memory: about 5 MB per lookahead card on the PB710.

### Card HTML

The worker builds `CardHtml` from `RenderCardOutput`:

```html
<!DOCTYPE html><html><head><style>{BASE_CSS}{note type css}</style></head>
<body class="card card{ord+1}">{question or answer html}</body></html>
```

- `BASE_CSS`: white background and default black text on `html` (the note type CSS can set its own colors), the card font list (`font-family: "<card_font>", <fallback fonts>`), `img { max-width: 100%; max-height: <page height>px }`. Thus an image never needs more than one page.
- `[[type:…]]`: remove it from the question, as `remove_brackets` does today. On the answer side, use the `Back` field value as the current code does (`study.rs`).
- Audio and TTS tags (`[sound:…]`, `{{tts …}}`): not played yet, but audio support is planned. Do not throw them away:
  - Call `anki::card_rendering::extract_av_tags(html, question_side, tr)` on each side. It returns the HTML with `[anki:play:q:N]` / `[anki:play:a:N]` placeholders and the list of `AvTag`s (`SoundOrVideo(file)` or `TextToSpeech { field_text, lang, … }`).
  - Replace each placeholder with an empty inline element: `<span class="pbanki-av" data-side="q" data-index="N"></span>`. It draws nothing now.
  - Keep the `AvTag` lists in `CardHtml` and `LoadedCard` (`question_av`, `answer_av`), unused for now.
  - Later (audio support): give `.pbanki-av` a size and an icon in `BASE_CSS`. After layout, read the boxes of these spans (page and position). The study screen puts a Slint touch area on each box, and a tap plays the tag. Auto-play uses the lists in order. Put a short comment at the replacement in code that points to this note.
- Fonts: there are no system fonts. The card fonts are the fallback chain of all common scripts and of the generic families, with the `choose_font` family first (`fonts.md`, "Card renderer fonts"). The `[font]` missing-glyph warning stays (it uses the stripped text). `fonts.sanitize` is not used for HTML: changing characters inside markup could break tags.
- Media (`<img src>`): a `NetProvider` reads files from the collection media folder on the render thread. Before painting, wait until `has_pending_critical_resources()` is false (it reads files synchronously, so this should be immediate). Step 5.
- Image cache (`image_cache.rs`, 2026-11-05): Blitz decodes each image in each document, and it has no hook for decoded images. `MediaProvider` decodes an image once and gives Blitz PAM (uncompressed RGBA, the `pnm` feature of `image`), which Blitz decodes with little more than a copy. Cache key: path, length and modification time; LRU within `ImageCacheSize` (config `image_cache_mb`, default 32 MB). One image can take `image_cache_max_image_mb` (default 16 MB of RGBA, about 4 megapixels). A larger image goes to Blitz unchanged: the size comes from the header before the decode. Converting an image that is not kept only adds copies (PB710, 1600×2400 JPEG: 1226 → 1505 ms with a 6 MB limit). A cache hit costs about 7 ms per document for a 400×600 image on the PB710 (copies at about 330 MB/s). SVG, fonts, and files that do not decode go to Blitz unchanged. The pixels and the size are the ones Blitz would decode itself, so layout does not change.
- `card_shadows = false` (the default, `collection-and-config.md`) adds `box-shadow: none !important` after `OVERRIDE_CSS` (`NO_SHADOW_CSS`). `text-shadow` stays: it is cheap.

### Pagination

Constraints: a page is exactly the card area, `W × H` device px. Content does not continue outside the page. A text line is not cut in half. Blitz has no CSS paged media and no multi-column layout. Thus pbAnki finds the page breaks itself, from the layout tree.

1. **Layout once.** The viewport is `W × H` at `scale` (`Viewport::new(W, H, scale, Light)`). The width sets line wrapping. The height only affects `vh` units. `resolve(0.0)` lays out the full content height, also when it is taller than the viewport.
2. **Collect unbreakable bands** `[top, bottom]` in CSS px, from the document top:
   - Each line box of each `inline_layout_data`: the node's absolute content-box top + `block_offset` + `min(block_min_coord, content_block_min_coord)` to `max(block_max_coord, content_block_max_coord)`. The `content_*` values catch glyphs that overflow a small `line-height`. Inline images, ruby and inline-blocks are in their line box, so they are covered.
   - Each replaced element that is not in an inline layout (a block `<img>`, `<svg>`): its border box.
   - Later, if needed: elements with `break-inside: avoid` (check if Stylo gives it to Blitz).
3. **Choose the breaks** (a pure function, unit tested without Blitz):

   ```
   top = first band top (0 if the first band starts near 0)
   loop:
     limit = top + H/scale
     cut   = min(band.top for bands with band.top < limit < band.bottom), else limit
     if cut <= top: cut = limit        // band taller than a page: hard cut
     page(top, cut)
     top = min(band.top for bands with band.top >= cut)   // skip margins and gaps
     stop when no band is left
   ```

   - `cut` is rounded down to a device pixel. `top` is rounded down too, so a line is never lost between two pages.
   - Table rows: the cells have lines at different heights. The rule takes the highest line that crosses the limit, so no cell line is cut.
   - A gap between bands (paragraph margin) never starts a page. Thus there is no blank page.
4. **Render one page.** `set_viewport_scroll((0, top))`, then fill white, `paint_scene`, rasterize `W × H`, and convert to the page format (see Page storage). Then fill rows `(cut - top) × scale .. H` with white. Without this, the top part of the next line shows at the page bottom.
5. **Short card.** If the content fits on one page, use a negative scroll `-(H/scale - content height) / 2` to center it vertically. This is the same as the current `vertical-alignment: center` (decision 1). Pages of a multi-page card start at the top.
6. **Answer start page.** Anki's answer HTML usually repeats the question (`{{FrontSide}}<hr id=answer>`). Show answer opens the page that contains the top of `get_fragment_target("answer")`, not page 1 (decision 2). Without `#answer`, it opens page 1.

Rejected alternatives:

- Render the whole card into one tall buffer and slice it: 7.9 MB of RGBA per screen (40 MB for the five-screen fixture), and all pages cost time before the first page shows.
- Pages by character count (today): it does not know font, size, images or tables, and it cannot keep HTML formatting.
- CSS `column-width` or `@page`: Blitz does not support them.

### Slint side

- **Card area size.** `main.slint` has `card-area-width` and `card-area-height` (logical) in the place of `chars-per-page`. Rust reads them when a job starts and multiplies by the scale factor (rounded down). `LearnScreen` gives the swipe area the same height.
- **Page display.** `Image { source: page-image(...); width: source.width * 1phx; height: source.height * 1phx; image-rendering: pixelated; }` in the place of the card `Text`. `phx` sizes the image in physical pixels, so it shows at 1:1, with no scaling.
- **Page storage and color.** The page format follows the panel. The frontend gives it to the render thread at start (`PageFormat::Gray8` for a `BB8` framebuffer, `PageFormat::Rgb8` for `RGB24` and the desktop).
  - `Gray8` (black-and-white panels): RGBA → 8-bit luma in one pass, with the same integer Rec. 709 formula as `inkview` (`convert_span` in the bench). About 2 MB per page. Slint has no gray pixel format. Thus the UI thread expands the shown page to a `SharedPixelBuffer<Rgb8Pixel>` (gray in all three channels). Slint converts it back to `BB8` when it draws. Measure the two passes in step 4. If they are slow, store `Rgb8` also here.
  - `Rgb8` (color panels): RGBA → RGB (drop alpha, the frame is opaque). About 3 bytes per pixel. The page is a `SharedPixelBuffer<Rgb8Pixel>` directly, and Slint converts to the `RGB24` framebuffer as it does for its own colors.
  - Do not convert color pages to gray on the render thread. The panel kind decides, not the card.
  - Colors on black-and-white panels: luma only. Light colors (yellow, light blue) become very light gray. A later option: a `BASE_CSS` rule or a contrast step for `Gray8`. Not in this plan.
  - Color panels: the color layer has a lower resolution than the gray layer. The page is rendered at the full panel resolution. The panel firmware does the color filter. `eink-refresh.md` already uses `partial-hq` for color panels.
  - The color path of the backend is not tested on a device (`inkview-backend.md`). Card pages add no new risk there: they go through the same Slint draw.
- **Page indicator.** `[n/m]` moves from the card text to the right side of the `Menu` row (it is empty now). The image fills the card area, so the indicator cannot be inside it.
- **Page counts.** `FirstPages` contains the page counts and break lists of both sides. `total-pages` is correct when the card shows, before the prefetch finishes.
- **Not-yet-rendered page.** `current-page` changes at once. The area keeps the old page until `PageReady` arrives, then `request_redraw()`. The rendering of one page (about 90 ms) is shorter than the reading time of a page, so this is rare. Log it (`[card] page n not ready`).
- **E-ink.** No change. A card change and show answer already mark the whole window dirty (`eink-refresh.md`). A page turn changes the whole card area.

### Errors

- No text fallback (decision 3). `text.rs` pagination, `chars-per-page`, and the font `sanitize` for card text are removed in step 4.
- If the render fails (for example the render thread stopped), the job logs it with the card id. The card area shows a Slint `Text`: "Could not render this card", and the rating buttons stay usable. Thus a bad card does not block the session.
- The "No more cards due!" screen stays a Slint `Text`.

### Cost (PB710, softfp, from the bench)

| Step | Where | Time |
|---|---|---|
| Parse + resolve, two documents | render thread, in the job | 2–14 ms each |
| Question page 1 | render thread, in the job | 83–98 ms |
| Answer page 1 | render thread, in the job | about 90 ms |
| Other pages | render thread, prefetch | about 90–110 ms each |
| Gray → `Rgb8` page image | UI thread | not measured |
| Slint draw of a full-area image | UI thread | not measured (text card renders: 26–51 ms) |

Thus a card job takes about 200 ms more than today. Rating shows its overlay only after 300 ms, so a rating normally shows no overlay. A deck entry can show its loading screen (200 ms delay). The answer page 1 stays in the job (decision 4).

The times are from the PB710 (`Gray8`). `Rgb8` conversion is simpler (no luma), but writes 3 bytes per pixel. Color devices have other SoCs. Measure there when one is available.

Memory: the render thread RGBA buffer (7.9 MB at the PB710 size) + Blitz/Stylo state + the page window (at most about 6 pages: about 12 MB `Gray8`, about 36 MB `Rgb8`). Log `VmHWM` after the first card.

## Steps

1. **Pagination spike in the bench.** Not done as a separate step. The host tests of step 3 cover it (no cut lines, units checked against the painted ink at scale 2). The per-page time on the PB710 comes from the `[card] render` and `[card] page` lines in step 6.
2. **Dependencies.** Done. Blitz at the bench pin (`d0f48bd`, with the `svg` features), plus `image` with png/jpeg/gif/webp/bmp (blitz-dom enables no decoders). The lock resolve raised six crates to the minimum Blitz needs: bytemuck 1.25.2, png 0.18.1, rand 0.8.8, smallvec 1.16.4, thiserror(-impl) 2.0.21. `pbanki.bin`: 24 → 32 MB (PB710), 33 MB (RK3566). Release build about 2.5 min.
3. **`card_render` module.** Done. 24 tests (`cargo test -p pbanki-core card_render`). The Blitz tests need a host font (Noto Sans, DejaVu Sans or Liberation Sans) and pass with a notice without one.
4. **Study wiring.** Done. Lint rule `pbanki-show-new-card`.
5. **Media.** Done (`MediaProvider`, synchronous; files only inside the media folder).
6. **PB710 check.** Open. The user deploys and shares the log. Walk the checklist below. Record the times in `performance.md`.

## Implementation notes

Where the code differs from the design above, and what the desktop run showed.

- **Pages reach Slint through a pure callback.** `page-image(answer, page, version)` is a `pure callback` that `LearnScreen` binds to the image. A page turn re-evaluates it in the same frame. `page-version` changes only when a page that was waiting arrives (`card_pages::deliver`).
- **Page window.** The UI keeps the shown page and its neighbors, and question pages 0–1 and the answer start page with its neighbors: at most 9 pages (about 2 MB each `Gray8` at the PB710 size, 3× that for `Rgb8`). Pages outside it are dropped and asked again when needed.
- **Page state reset.** `show_new_card` sets `show-answer = false` and `study-page = 0` before `current-card`. A Slint `changed card` handler ran too late: the first frame of the new card would bind to the old page state.
- **No `catch_unwind`.** The release profile aborts on panic, so `catch_unwind` would not help on the device. A dead render thread makes later cards show "Could not render this card: the card renderer stopped".
- **Relative media folders.** The desktop config has a relative collection path. `Url::from_directory_path` needs an absolute path; without a base URL, Blitz panicked on the first relative `<img src>` (`BaseDocument::resolve_url`). The folder is now made absolute, and the base URL is never empty. Regression test: `relative_media_folder_does_not_panic`.
- **Desktop run (eink-sim, debug build, 1144×1176 at scale 3).** Card render 94–107 ms for a short card, 219 ms for a nine-page answer, 522 ms for the image card (unoptimized PNG decode). Prefetched pages 150–164 ms each. Each step (next card, show answer, page turn) gave one `update partial`; no page was waited for. Slint draw of the full-area image: 20–58 ms. These are debug desktop numbers, not PB710 numbers.
- **Fade-in animations (device, 2026-11-03).** The "Yap" decks (`yap-jpn-eng.apkg`) give `.answer` the animation `yap-in` from `opacity: 0`. At `t = 0` the answer was laid out (the pages existed) but blank. Fixed with the override sheet. Regression test: `fade_in_animations_show_their_end_state`.
- **Stale card after Home (device, 2026-11-03).** `card_pages` cached the last image by side and page only. Home from question page 1, then another deck: the new card asked for the same key and got the old card's image. `PageStore::install` now drops the cache. Regression test: `a_new_card_does_not_show_the_old_card_image`.
- **Furigana on the same row (device, 2026-11-03).** Blitz parses `display: ruby` but has no ruby layout, so `<rt>` was inline text at 50% size next to its base (`分ゎかった`). `BASE_CSS` now makes `<ruby>` an inline-block with room above the base and places `<rt>` there, centered (other CSS forms tried on the host: `inline-flex` column-reverse moved the base below the baseline; `inline-table` dropped the reading). It covers Anki's `{{furigana:}}` output, one base and one reading per `<ruby>`. Regression test: `ruby_text_sits_above_its_base`. Side effect on the Yap word cards: the taller first line moves `#answer` to answer page 2, so Show answer opens the translation and the word with its reading is one page back (decision 2).
- **Slow cards (device, 2026-11-03).** Yap word cards take 0.8–0.9 s on the PB710 (layout 190–340 ms, each page 290–360 ms; plain cards 50 ms per page). That is above the 300 ms rating overlay delay, so each rating gives two updates (overlay, then card). The CSS bench deck (`test-decks.md`, 2026-11-05) showed where the time goes: the WebP poster decode (about 93 ms per document, two documents per card) and the blurred `box-shadow` of the clip panel (about 170 ms per page). Fixes: the image cache and `card_shadows` (above). PB710 with both (2026-11-05): a bench Yap card takes 257 ms against 619, below the overlay delay. With render-ahead, a real Yap session (39 ratings) waited 3–11 ms for each card; tap to card update median 129 ms (`deep-research/card-page-cache.md`, step 4).
- **Seen in the run, not fixed.** The HTML `border` attribute on `<table>` draws no border (CSS borders work). Bold and italic use the single registered face per family (`fonts.md`).

## Log lines

All at info level, prefix `[card]`. Times in ms.

- `render seq=N WxH@scale pages q=… a=… a_start=… layout=… (parse … incl. media F file(s) read … decode … (C cached), resolve …, breaks …) q=… (record …, raster …, convert …) a=… (…) total=…`: one per card job. `layout` covers both documents. `parse` includes the media: the provider answers synchronously, and `decode` is the image decodes of the provider (cache misses) plus `NetHandler::bytes`, where Blitz decodes the bytes it gets. `C cached` is the files that came from the image cache (logs before 2026-11-05 do not have it). `record` is `paint_scene`, `raster` is vello_cpu (blurs of `box-shadow` are here), `convert` is RGBA to the page format.
  Since render-ahead, the line ends with `wait=… ms` (the time of the `Render` call; `total` is the work, also when it ran ahead) and, for a lookahead card, `ahead=layout+q+a` (the parts done before the `Render`; `-` for none).
- `ahead unused: layout+q+a done in … ms`: a lookahead card was dropped after work was done on it (wasted work: the prediction missed, or a deck entry).
- `page seq=N Side n in … ms (record …, raster …, convert …)`: a prefetched or wanted page.
- `lookahead hit pos=P card=… kind=…` / `lookahead miss predicted=[…] card=… kind=…`: after a rating, whether the new card was among the two cards that followed the previous card in the queue (`LoadedCard::predicted_next`, `get_queued_cards(3)`). It measures the hit rate of a render-ahead cache before one exists. Deck entries only record the prediction.
- `page N of the Side is not ready yet`: a page turn reached a page that was not rendered. The old page stays until it arrives.

Desktop example (eink-sim, debug build, Yap word card): `layout=186 ms (parse 174 incl. media 2 file(s) read 0 decode 169, …) q=264 ms (record 48, raster 209, convert 7)`. The two decodes are the poster in the question and the answer document; the raster is mostly the shadow of the clip panel (a page without it: raster 26–39 ms).

## PB710 checklist (step 6)

1. Deck entry: the card shows with one present. Note `[card] render` times.
2. Show answer: one update. It opens the `#answer` page.
3. A five-page card: swipe and keys page forward and back, one update each. No line is cut at a page edge. No extra touch is necessary, also when a page was not ready.
4. Rating: no overlay for a normal card. The next card shows without an extra touch.
5. CJK card, ruby, table, image card: correct fonts, no missing glyphs that the text path showed.
6. A colored card: readable gray (no color device yet; check color on the desktop eink-sim and on a color device when one is available).
7. `VmHWM` after 50 cards: no growth from card to card.

## Decisions (2026-11-03)

| # | Decision | Choice |
|---|---|---|
| 1 | A short card centered vertically or at the top (as in Anki) | Centered, as pbAnki does now |
| 2 | Show answer opens page 1 or the page with `#answer` | The page with `#answer` |
| 3 | Keep the text path | No. Remove it (git has it). A render error shows an error text. |
| 4 | Answer page 1 in the job or in the prefetch | In the job. Show answer never waits. |
| 5 | Panels | Black-and-white (`Gray8` pages) and color (`Rgb8` pages) |
| 6 | Audio | Not in this plan. Keep the AV tags as placeholders and lists for a later audio step. |

## Risks

| Risk | Mitigation |
|---|---|
| Card templates that use JS (MathJax, scripted cloze, random order) | Show the source. LaTeX images that Anki desktop generated are normal media files and work. Later: convert MathJax on the worker. List the affected note types from the logs. |
| Blitz CSS gaps or a panic on unusual CSS | A render error shows the error text. A panic aborts the app (release `panic = "abort"`). See Implementation notes. |
| Card CSS for screens (dark backgrounds, `.nightMode`) | `BASE_CSS` sets a white background. No night mode class. |
| Line metrics in the wrong units or missing for some boxes (inline-block, flex items) | Step 1 checks the units and the fixtures before the app code |
| Binary size and build time (Stylo, html5ever, ICU) | Step 2 measures it. Feature-gate Blitz if the size is a problem. |
| Slint draw cost of a full-area image | Step 4 measures it. If slow: draw the page into a smaller area, or ask for a direct blit in the backend. |
| Blitz API churn (`0.3.0-beta`) | One pin for the bench and core. Update both together. |
