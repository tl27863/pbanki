#!/usr/bin/env python3
"""Builds the CSS bench deck: one card per CSS or media feature.

Each card isolates one feature on the same small card (shadows, gradients,
clips, images in several formats, long text, ...), so the `[card] render`
log lines show what that feature costs on the device. See
notes/test-decks.md.

Needs the Anki Python package (Anki desktop 23.10+) and Pillow. It builds a
new collection in a temporary folder; the user's Anki profile is not used.

    python3 -I scripts/test-decks/make-css-bench.py [--out target/test-decks]

Writes `pbanki-css-bench.apkg` and `pbanki-css-bench.manifest.json` (card
id -> variant, for `analyze-render-log.py`).
"""

import argparse
import io
import json
import random
import shutil
import sys
import tempfile
from pathlib import Path

from anki.collection import (
    Collection,
    DeckIdLimit,
    ExportAnkiPackageOptions,
)
from PIL import Image, ImageDraw, ImageFilter

DECK = "pbAnki CSS bench"
NOTETYPE = "pbAnki CSS bench"
# Anki exports a `_` media file only when a note type references it, so the
# web font variant has a note type of its own.
NOTETYPE_WEBFONT = "pbAnki CSS bench (web font)"
WEBFONT_CSS = '@font-face { font-family: "BenchFont"; src: url("_bench-font.ttf"); }\n'
PRESET = "pbAnki CSS bench"
FIELDS = ["Variant", "Style", "Front", "Back"]

# The note type CSS: Anki's default card CSS, plus the label at the top.
NOTETYPE_CSS = """\
.card { font-family: arial; font-size: 20px; text-align: center; color: black; background-color: white; }
.bench-label { font-family: monospace; font-size: 12px; color: #555; }
"""

# The variant CSS is in a field, inside <style> in the body, as Yap's
# templates do. One note type then covers all variants.
QFMT = '<style>{{Style}}</style><div class="bench-label">{{Variant}}</div>\n{{Front}}'
AFMT = '{{FrontSide}}\n<hr id="answer">\n{{Back}}'

WORD = '<div class="word">とんでもない</div>'
SENTENCE = '<div class="sentence">彼の行動はとんでもない。 His behavior is outrageous.</div>'
BACK = '<div class="meaning">outrageous, absurd, unbelievable</div>'

# A 16:9-ish panel near the size of Yap's clip panel (`.scene`). It is in
# the front, so the question page and the answer start page both show it.
PANEL_CSS = """\
.word { font-size: 32px; }
.panel { height: 150px; margin: 12px 0; border-radius: 14px; background: #000; color: #fff;
  display: flex; align-items: center; justify-content: center; }
"""
PANEL = '<div class="panel"><span>彼の行動はとんでもない。</span></div>'

THUMB_CSS = """\
.source { display: flex; align-items: center; gap: 8px; justify-content: center; font-size: 13px; color: #555; }
.source img { display: block; height: 32px; border-radius: 4px; }
"""


def thumb(src: str, times: int = 1) -> str:
    imgs = "".join(f'<img src="{src}" alt="">' for _ in range(times))
    return f'<div class="source">{imgs}<span>東京物語 · 1953</span></div>'


LONG_TEXT = "".join(
    f"<p>Paragraph {i}. The Peace of Westphalia ended the Thirty Years' War and is "
    "often cited as the origin of the modern system of sovereign states. "
    "ウェストファリア条約は三十年戦争を終わらせた。</p>"
    for i in range(1, 11)
)

RUBY = "".join(
    f"<ruby>{kanji}<rt>{kana}</rt></ruby>"
    for kanji, kana in [
        ("彼", "かれ"), ("行動", "こうどう"), ("毎日", "まいにち"), ("日本語", "にほんご"),
        ("勉強", "べんきょう"), ("東京", "とうきょう"), ("物語", "ものがたり"), ("映画", "えいが"),
    ]
    * 4
)

TABLE = (
    "<table><tr><th>Form</th><th>Reading</th><th>Meaning</th><th>JLPT</th></tr>"
    + "".join(
        f"<tr><td>語{i}</td><td>ご{i}</td><td>word {i}</td><td>N{1 + i % 5}</td></tr>"
        for i in range(10)
    )
    + "</table>"
)

SPANS = " ".join(
    f'<span class="s{i % 5}">word{i}</span>' for i in range(200)
)

GRID = (
    '<ul class="glosses">'
    + "".join(
        f'<li><a class="term">語{i}</a><span class="gloss">gloss {i}</span></li>' for i in range(8)
    )
    + "</ul>"
    + '<ol class="senses">'
    + "".join(
        f'<li><div class="meaning">sense {i}</div><div class="example"><span lang="ja">'
        f"例文{i}です。</span><span class=\"example-native\">Example {i}.</span></div></li>"
        for i in range(2)
    )
    + "</ol>"
)

INLINE_SVG = (
    '<svg viewBox="0 0 300 120" width="300" height="120">'
    + "".join(
        f'<circle cx="{10 + i * 12}" cy="{60 + 40 * ((i * 37) % 7 - 3) / 3:.0f}" r="4" fill="#333"/>'
        for i in range(24)
    )
    + '<polyline fill="none" stroke="#000" stroke-width="2" points="'
    + " ".join(f"{10 + i * 12},{60 + 40 * ((i * 37) % 7 - 3) / 3:.0f}" for i in range(24))
    + '"/><path d="M0 0l12 7-12 7z" transform="translate(140 50) scale(2)"/></svg>'
)

CHIPS = " ".join(f'<span class="chip">tag {i}</span>' for i in range(8))
MANY_CHIPS = " ".join(f'<span class="chip">tag {i}</span>' for i in range(30))

# A Yap word card, copied from the "Yap jpn-eng words" note type (script,
# audio and the remote video removed). `yap-*` variants take parts away.
YAP_CSS = """\
.card {
  --yap-fg: #1c1b19; --yap-muted: #75716a; --yap-line: rgba(0,0,0,.12); --yap-chip: rgba(0,0,0,.05);
  --yap-accent: #d9480f; --yap-accent-soft: rgba(217,72,15,.1);
  font-family: system-ui, -apple-system, "Segoe UI", sans-serif; text-align: center; line-height: 1.45;
  padding: 24px 16px; color: var(--yap-fg);
}
.yap { max-width: 520px; margin: 0 auto; display: flex; flex-direction: column; align-items: center; gap: 16px; }
.sentence { font-size: 28px; font-weight: 600; line-height: 1.25; letter-spacing: -.01em; }
.word { font-size: 36px; }
.yap #answer { width: 100%; height: 0; margin: -8px 0; border: 0; background: none; }
.answer { display: flex; flex-direction: column; align-items: center; gap: 12px; width: 100%; }
.card ol.senses { display: flex; flex-direction: column; gap: 14px; width: 100%; max-width: 420px; margin: 0; padding: 0; list-style: none; }
.senses .meaning { font-size: 20px; }
.senses .example { display: flex; flex-direction: column; gap: 2px; margin-top: 8px; padding: 8px 12px;
  border-radius: 10px; background: var(--yap-chip); font-size: 15px; }
.senses .example b { font-weight: inherit; color: var(--yap-accent); }
.senses .example-native { font-size: 14px; color: var(--yap-muted); }
.scene {
  position: relative; display: block; width: 100%; aspect-ratio: 16 / 9; padding: 0; border: 0; border-radius: 14px; overflow: hidden;
  background: #000; cursor: pointer; font: inherit; box-shadow: 0 6px 24px rgba(0,0,0,.18);
}
.subs { position: absolute; left: 8px; right: 8px; bottom: 10px; display: flex; flex-direction: column; align-items: center; gap: 3px; }
.subs span { font-size: 15px; line-height: 1.3; padding: 2px 8px; border-radius: 6px; background: rgba(0,0,0,.65); color: rgba(255,255,255,.75); }
.subs .sentence { color: #ffd166; font-weight: 600; letter-spacing: 0; }
.play { position: absolute; top: 10px; right: 10px; width: 34px; height: 34px; border-radius: 50%;
  background: rgba(0,0,0,.5); display: grid; place-items: center; }
.play svg { width: 12px; height: 14px; margin-left: 2px; fill: #fff; }
.source { display: flex; align-items: center; gap: 8px; align-self: flex-start; padding: 4px 12px 4px 4px;
  border-radius: 8px; background: var(--yap-chip); color: var(--yap-muted); font-size: 13px; }
.source img { display: block; height: 32px; border-radius: 4px; }
"""
YAP_DEFINITION = (
    '<ol class="senses"><li><div class="meaning">outrageous, absurd, unbelievable</div>'
    '<div class="example"><span lang="ja">彼の行動は<b>とんでもない</b>。</span>'
    '<span class="example-native">His behavior is outrageous.</span></div></li>'
    '<li><div class="meaning">not at all, far from it</div><div class="example">'
    '<span lang="ja">そんなことは<b>とんでもない</b>。</span>'
    '<span class="example-native">That’s not true at all.</span></div></li></ol>'
)


def yap_front(scene: bool = True, poster: str | None = "bench-poster.webp") -> str:
    parts = ['<div class="yap"><div class="sentence word prompt"><span lang="ja">'
             '<ruby>とんでもない<rt></rt></ruby></span></div>']
    if scene:
        parts.append(
            '<button type="button" class="scene" style="aspect-ratio: 1.7777"><video preload="metadata">'
            '</video><div class="subs"><span class="sentence">とんでもない</span></div>'
            '<span class="play"><svg viewBox="0 0 12 14"><path d="M0 0l12 7-12 7z"/></svg></span></button>'
        )
    if poster:
        parts.append(f'<div class="source"><img src="{poster}" alt=""><span>東京物語 · 1953</span></div>')
    return "".join(parts)


def yap_back() -> str:
    # The Yap template closes `.yap` after the answer; here the answer side
    # is {{FrontSide}}<hr>{{Back}}, so the back closes it.
    return f'<div class="answer">{YAP_DEFINITION}</div></div>'


def variants() -> list[dict]:
    """(group, name, description, style, front, back), in study order."""
    v = []

    def add(group, name, desc, style="", front=WORD + SENTENCE, back=BACK, notetype=NOTETYPE):
        v.append(dict(group=group, name=name, desc=desc, style=style, front=front, back=back,
                      notetype=notetype))

    plain_front = WORD + SENTENCE
    add("base", "00-warmup", "First card: absorbs the cold start (fonts, Stylo). Ignore its numbers.")
    add("base", "01-plain", "Anki default CSS, short text. The reference card.")
    add("base", "02-plain-again", "Same as 01, other order of text: run-to-run noise.",
        front=SENTENCE + WORD)

    def panel(name, desc, extra):
        add("shadow", name, desc, PANEL_CSS + extra, WORD + PANEL, BACK)

    panel("10-panel-flat", "Black rounded panel, no shadow (control for 11-17).", "")
    panel("11-shadow-yap", "Yap's clip shadow: 0 6px 24px rgba(0,0,0,.18).",
          ".panel { box-shadow: 0 6px 24px rgba(0,0,0,.18); }")
    panel("12-shadow-blur0", "Hard shadow, no blur: 6px 6px 0 #888.",
          ".panel { box-shadow: 6px 6px 0 #888; }")
    panel("13-shadow-blur4", "Small blur: 0 2px 4px rgba(0,0,0,.3).",
          ".panel { box-shadow: 0 2px 4px rgba(0,0,0,.3); }")
    panel("14-shadow-blur64", "Large blur: 0 8px 64px rgba(0,0,0,.3).",
          ".panel { box-shadow: 0 8px 64px rgba(0,0,0,.3); }")
    panel("15-shadow-inset", "Inset shadow: inset 0 2px 12px rgba(0,0,0,.5) on a gray panel.",
          ".panel { background: #ccc; color: #000; box-shadow: inset 0 2px 12px rgba(0,0,0,.5); }")
    panel("16-shadow-multi", "Three stacked shadows on the panel.",
          ".panel { box-shadow: 0 1px 2px rgba(0,0,0,.2), 0 4px 8px rgba(0,0,0,.2), 0 12px 32px rgba(0,0,0,.2); }")
    add("shadow", "17-shadow-chips", "Eight small chips with 0 2px 6px shadows (no panel).",
        ".chip { display: inline-block; margin: 6px; padding: 4px 10px; border-radius: 99px; "
        "background: #eee; box-shadow: 0 2px 6px rgba(0,0,0,.25); font-size: 16px; }",
        WORD + f"<div>{CHIPS}</div>", BACK)
    add("shadow", "18-text-shadow", "text-shadow 0 1px 3px on all text.",
        ".card { text-shadow: 0 1px 3px rgba(0,0,0,.5); }", WORD + SENTENCE + SENTENCE, BACK)

    def paint(name, desc, extra, front=WORD + PANEL):
        add("paint", name, desc, PANEL_CSS + extra, front, BACK)

    paint("20-border-radius", "Panel with a 3px border and radius, no fill.",
          ".panel { background: none; color: #000; border: 3px solid #333; border-radius: 24px; }")
    paint("21-border-dashed", "Dashed and dotted borders.",
          ".panel { background: none; color: #000; border: 3px dashed #333; } "
          ".word { border-bottom: 3px dotted #333; }")
    paint("22-linear-gradient", "Panel filled with a linear gradient.",
          ".panel { background: linear-gradient(135deg, #111, #ddd); }")
    paint("23-radial-gradient-page", "Whole page (body) filled with a radial gradient.",
          ".card { background: radial-gradient(circle at 30% 20%, #fff, #999); } .panel { background: none; color: #000; }")
    paint("24-opacity", "Panel at opacity .6 (a compositing layer).",
          ".panel { opacity: .6; }")
    paint("25-transform", "Panel rotated by 3 degrees.",
          ".panel { transform: rotate(-3deg); }")
    paint("26-overflow-clip", "overflow: hidden with a radius (rounded clip), as Yap's .scene.",
          ".panel { overflow: hidden; } .panel span { padding: 80px; background: #444; }")
    paint("27-filter", "filter: drop-shadow() on the panel.",
          ".panel { filter: drop-shadow(0 4px 8px rgba(0,0,0,.5)); }")
    add("paint", "28-rgba-chips", "30 translucent chips (rgba backgrounds, radius), no shadow.",
        ".chip { display: inline-block; margin: 4px; padding: 4px 10px; border-radius: 10px; "
        "background: rgba(0,0,0,.08); font-size: 15px; }",
        WORD + f"<div>{MANY_CHIPS}</div>", BACK)

    add("text", "30-long-text", "Ten paragraphs on the back: several answer pages.",
        "p { text-align: left; }", WORD, BACK + LONG_TEXT)
    add("text", "31-ruby", "32 ruby words (furigana) in a paragraph.",
        "", WORD + f'<div class="sentence">{RUBY}</div>', BACK)
    add("text", "32-big-font", "One 96px word.",
        ".word { font-size: 96px; font-weight: bold; }", WORD, BACK)
    add("text", "33-table", "11-row table with collapsed borders and striped rows.",
        "table { border-collapse: collapse; margin: 0 auto; font-size: 14px; } "
        "td, th { border: 1px solid #444; padding: 2px 6px; } tr:nth-child(even) { background: #eee; }",
        WORD, TABLE)
    add("text", "34-flex-grid", "Yap glosses grid and senses flex boxes.",
        YAP_CSS.split(".yap {")[0]
        + ".card ul.glosses { display: grid; grid-template-columns: auto auto; justify-content: center; "
        "gap: 4px 14px; margin: 0; padding: 12px 0 0; list-style: none; font-size: 15px; } "
        ".card ul.glosses li { display: contents; } .glosses .term { text-align: right; font-weight: 600; } "
        ".glosses .gloss { text-align: left; color: #75716a; } "
        ".card ol.senses { display: flex; flex-direction: column; gap: 14px; margin: 0; padding: 0; list-style: none; } "
        ".senses .example { display: flex; flex-direction: column; gap: 2px; padding: 8px 12px; border-radius: 10px; background: rgba(0,0,0,.05); font-size: 15px; }",
        WORD + GRID, BACK)
    add("text", "35-many-spans", "200 spans: colors, bold, italic, underline.",
        ".s0 { color: #d9480f; } .s1 { font-weight: bold; } .s2 { font-style: italic; } "
        ".s3 { text-decoration: underline; } .s4 { background: #eee; } .spans { font-size: 14px; }",
        WORD + f'<div class="spans">{SPANS}</div>', BACK)
    add("text", "36-webfont", "@font-face from the media folder (_bench-font.ttf, Noto Sans).",
        '.card { font-family: "BenchFont"; }',
        '<div class="word">outrageous</div><div class="sentence">His behavior is outrageous.</div>', BACK,
        notetype=NOTETYPE_WEBFONT)
    add("text", "37-inline-svg", "Inline SVG: 24 circles, a polyline, a path.",
        "", WORD + INLINE_SVG, BACK)

    def image(name, desc, front, extra=""):
        add("image", name, desc, PANEL_CSS + THUMB_CSS + extra, WORD + front, BACK)

    image("40-thumb-webp", "400x600 WebP shown 32px high (Yap poster).", thumb("bench-poster.webp"))
    image("41-thumb-jpeg", "Same picture as JPEG.", thumb("bench-poster.jpg"))
    image("42-thumb-png", "Same picture as PNG.", thumb("bench-poster.png"))
    image("43-thumb-webp-lossless", "Same picture as lossless WebP.", thumb("bench-poster-lossless.webp"))
    image("44-thumb-bmp", "Same picture as 24-bit BMP (a decoded-image cache would serve this).",
          thumb("bench-poster.bmp"))
    image("45-thumb-gif", "Same picture as GIF.", thumb("bench-poster.gif"))
    image("46-thumb-jpeg-gray", "Same picture as grayscale JPEG.", thumb("bench-poster-gray.jpg"))
    image("47-thumb-jpeg-large", "1600x2400 JPEG shown 32px high.", thumb("bench-photo-large.jpg"))
    image("48-thumb-webp-again", "The WebP of 40 again: is it decoded again on the next card?",
          thumb("bench-poster.webp"))
    image("49-thumb-webp-x3", "The same WebP three times in one document.", thumb("bench-poster.webp", 3))
    image("4a-webp-full", "The WebP shown 150px high (raster of a scaled image).",
          '<img class="big" src="bench-poster.webp" alt="">', ".big { height: 150px; }")
    image("4b-icons-x12", "Twelve 64x64 PNG icons with alpha.",
          '<div>' + "".join(f'<img class="icon" src="bench-icon-{i:02}.png" alt="">' for i in range(12)) + '</div>',
          ".icon { width: 32px; height: 32px; margin: 2px; }")
    image("4c-svg-file", "<img> of an SVG file.",
          '<img class="big" src="bench-diagram.svg" alt="">', ".big { height: 150px; }")
    image("4d-css-background", "The WebP as a CSS background-image of the panel (cover).",
          '<div class="panel bg"></div>',
          '.bg { background: #000 url("bench-poster.webp") center / cover no-repeat; }')

    add("yap", "50-yap", "Yap word card: shadowed clip panel, rgba chips, WebP thumb.",
        YAP_CSS, yap_front(), yap_back())
    add("yap", "51-yap-no-shadow", "50 without the box-shadow (option A2).",
        YAP_CSS + ".scene { box-shadow: none; }", yap_front(), yap_back())
    add("yap", "52-yap-no-poster", "50 without the WebP thumb (option A1 at best).",
        YAP_CSS, yap_front(poster=None), yap_back())
    add("yap", "53-yap-no-shadow-no-poster", "50 without shadow and thumb (A1 + A2 at best).",
        YAP_CSS + ".scene { box-shadow: none; }", yap_front(poster=None), yap_back())
    add("yap", "54-yap-no-clip", "50 without overflow: hidden on the panel.",
        YAP_CSS + ".scene { overflow: visible; }", yap_front(), yap_back())
    add("yap", "55-yap-jpeg-poster", "50 with the JPEG thumb instead of WebP.",
        YAP_CSS, yap_front(poster="bench-poster.jpg"), yap_back())
    add("yap", "56-yap-bmp-poster", "50 with the BMP thumb (decoded-image cache hit, A1).",
        YAP_CSS, yap_front(poster="bench-poster.bmp"), yap_back())
    add("base", "99-plain-last", "Same as 01 at the end: drift over the session.")
    return v


def poster(width: int, height: int, seed: int) -> Image.Image:
    """A photo-like picture: a gradient, soft shapes and grain."""
    rng = random.Random(seed)
    base = Image.linear_gradient("L").resize((width, height)).convert("RGB")
    tint = Image.new("RGB", (width, height), (rng.randrange(256), rng.randrange(256), rng.randrange(256)))
    im = Image.blend(base, tint, 0.5)
    draw = ImageDraw.Draw(im)
    for _ in range(40):
        x, y = rng.randrange(width), rng.randrange(height)
        r = rng.randrange(width // 20, width // 4)
        color = (rng.randrange(256), rng.randrange(256), rng.randrange(256))
        if rng.random() < 0.5:
            draw.ellipse((x - r, y - r, x + r, y + r), fill=color)
        else:
            draw.rectangle((x - r, y - r // 2, x + r, y + r // 2), fill=color)
    im = im.filter(ImageFilter.GaussianBlur(radius=max(2, width // 150)))
    noise = Image.effect_noise((width, height), 40).convert("RGB")
    return Image.blend(im, noise, 0.12)


def encode(im: Image.Image, fmt: str, **kwargs) -> bytes:
    out = io.BytesIO()
    im.save(out, fmt, **kwargs)
    return out.getvalue()


def media_files() -> dict[str, bytes]:
    p = poster(400, 600, 1)
    files = {
        "bench-poster.webp": encode(p, "WEBP", quality=80, method=4),
        "bench-poster-lossless.webp": encode(p, "WEBP", lossless=True),
        "bench-poster.jpg": encode(p, "JPEG", quality=85),
        "bench-poster-gray.jpg": encode(p.convert("L"), "JPEG", quality=85),
        "bench-poster.png": encode(p, "PNG", optimize=True),
        "bench-poster.bmp": encode(p, "BMP"),
        "bench-poster.gif": encode(p.convert("P", palette=Image.Palette.ADAPTIVE), "GIF"),
        "bench-photo-large.jpg": encode(poster(1600, 2400, 2), "JPEG", quality=85),
    }
    rng = random.Random(3)
    for i in range(12):
        icon = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
        draw = ImageDraw.Draw(icon)
        color = (rng.randrange(200), rng.randrange(200), rng.randrange(200), 255)
        draw.ellipse((4, 4, 60, 60), fill=color)
        draw.rectangle((24, 14, 40, 50), fill=(255, 255, 255, 200))
        files[f"bench-icon-{i:02}.png"] = encode(icon, "PNG")
    svg = ['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 300 200">',
           '<rect x="0" y="0" width="300" height="200" fill="#fff" stroke="#000"/>']
    for i in range(20):
        h = 20 + (i * 53) % 150
        svg.append(f'<rect x="{10 + i * 14}" y="{190 - h}" width="10" height="{h}" fill="#444"/>')
    svg.append('<text x="150" y="20" text-anchor="middle" font-size="14">chart</text></svg>')
    files["bench-diagram.svg"] = "".join(svg).encode()
    font = Path("/usr/share/fonts/noto/NotoSans-Regular.ttf")
    if font.exists():
        files["_bench-font.ttf"] = font.read_bytes()
    else:
        print(f"warning: {font} not found; 36-webfont falls back to the card font", file=sys.stderr)
    return files


def build(out_dir: Path) -> None:
    out_dir.mkdir(parents=True, exist_ok=True)
    apkg = out_dir / "pbanki-css-bench.apkg"
    manifest_path = out_dir / "pbanki-css-bench.manifest.json"
    work = Path(tempfile.mkdtemp(prefix="pbanki-css-bench-"))
    try:
        col = Collection(str(work / "collection.anki2"))
        media = media_files()
        for name, data in media.items():
            col.media.write_data(name, data)

        mm = col.models
        notetypes = {}
        for name, css in [(NOTETYPE, NOTETYPE_CSS), (NOTETYPE_WEBFONT, NOTETYPE_CSS + WEBFONT_CSS)]:
            nt = mm.new(name)
            for field_name in FIELDS:
                mm.add_field(nt, mm.new_field(field_name))
            template = mm.new_template("Card 1")
            template["qfmt"] = QFMT
            template["afmt"] = AFMT
            mm.add_template(nt, template)
            nt["css"] = css
            mm.add(nt)
            notetypes[name] = mm.by_name(name)

        deck_id = col.decks.id(DECK)
        # All cards in one session, in order: no daily limit, and no learning
        # steps, so a rated card does not come back.
        conf = col.decks.add_config_returning_id(PRESET)
        config = col.decks.get_config(conf)
        config["new"]["perDay"] = 9999
        config["new"]["delays"] = []
        config["rev"]["perDay"] = 9999
        col.decks.update_config(config)
        deck = col.decks.get(deck_id)
        deck["conf"] = conf
        col.decks.save(deck)

        cards = []
        for variant in variants():
            note = col.new_note(notetypes[variant["notetype"]])
            # A fixed guid: importing a newer build updates the notes.
            note.guid = f"pbanki-css-bench-{variant['name']}"
            note["Variant"] = variant["name"]
            note["Style"] = variant["style"]
            note["Front"] = variant["front"]
            note["Back"] = variant["back"]
            col.add_note(note, deck_id)
            for card in note.cards():
                cards.append({
                    "id": card.id,
                    "variant": variant["name"],
                    "group": variant["group"],
                    "desc": variant["desc"],
                })

        col.export_anki_package(
            out_path=str(apkg),
            options=ExportAnkiPackageOptions(
                with_scheduling=True, with_deck_configs=True, with_media=True, legacy=False
            ),
            limit=DeckIdLimit(deck_id),
        )
        col.close()
        manifest_path.write_text(json.dumps({"deck": DECK, "cards": cards}, indent=1, ensure_ascii=False))
        sizes = {name: len(data) for name, data in media.items()}
        print(f"{apkg} ({apkg.stat().st_size // 1024} KB, {len(cards)} cards)")
        print(f"{manifest_path}")
        for name, size in sorted(sizes.items()):
            if not name.startswith("bench-icon-"):
                print(f"  {name}: {size // 1024} KB")
    finally:
        shutil.rmtree(work, ignore_errors=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--out", type=Path, default=Path("target/test-decks"))
    args = parser.parse_args()
    build(args.out)


if __name__ == "__main__":
    main()
