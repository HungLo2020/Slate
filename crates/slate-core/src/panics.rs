//! Panics that their caller catches and reports itself.
use std::{
    cell::Cell,
    panic::{catch_unwind, AssertUnwindSafe},
};

thread_local! {
    static HANDLED: Cell<usize> = const { Cell::new(0) };
}

/// Run `work` and catch a panic, which the caller handles (for example by
/// reporting the failed operation). Panic hooks see it as handled: see
/// [`is_handled`].
pub fn catch_handled<T>(work: impl FnOnce() -> T) -> std::thread::Result<T> {
    HANDLED.with(|depth| depth.set(depth.get() + 1));
    let result = catch_unwind(AssertUnwindSafe(work));
    HANDLED.with(|depth| depth.set(depth.get() - 1));
    result
}

/// Whether a panic on this thread happens inside [`catch_handled`]. Panic
/// hooks run before unwinding, on the panicking thread, so they can ask.
pub fn is_handled() -> bool {
    HANDLED.with(Cell::get) > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handled_scopes_nest_and_end_with_the_call() {
        assert!(!is_handled());
        let result = catch_handled(|| {
            assert!(is_handled());
            assert_eq!(catch_handled(|| 7).unwrap(), 7);
            assert!(is_handled());
            panic!("caught by the caller");
        });
        assert!(result.is_err());
        assert!(!is_handled());
    }
}
