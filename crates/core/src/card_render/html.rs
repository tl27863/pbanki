//! The HTML document for one card side: Anki's rendered HTML wrapped with the
//! note type CSS, with type-in and audio tags replaced.

use std::fmt::Write as _;

/// Default style under the note type CSS. The note type CSS can override all
/// of it (colors included: color panels show them).
///
/// Ruby: Blitz parses `display: ruby` but has no ruby layout, so `<rt>` was
/// inline text on the same row as its base. Here the `<ruby>` is an
/// inline-block with room above the base (`padding-top`), and the `<rt>` is
/// placed in that room, centered over the base. The base stays on the line's
/// baseline, and the line grows to fit the reading. It covers Anki's
/// `{{furigana:}}` output (one base and one reading per `<ruby>`); a `<ruby>`
/// with several pairs would stack its readings at the same place.
const BASE_CSS: &str = "\
html { background-color: white; color: black; }
img { max-width: 100%; }
ruby { display: inline-block; position: relative; padding-top: 0.6em; text-indent: 0; }
rb { display: inline; }
rt { position: absolute; top: 0; left: -1em; right: -1em; text-align: center; font-size: 50%; line-height: 1; white-space: nowrap; }
rp { display: none; }
";

/// Last in the cascade (`!important` also beats `<style>` blocks inside the
/// card templates). A page is one still frame: documents are resolved at
/// `t = 0`, which is the *first* frame of a CSS animation. A fade-in such as
/// `@keyframes x { from { opacity: 0 } }` then leaves the content laid out but
/// invisible (the "Yap" decks: the whole answer). Without animations and
/// transitions every element shows its end state.
const OVERRIDE_CSS: &str = "\
*, *::before, *::after { animation: none !important; transition: none !important; }
";

/// With `card_shadows = false` in the config. vello_cpu blurs a shadow on
/// every page that shows it: about 170 ms per page for the Yap clip panel on
/// the PB710, more for larger blurs (notes/test-decks.md). `text-shadow` stays:
/// it costs a few ms.
const NO_SHADOW_CSS: &str = "\
*, *::before, *::after { box-shadow: none !important; }
";

/// Builds the document for one side.
///
/// `page_height_css` limits images to one page, so an image never needs a
/// hard cut across two pages. `shadows` false removes all box shadows.
pub fn card_document(
    body: &str,
    note_css: &str,
    ord: u16,
    page_height_css: f32,
    shadows: bool,
) -> String {
    let mut out = String::with_capacity(body.len() + note_css.len() + 512);
    out.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>");
    out.push_str(BASE_CSS);
    let _ = writeln!(out, "img {{ max-height: {page_height_css:.0}px; }}");
    out.push_str("</style><style>");
    out.push_str(note_css);
    out.push_str("</style><style>");
    out.push_str(OVERRIDE_CSS);
    if !shadows {
        out.push_str(NO_SHADOW_CSS);
    }
    let _ = write!(
        out,
        "</style></head><body class=\"card card{}\">",
        u32::from(ord) + 1
    );
    out.push_str(body);
    out.push_str("</body></html>");
    out
}

/// Replaces the `[anki:play:q:N]` / `[anki:play:a:N]` placeholders that
/// `anki::card_rendering::extract_av_tags` leaves in the HTML.
///
/// Audio is not played yet, but it is planned (notes/blitz-card-render.md,
/// "Card HTML"). Each placeholder becomes an empty inline element that keeps
/// its position in the layout: a later audio step gives `.pbanki-av` a size and
/// an icon, reads the boxes after layout, and maps a tap on one to the `AvTag`
/// with the same side and index (kept in `LoadedCard`).
pub fn replace_av_placeholders(html: &str) -> String {
    const PREFIX: &str = "[anki:play:";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(PREFIX) {
        let after = &rest[start + PREFIX.len()..];
        let parsed = after.find(']').and_then(|end| {
            let (side, index) = after[..end].split_once(':')?;
            let valid = matches!(side, "q" | "a") && index.bytes().all(|b| b.is_ascii_digit());
            (valid && !index.is_empty()).then_some((side, index, end))
        });
        out.push_str(&rest[..start]);
        match parsed {
            Some((side, index, end)) => {
                let _ = write!(
                    out,
                    "<span class=\"pbanki-av\" data-side=\"{side}\" data-index=\"{index}\"></span>"
                );
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(PREFIX);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Replaces each `[[type:Field]]` tag. Anki desktop shows an input box on the
/// question and the comparison on the answer. pbAnki has no input, so the
/// question drops the tag and the answer shows the field value.
///
/// `field_value` gets the field name (without `cloze:` or `nc:` prefixes) and
/// returns its HTML. Cloze type-in fields return `None`: the cloze answer is
/// already in the card text.
pub fn replace_type_tags(html: &str, field_value: impl Fn(&str) -> Option<String>) -> String {
    const PREFIX: &str = "[[type:";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(PREFIX) {
        let after = &rest[start + PREFIX.len()..];
        out.push_str(&rest[..start]);
        let Some(end) = after.find("]]") else {
            out.push_str(PREFIX);
            rest = after;
            continue;
        };
        let spec = &after[..end];
        let is_cloze = spec.starts_with("cloze:") || spec.contains(":cloze:");
        let name = spec.rsplit(':').next().unwrap_or(spec);
        if !is_cloze && let Some(value) = field_value(name) {
            out.push_str("<span class=\"pbanki-typeans\">");
            out.push_str(&value);
            out.push_str("</span>");
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn av_placeholders_become_spans() {
        let html = "a[anki:play:q:0]b[anki:play:a:12]c";
        assert_eq!(
            replace_av_placeholders(html),
            "a<span class=\"pbanki-av\" data-side=\"q\" data-index=\"0\"></span>b\
             <span class=\"pbanki-av\" data-side=\"a\" data-index=\"12\"></span>c"
        );
    }

    #[test]
    fn malformed_av_placeholders_stay() {
        for html in [
            "[anki:play:x:0]",
            "[anki:play:q:]",
            "[anki:play:q:1",
            "[anki:play:q:1a]",
        ] {
            assert_eq!(replace_av_placeholders(html), html);
        }
    }

    #[test]
    fn type_tags() {
        let lookup = |name: &str| (name == "Back").then(|| "<b>yes</b>".to_owned());
        assert_eq!(replace_type_tags("Q [[type:Back]]!", |_| None), "Q !");
        assert_eq!(
            replace_type_tags("A [[type:Back]]", lookup),
            "A <span class=\"pbanki-typeans\"><b>yes</b></span>"
        );
        assert_eq!(
            replace_type_tags("[[type:nc:Back]]", lookup),
            "<span class=\"pbanki-typeans\"><b>yes</b></span>"
        );
        assert_eq!(replace_type_tags("x[[type:cloze:Text]]y", lookup), "xy");
        assert_eq!(replace_type_tags("x[[type:Back", lookup), "x[[type:Back");
    }

    #[test]
    fn document_has_card_class_and_css() {
        let doc = card_document("<p>hi</p>", ".card { color: red }", 1, 500.0, true);
        assert!(doc.contains("<body class=\"card card2\"><p>hi</p></body>"));
        assert!(doc.contains(".card { color: red }"));
        assert!(doc.contains("max-height: 500px"));
        // The note type CSS comes after the base CSS, so it wins; the
        // animation override comes last.
        assert!(doc.find("background-color: white").unwrap() < doc.find("color: red").unwrap());
        assert!(doc.find("color: red").unwrap() < doc.find("animation: none").unwrap());
        assert!(!doc.contains("box-shadow"));
    }

    #[test]
    fn shadows_off_comes_after_the_note_css() {
        let css = ".panel { box-shadow: 0 6px 24px rgba(0,0,0,.18) !important }";
        let doc = card_document("<p>hi</p>", css, 0, 500.0, false);
        assert!(doc.find(css).unwrap() < doc.find("box-shadow: none !important").unwrap());
    }
}
