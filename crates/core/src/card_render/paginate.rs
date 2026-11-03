//! Page breaks from the laid-out document (notes/blitz-card-render.md,
//! "Pagination").
//!
//! Blitz has no paged media and no multi-column layout. Thus the card is laid
//! out once at the page width, and the breaks come from the line boxes: a
//! page ends above the first line that would cross its bottom edge.
//!
//! All values here are device pixels from the top of the document.

use blitz_dom::{BaseDocument, Node};

/// Upper bound for pages of one side. A card this long is broken content.
const MAX_PAGES: usize = 500;

/// Content that must not be cut at a page edge: one line box or one image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub top: f32,
    pub bottom: f32,
}

/// One page: the document is drawn scrolled to `top`, and only the first
/// `keep` rows are kept. The rows below belong to the next page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSpan {
    /// Scroll offset. Negative to center a short card.
    pub top: f32,
    pub keep: u32,
}

/// Splits the bands into pages of `page_height` rows.
///
/// - A card that fits on one page is centered vertically.
/// - A page ends above the highest band that crosses its bottom edge. A band
///   taller than a page is cut at the edge (a hard cut).
/// - The next page starts at the next band, so gaps (margins) never start a
///   page and no page is blank.
pub fn paginate(bands: &[Band], page_height: u32) -> Vec<PageSpan> {
    let height = page_height as f32;
    let bands: Vec<Band> = bands
        .iter()
        .copied()
        .filter(|band| band.bottom > band.top && band.top.is_finite() && band.bottom.is_finite())
        .collect();
    let first = bands.iter().map(|b| b.top).fold(f32::INFINITY, f32::min);
    let last = bands
        .iter()
        .map(|b| b.bottom)
        .fold(f32::NEG_INFINITY, f32::max);
    if bands.is_empty() {
        return vec![PageSpan {
            top: 0.0,
            keep: page_height,
        }];
    }
    if last <= height {
        let ink = last - first;
        return vec![PageSpan {
            top: (first - (height - ink) / 2.0).floor(),
            keep: page_height,
        }];
    }

    let mut pages = Vec::new();
    let mut top = 0.0f32;
    while pages.len() < MAX_PAGES {
        let limit = top + height;
        let straddle = bands
            .iter()
            .filter(|b| b.top < limit && b.bottom > limit)
            .map(|b| b.top)
            .fold(f32::INFINITY, f32::min);
        let mut cut = if straddle.is_finite() {
            straddle.floor()
        } else {
            limit
        };
        if cut <= top {
            cut = limit;
        }
        pages.push(PageSpan {
            top,
            keep: (cut - top) as u32,
        });
        let next = bands
            .iter()
            .filter(|b| b.bottom > cut)
            .map(|b| b.top.max(cut))
            .fold(f32::INFINITY, f32::min);
        if !next.is_finite() {
            break;
        }
        top = next.floor();
    }
    pages
}

/// The page that shows device row `y` of the document.
pub fn page_of(pages: &[PageSpan], y: f32) -> usize {
    pages.iter().rposition(|page| page.top <= y).unwrap_or(0)
}

/// Line boxes and images of the laid-out document, in device pixels.
///
/// Positions follow blitz-paint: each node is placed at its parent's position
/// plus its `final_layout().location`. The lines of an inline root start at
/// its content box plus `TextLayout::block_offset` (CSS px). Parley line
/// metrics are in layout units, which are CSS px times `Layout::scale()`.
pub fn collect_bands(doc: &BaseDocument, scale: f32) -> Vec<Band> {
    let mut bands = Vec::new();
    if let Some(root) = doc.try_root_element() {
        walk(doc, root, 0.0, scale, &mut bands);
    }
    bands
}

fn walk(doc: &BaseDocument, node: &Node, parent_top: f32, scale: f32, bands: &mut Vec<Band>) {
    let layout = node.final_layout();
    let top = parent_top + layout.location.y;
    if let Some(element) = node.element_data() {
        if node.flags.is_inline_root()
            && let Some(text) = element.inline_layout_data.as_deref()
        {
            let content_top = top + layout.border.top + layout.padding.top + text.block_offset;
            let units = text.layout.scale().max(f32::EPSILON);
            for line in text.layout.lines() {
                let m = line.metrics();
                let line_top = m.block_min_coord.min(m.content_block_min_coord) / units;
                let line_bottom = m.block_max_coord.max(m.content_block_max_coord) / units;
                bands.push(Band {
                    top: (content_top + line_top) * scale,
                    bottom: (content_top + line_bottom) * scale,
                });
            }
        }
        if element.image_data().is_some() {
            bands.push(Band {
                top: top * scale,
                bottom: (top + layout.size.height) * scale,
            });
        }
    }
    if let Some(children) = &*node.layout_children.borrow() {
        for &child in children {
            walk(doc, &doc.tree()[child], top, scale, bands);
        }
    }
}

/// Top of the element with `id`, in device pixels.
pub fn element_top(doc: &BaseDocument, id: &str, scale: f32) -> Option<f32> {
    let node_id = doc.get_fragment_target(id)?;
    let node = doc.get_node(node_id)?;
    Some(node.absolute_position(0.0, 0.0).y * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(start: f32, height: f32, count: usize) -> Vec<Band> {
        (0..count)
            .map(|i| Band {
                top: start + i as f32 * height,
                bottom: start + (i + 1) as f32 * height,
            })
            .collect()
    }

    /// No band crosses a cut, and every band is on some page.
    fn assert_no_cut_lines(bands: &[Band], pages: &[PageSpan], height: u32) {
        for band in bands {
            let page = pages
                .iter()
                .find(|p| band.top >= p.top && band.top < p.top + p.keep as f32)
                .unwrap_or_else(|| panic!("{band:?} is on no page: {pages:?}"));
            if band.bottom - band.top <= height as f32 {
                assert!(
                    band.bottom <= page.top + page.keep as f32,
                    "{band:?} is cut by {page:?}"
                );
            }
        }
    }

    #[test]
    fn empty_is_one_page() {
        assert_eq!(
            paginate(&[], 100),
            vec![PageSpan {
                top: 0.0,
                keep: 100
            }]
        );
    }

    #[test]
    fn short_card_is_centered() {
        // Ink from 10 to 50 on a 100 row page: 30 rows above and below.
        let pages = paginate(&lines(10.0, 20.0, 2), 100);
        assert_eq!(
            pages,
            vec![PageSpan {
                top: -20.0,
                keep: 100
            }]
        );
    }

    #[test]
    fn breaks_above_a_crossing_line() {
        // Lines of 30 rows: 0..30, 30..60, 60..90, 90..120 on 100 row pages.
        let bands = lines(0.0, 30.0, 4);
        let pages = paginate(&bands, 100);
        assert_eq!(
            pages,
            vec![
                PageSpan { top: 0.0, keep: 90 },
                PageSpan {
                    top: 90.0,
                    keep: 100
                },
            ]
        );
        assert_no_cut_lines(&bands, &pages, 100);
    }

    #[test]
    fn gaps_do_not_start_a_page() {
        let mut bands = lines(0.0, 30.0, 3);
        bands.extend(lines(150.0, 30.0, 2));
        let pages = paginate(&bands, 100);
        assert_eq!(pages[1].top, 150.0);
        assert_eq!(pages.len(), 2);
        assert_no_cut_lines(&bands, &pages, 100);
    }

    #[test]
    fn tall_band_is_hard_cut() {
        let bands = [
            Band {
                top: 0.0,
                bottom: 20.0,
            },
            Band {
                top: 20.0,
                bottom: 270.0,
            },
            Band {
                top: 270.0,
                bottom: 290.0,
            },
        ];
        let pages = paginate(&bands, 100);
        // Page 1 ends above the tall band, which then fills pages 2 to 4.
        assert_eq!(pages[0], PageSpan { top: 0.0, keep: 20 });
        assert_eq!(
            pages[1],
            PageSpan {
                top: 20.0,
                keep: 100
            }
        );
        assert_eq!(pages[2].top, 120.0);
        assert!(pages.last().unwrap().top + 100.0 >= 290.0);
    }

    #[test]
    fn table_cells_break_above_the_highest_crossing_line() {
        // Two columns with different line heights.
        let mut bands = lines(0.0, 30.0, 5);
        bands.extend(lines(0.0, 45.0, 3));
        let pages = paginate(&bands, 100);
        assert_eq!(pages[0].keep, 90);
        assert_no_cut_lines(&bands, &pages, 100);
    }

    #[test]
    fn fractional_positions_round_down() {
        let bands = lines(0.4, 33.3, 5);
        let pages = paginate(&bands, 100);
        assert_no_cut_lines(&bands, &pages, 100);
        for page in &pages {
            assert_eq!(page.top, page.top.floor());
        }
    }

    #[test]
    fn page_of_finds_the_page() {
        let pages = [
            PageSpan { top: 0.0, keep: 90 },
            PageSpan {
                top: 90.0,
                keep: 100,
            },
        ];
        assert_eq!(page_of(&pages, 0.0), 0);
        assert_eq!(page_of(&pages, 89.0), 0);
        assert_eq!(page_of(&pages, 95.0), 1);
        assert_eq!(page_of(&pages, -5.0), 0);
    }
}
