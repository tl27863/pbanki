#!/usr/bin/env python3
"""Tables of the `[card] render` and `[card] page` log lines, by card.

Reads a PB710 session log (`pbanki/logs/`) or the output of the host
harness (`crates/core/examples/render_deck.rs`), and prints one Markdown
row per card variant with the median of its renders. See
notes/test-decks.md.

    python3 -I scripts/test-decks/analyze-render-log.py LOG \
        [--manifest target/test-decks/pbanki-css-bench.manifest.json]

How a render line gets its card:
- host: the `[bench] card=… variant=…` line before it;
- device: the `[card] lookahead … card=…` line after it (same job);
- the first card after a deck entry has no lookahead line: it is the card
  before the next known card in manifest order (marked `*`).
"""

import argparse
import json
import re
import statistics
import sys
from dataclasses import dataclass, field
from pathlib import Path

N = r"([\d.]+)"  # milliseconds: integers in older logs, one decimal since 2026-11-03
PAGE_TIMES = rf"{N} ms \(record {N}, raster {N}, convert {N}\)"
RENDER = re.compile(
    r"\[card\] render seq=(\d+) (\d+)x(\d+)@(\S+) pages q=(\d+) a=(\d+) a_start=(\d+) "
    rf"layout={N} ms \(parse {N} incl\. media (\d+) file\(s\) read {N} decode {N}"
    r"(?: \((\d+) cached\))?, "  # since the image cache (2026-11-05)
    rf"resolve {N}, breaks {N}\) q=" + PAGE_TIMES + " a=" + PAGE_TIMES + rf" total={N} ms"
    rf"(?: wait={N} ms)?(?: ahead=(\S+))?"  # since render-ahead (2026-11-05)
)
PAGE = re.compile(r"\[card\] page seq=(\d+) (Question|Answer) (\d+) in " + PAGE_TIMES)
LOOKAHEAD = re.compile(r"\[card\] lookahead (?:hit|miss).* card=(-?\d+)")
BENCH = re.compile(r"\[bench\] card=(\d+) variant=(\S+)")
START = re.compile(r"starting pbAnki|\[bench\] \d+ card\(s\)")
DEFAULT_MANIFEST = Path("target/test-decks/pbanki-css-bench.manifest.json")


@dataclass
class Render:
    seq: int
    run: int  # app run (or harness run) in the log
    card: int | None
    inferred: bool
    pages_q: int
    pages_a: int
    a_start: int
    layout: int
    parse: int
    files: int
    read: int
    decode: int
    cached: int  # images from the image cache (0 in older logs)
    resolve: int
    q: tuple  # (total, record, raster, convert)
    a: tuple
    total: int
    wait: float | None  # time the job waited (None in older logs)
    ahead: str | None  # parts rendered ahead (`layout+q+a`), None for a miss
    extra: list = field(default_factory=list)  # other pages: (total, record, raster, convert)


def parse(lines):
    renders = []
    pending_bench = None
    app_run = 0
    by_seq = {}
    for line in lines:
        if START.search(line):
            app_run += 1
            by_seq = {}
        if m := BENCH.search(line):
            pending_bench = int(m.group(1))
            continue
        if m := RENDER.search(line):
            groups = m.groups()
            g = [x if i == 3 else float(x or 0) for i, x in enumerate(groups[:24])]
            r = Render(
                seq=int(g[0]), run=app_run, card=pending_bench, inferred=False,
                pages_q=int(g[4]), pages_a=int(g[5]), a_start=int(g[6]), layout=g[7], parse=g[8],
                files=int(g[9]), read=g[10], decode=g[11], cached=int(g[12]), resolve=g[13],
                q=tuple(g[15:19]), a=tuple(g[19:23]), total=g[23],
                wait=float(groups[24]) if groups[24] is not None else None,
                ahead=groups[25],
            )
            pending_bench = None
            renders.append(r)
            by_seq[r.seq] = r
            continue
        if m := LOOKAHEAD.search(line):
            if renders and renders[-1].card is None and renders[-1].run == app_run:
                renders[-1].card = int(m.group(1))
            continue
        if m := PAGE.search(line):
            r = by_seq.get(int(m.group(1)))
            if r is not None:
                r.extra.append(tuple(float(x) for x in m.groups()[3:7]))
    return renders


def fmt_wait(rs):
    """Median time the job waited; `*` when the first render was a
    lookahead hit (its work was done while the previous card showed)."""
    waits = [r.wait for r in rs if r.wait is not None]
    if not waits:
        return "—"
    return ms(med(waits)) + (" *" if rs[0].ahead else "")


def img(rs):
    """Files of the first render, with its cache hits when there are any."""
    first = rs[0]
    return f"{first.files} ({first.cached})" if first.cached else str(first.files)


def infer_first_cards(renders, order):
    """A render without a card, followed by a known card at manifest index
    k > 0, is the card at k - 1 (the first card of a deck entry)."""
    index = {card: i for i, card in enumerate(order)}
    for prev, nxt in zip(renders, renders[1:]):
        if prev.card is None and nxt.card in index and index[nxt.card] > 0:
            prev.card = order[index[nxt.card] - 1]
            prev.inferred = True


def med(values):
    values = list(values)
    return statistics.median(values) if values else None


def ms(value):
    """Compact: one decimal below 10 ms, else whole ms."""
    if value is None:
        return "—"
    return f"{value:.1f}" if abs(value) < 10 else f"{value:.0f}"


def fmt_page(pages):
    if not pages:
        return "—"
    parts = [ms(med(p[i] for p in pages)) for i in range(4)]
    return f"{parts[0]} ({parts[1]}/{parts[2]}/{parts[3]})"


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("log", type=Path)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--reference", default="01-plain", help="variant for the Δ column")
    parser.add_argument("--skip-first", action="store_true",
                        help="drop the first render of each card (host: the cold run)")
    args = parser.parse_args()

    manifest = []
    if args.manifest.exists() and args.manifest.stat().st_size:  # `--manifest /dev/null`: none
        manifest = json.loads(args.manifest.read_text())["cards"]
    names = {c["id"]: c["variant"] for c in manifest}
    groups = {c["id"]: c["group"] for c in manifest}
    descs = {c["id"]: c["desc"] for c in manifest}
    order = [c["id"] for c in manifest]

    renders = parse(args.log.read_text(errors="replace").splitlines())
    infer_first_cards(renders, order)
    if not renders:
        sys.exit(f"no [card] render lines in {args.log}")

    by_card = {}
    for r in renders:
        by_card.setdefault(r.card, []).append(r)
    if args.skip_first:
        by_card = {k: v[1:] or v for k, v in by_card.items()}

    def key(card):
        return (order.index(card) if card in names else len(order), str(card))

    ref_total = None
    for card, rs in by_card.items():
        if names.get(card) == args.reference:
            ref_total = med(r.total for r in rs)

    print(f"{args.log.name}: {len(renders)} renders, {len(by_card)} cards. Times in ms, median.")
    print("Pages: total (record/raster/convert). img: files (cache hits, first render). "
          "total: the render work; wait: what the card job waited (`*`: rendered ahead). "
          "`*`: card inferred from manifest order.\n")
    print("| Variant | n | pages q/a | layout | parse | img | decode | resolve | q page | a page "
          "| other pages | total | Δ total | wait |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    group = None
    for card in sorted(by_card, key=key):
        rs = by_card[card]
        name = names.get(card, f"card {card}" if card is not None else "(unknown)")
        if any(r.inferred for r in rs):
            name += " *"
        if groups.get(card) != group:
            group = groups.get(card)
            if group:
                print(f"| **{group}** ||||||||||||||")
        total = med(r.total for r in rs)
        delta = "" if ref_total is None else ("+" if total >= ref_total else "") + ms(total - ref_total)
        extra = [p for r in rs for p in r.extra]
        print(
            f"| {name} | {len(rs)} | {rs[0].pages_q}/{rs[0].pages_a} | {ms(med(r.layout for r in rs))} "
            f"| {ms(med(r.parse for r in rs))} | {img(rs)} | {ms(med(r.decode for r in rs))} "
            f"| {ms(med(r.resolve for r in rs))} | {fmt_page([r.q for r in rs])} "
            f"| {fmt_page([r.a for r in rs])} | {fmt_page(extra)} | {ms(total)} | {delta} | {fmt_wait(rs)} |"
        )
    if descs:
        print("\nVariants:\n")
        for c in manifest:
            if c["id"] in by_card:
                print(f"- `{c['variant']}`: {c['desc']}")


if __name__ == "__main__":
    main()
