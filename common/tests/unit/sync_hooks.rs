use super::*;
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};

fn hooks(before: impl Fn() -> Result<(), String> + 'static) -> (SyncHooks, Rc<Cell<u32>>) {
    let finished = Rc::new(Cell::new(0));
    let after = finished.clone();
    let hooks = SyncHooks {
        before_sync: Some(Rc::new(before)),
        after_sync: Some(Rc::new(move || after.set(after.get() + 1))),
    };
    (hooks, finished)
}

#[test]
fn guard_finishes_after_successful_prepare() {
    let (hooks, finished) = hooks(|| Ok(()));

    {
        let _guard = hooks.start().unwrap();
        assert_eq!(finished.get(), 0);
    }

    assert_eq!(finished.get(), 1);
}

#[test]
fn guard_does_not_finish_when_prepare_fails() {
    let (hooks, finished) = hooks(|| Err("offline".to_string()));

    assert!(hooks.start().is_err());
    assert_eq!(finished.get(), 0);
}

#[test]
fn guard_finishes_when_sync_panics() {
    let (hooks, finished) = hooks(|| Ok(()));

    let result = catch_unwind(AssertUnwindSafe(|| {
        let _guard = hooks.start().unwrap();
        panic!("sync failed");
    }));

    assert!(result.is_err());
    assert_eq!(finished.get(), 1);
}
