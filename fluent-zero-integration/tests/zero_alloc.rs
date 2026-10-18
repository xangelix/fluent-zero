//! Verifies the zero-allocation claim for static lookups with a counting
//! global allocator. Counting is per thread so the test harness's own
//! threads cannot skew the result.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    borrow::Cow,
    cell::Cell,
};

use fluent_zero::{set_lang, t};
use fluent_zero_integration::{CACHE, LOCALES};

struct CountingAlloc;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn record() {
    // `try_with` fails only during thread teardown; nothing to count then.
    let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
}

// SAFETY: every call is forwarded unchanged to the system allocator.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// Returns how many heap allocations `f` performed on this thread.
fn allocations_in<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = ALLOCATIONS.with(Cell::get);
    let value = f();
    (value, ALLOCATIONS.with(Cell::get) - before)
}

/// Runs a static lookup once to warm one-time state (the language store's
/// lazy init and this thread's lock-free read slot), then counts a second one.
fn assert_static_lookup_is_allocation_free(key: &'static str, expected: &str) {
    let _ = t!(key);
    let (value, allocations) = allocations_in(|| t!(key));
    assert!(matches!(value, Cow::Borrowed(_)), "{key} was not borrowed");
    assert_eq!(value, expected);
    assert_eq!(allocations, 0, "{key} allocated {allocations} time(s)");
}

// One test so the global language cannot change under a concurrent test.
// `greeting` below exists only in en-US, so it resolves the same either way.
#[test]
fn static_lookups_do_not_allocate() {
    set_lang("en-US".parse().expect("valid language id"));
    assert_static_lookup_is_allocation_free("plain", "Hello");

    // `about` exists only in en-US, so fr-FR takes the fallback path.
    set_lang("fr-FR".parse().expect("valid language id"));
    assert_static_lookup_is_allocation_free("plain", "Bonjour");
    assert_static_lookup_is_allocation_free("about", "About Acme");
}

#[test]
fn counter_sees_dynamic_allocations() {
    // Guards against a counter that silently records nothing.
    let (value, allocations) = allocations_in(|| t!("greeting", { "name" => "Alice" }));
    assert!(value.contains("Alice"));
    assert!(allocations > 0);
}
