use super::*;
use anki::scheduler::states::{CardState, NewState};

fn temp_collection(name: &str) -> (PathBuf, Collection) {
    let dir = std::env::temp_dir().join(format!("pbanki-api-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let col = CollectionBuilder::new(dir.join("collection.anki2"))
        .build()
        .unwrap();
    (dir, col)
}

fn new_states() -> SchedulingStates {
    let new: CardState = NewState { position: 0 }.into();
    SchedulingStates {
        current: new,
        again: new,
        hard: new,
        good: new,
        easy: new,
    }
}

/// A rating that cannot be saved must surface as an error instead of loading
/// the unchanged queue head as if it were the next card.
#[test]
fn rate_card_reports_failed_answer() {
    let (dir, mut col) = temp_collection("rate");
    let fonts = FontCatalog::build("none", &[], &[]);

    let result = rate_card(&mut col, 424242, new_states(), 0, 2, 1, 100, true, &fonts);

    let err = result.err().expect("answering a missing card must fail");
    assert!(err.starts_with("could not save the rating"), "{err}");
    let reviews: u32 = col
        .storage
        .db()
        .query_row("select count() from revlog", [], |row| row.get(0))
        .unwrap();
    assert_eq!(reviews, 0);

    drop(col);
    let _ = fs::remove_dir_all(&dir);
}
