//! Hardware keys of the study screen: what Prev and Next do.
//!
//! Slint only classifies the key (`HwKey`). `resolve` decides the action
//! from the card side, the page, and the configured `KeyAction`s.

use serde::{Deserialize, Serialize};

/// What a hardware key does on one card side. In the config file: `nothing`,
/// `showanswer`, `again`, `hard`, `good`, `easy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyAction {
    #[default]
    Nothing,
    ShowAnswer,
    Again,
    Hard,
    Good,
    Easy,
}

impl KeyAction {
    /// The name in the config file.
    pub fn name(self) -> &'static str {
        match self {
            Self::Nothing => "nothing",
            Self::ShowAnswer => "showanswer",
            Self::Again => "again",
            Self::Hard => "hard",
            Self::Good => "good",
            Self::Easy => "easy",
        }
    }

    /// The rating that `rate_card` takes: 0 Again, 1 Hard, 2 Good, 3 Easy.
    fn rating(self) -> Option<i32> {
        match self {
            Self::Again => Some(0),
            Self::Hard => Some(1),
            Self::Good => Some(2),
            Self::Easy => Some(3),
            Self::Nothing | Self::ShowAnswer => None,
        }
    }
}

/// The two hardware keys that the app tells apart. Left, Down and PageDown
/// count as Prev; Right, Up, PageUp, Return and Menu count as Next
/// (`ui/screens/learn.slint`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwKey {
    Prev,
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Question,
    Answer,
}

/// The configured action of each key on each side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyConfig {
    pub prev_question: KeyAction,
    pub next_question: KeyAction,
    pub prev_answer: KeyAction,
    pub next_answer: KeyAction,
}

impl Default for KeyConfig {
    fn default() -> Self {
        Self {
            prev_question: KeyAction::Nothing,
            next_question: KeyAction::ShowAnswer,
            prev_answer: KeyAction::Again,
            next_answer: KeyAction::Good,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {
    PagePrev,
    PageNext,
    Reveal,
    /// 0 Again, 1 Hard, 2 Good, 3 Easy.
    Rate(i32),
    None,
}

/// What `key` does now. `page` is the 0-based page of the side, which has
/// `total_pages` pages.
///
/// A page turn has precedence: Prev acts only on the first page and Next only
/// on the last page. Show answer works on the question side only, and a
/// rating on the answer side only.
pub fn resolve(
    key: HwKey,
    side: Side,
    page: i32,
    total_pages: i32,
    config: &KeyConfig,
) -> Resolved {
    match key {
        HwKey::Prev if page > 0 => return Resolved::PagePrev,
        HwKey::Next if page < total_pages - 1 => return Resolved::PageNext,
        _ => {}
    }
    let action = match (key, side) {
        (HwKey::Prev, Side::Question) => config.prev_question,
        (HwKey::Next, Side::Question) => config.next_question,
        (HwKey::Prev, Side::Answer) => config.prev_answer,
        (HwKey::Next, Side::Answer) => config.next_answer,
    };
    match (side, action) {
        (Side::Question, KeyAction::ShowAnswer) => Resolved::Reveal,
        (Side::Answer, action) => action.rating().map_or(Resolved::None, Resolved::Rate),
        _ => Resolved::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_on_a_one_page_card() {
        let config = KeyConfig::default();
        let table = [
            (HwKey::Prev, Side::Question, Resolved::None),
            (HwKey::Next, Side::Question, Resolved::Reveal),
            (HwKey::Prev, Side::Answer, Resolved::Rate(0)),
            (HwKey::Next, Side::Answer, Resolved::Rate(2)),
        ];
        for (key, side, expected) in table {
            assert_eq!(
                resolve(key, side, 0, 1, &config),
                expected,
                "{key:?} {side:?}"
            );
        }
    }

    #[test]
    fn page_turn_has_precedence() {
        let config = KeyConfig::default();
        // Three pages: Next turns until the last page, Prev until the first.
        assert_eq!(
            resolve(HwKey::Next, Side::Question, 0, 3, &config),
            Resolved::PageNext
        );
        assert_eq!(
            resolve(HwKey::Next, Side::Question, 1, 3, &config),
            Resolved::PageNext
        );
        assert_eq!(
            resolve(HwKey::Next, Side::Question, 2, 3, &config),
            Resolved::Reveal
        );
        assert_eq!(
            resolve(HwKey::Prev, Side::Answer, 2, 3, &config),
            Resolved::PagePrev
        );
        assert_eq!(
            resolve(HwKey::Prev, Side::Answer, 1, 3, &config),
            Resolved::PagePrev
        );
        assert_eq!(
            resolve(HwKey::Prev, Side::Answer, 0, 3, &config),
            Resolved::Rate(0)
        );
        assert_eq!(
            resolve(HwKey::Next, Side::Answer, 2, 3, &config),
            Resolved::Rate(2)
        );
        // The other key acts at once on its end of the card.
        assert_eq!(
            resolve(HwKey::Next, Side::Answer, 0, 3, &config),
            Resolved::PageNext
        );
    }

    #[test]
    fn every_action_on_each_side() {
        let actions = [
            (KeyAction::Nothing, Resolved::None, Resolved::None),
            (KeyAction::ShowAnswer, Resolved::Reveal, Resolved::None),
            (KeyAction::Again, Resolved::None, Resolved::Rate(0)),
            (KeyAction::Hard, Resolved::None, Resolved::Rate(1)),
            (KeyAction::Good, Resolved::None, Resolved::Rate(2)),
            (KeyAction::Easy, Resolved::None, Resolved::Rate(3)),
        ];
        for (action, question, answer) in actions {
            let config = KeyConfig {
                prev_question: action,
                next_question: action,
                prev_answer: action,
                next_answer: action,
            };
            for key in [HwKey::Prev, HwKey::Next] {
                assert_eq!(resolve(key, Side::Question, 0, 1, &config), question);
                assert_eq!(resolve(key, Side::Answer, 0, 1, &config), answer);
            }
        }
    }

    #[test]
    fn a_message_card_has_no_pages() {
        // `total_pages` is at least 1 in the UI; 0 must not turn pages.
        let config = KeyConfig::default();
        assert_eq!(
            resolve(HwKey::Next, Side::Question, 0, 0, &config),
            Resolved::Reveal
        );
    }

    #[test]
    fn config_names_match_serde() {
        for action in [
            KeyAction::Nothing,
            KeyAction::ShowAnswer,
            KeyAction::Again,
            KeyAction::Hard,
            KeyAction::Good,
            KeyAction::Easy,
        ] {
            #[derive(Deserialize)]
            struct Wrapper {
                key: KeyAction,
            }
            let parsed: Wrapper = toml::from_str(&format!("key = \"{}\"", action.name())).unwrap();
            assert_eq!(parsed.key, action);
        }
    }
}
