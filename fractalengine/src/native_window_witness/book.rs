//! Serialized property ownership and nonresetting epochs; see AGENTS.md.

use super::windows;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(2);
static POISONED: AtomicBool = AtomicBool::new(false);
static BOOK: Mutex<Book> = Mutex::new(Book {
    revision: 1,
    published: None,
});
struct Book {
    revision: u64,
    published: Option<(isize, u64)>,
}

fn poison() {
    if !POISONED.swap(true, Ordering::SeqCst) {
        tracing::warn!("native window witness unavailable until process restart");
    }
}

pub(super) fn successor(value: u64) -> Option<u64> {
    value
        .checked_add(1)
        .filter(|next| *next <= isize::MAX as u64)
}

#[allow(deprecated)] // try_update needs Rust 1.95; MSRV is 1.91
pub(super) fn fresh() -> Option<u64> {
    match NEXT.fetch_update(Ordering::SeqCst, Ordering::SeqCst, successor) {
        Ok(value) => Some(value),
        Err(_) => {
            poison();
            None
        }
    }
}

fn clear(book: &mut Book) {
    if let Some((hwnd, epoch)) = book.published.take() {
        if windows::clear(hwnd, epoch).is_err() {
            poison();
        }
    }
}

pub(super) fn invalidate() {
    let Ok(mut book) = BOOK.lock() else {
        poison();
        return;
    };
    clear(&mut book);
    if let Some(revision) = fresh() {
        book.revision = revision;
    }
}

pub(super) fn begin_frame() -> Option<u64> {
    let Ok(mut book) = BOOK.lock() else {
        poison();
        return None;
    };
    clear(&mut book);
    (!POISONED.load(Ordering::SeqCst)).then_some(book.revision)
}

pub(super) fn revision() -> Option<u64> {
    let book = BOOK.lock().ok()?;
    (!POISONED.load(Ordering::SeqCst)).then_some(book.revision)
}

pub(super) fn publish(revision: u64, hwnd: isize, epoch: u64) -> bool {
    let Ok(mut book) = BOOK.lock() else {
        poison();
        return false;
    };
    if POISONED.load(Ordering::SeqCst) || book.revision != revision || book.published.is_some() {
        return false;
    }
    // Record ownership before a partial native failure, then retire only that exact value.
    book.published = Some((hwnd, epoch));
    if windows::publish(hwnd, epoch).is_err() {
        clear(&mut book);
        poison();
        return false;
    }
    true
}
