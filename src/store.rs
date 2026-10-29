//! Mac App Store subscriptions (StoreKit 2 bridge in packaging/appstore/Store.swift, linked by build.rs).
//! Answers arrive on background threads; the UI collects them with [`take`] every frame.

use eframe::egui;
use std::ffi::{CStr, CString, c_char};
use std::sync::{Mutex, OnceLock};

/// Product IDs of the auto-renewable subscriptions in App Store Connect (one subscription group).
pub const MONTHLY: &str = "com.pelamx.deskmonk.pro.monthly";
pub const YEARLY: &str = "com.pelamx.deskmonk.pro.yearly";

pub enum Event {
    /// End of the paid period (unix seconds), 0 = not subscribed.
    ProUntil(u64),
    /// Localized prices (monthly, yearly).
    Prices(Option<String>, Option<String>),
    Bought,
    Cancelled,
    Pending,
    Failed,
}

static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());
static REPAINT: OnceLock<egui::Context> = OnceLock::new();

unsafe extern "C" {
    fn dm_store_start(status: extern "C" fn(i64));
    fn dm_store_restore(status: extern "C" fn(i64));
    fn dm_store_prices(monthly: *const c_char, yearly: *const c_char, cb: extern "C" fn(*const c_char, *const c_char));
    fn dm_store_buy(id: *const c_char, cb: extern "C" fn(i32), status: extern "C" fn(i64));
}

fn push(event: Event) {
    EVENTS.lock().unwrap().push(event);
    if let Some(ctx) = REPAINT.get() {
        ctx.request_repaint();
    }
}

extern "C" fn on_status(until: i64) {
    push(Event::ProUntil(until.max(0) as u64));
}

extern "C" fn on_prices(monthly: *const c_char, yearly: *const c_char) {
    let text = |p: *const c_char| (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned());
    push(Event::Prices(text(monthly), text(yearly)));
}

extern "C" fn on_buy(code: i32) {
    push(match code {
        0 => Event::Bought,
        1 => Event::Cancelled,
        2 => Event::Pending,
        _ => Event::Failed,
    });
}

/// Starts listening for subscription changes and asks for the current status and prices.
pub fn start(ctx: &egui::Context) {
    let _ = REPAINT.set(ctx.clone());
    let (m, y) = (CString::new(MONTHLY).unwrap(), CString::new(YEARLY).unwrap());
    unsafe {
        dm_store_start(on_status);
        dm_store_prices(m.as_ptr(), y.as_ptr(), on_prices);
    }
}

pub fn buy(yearly: bool) {
    let id = CString::new(if yearly { YEARLY } else { MONTHLY }).unwrap();
    unsafe { dm_store_buy(id.as_ptr(), on_buy, on_status) }
}

pub fn restore() {
    unsafe { dm_store_restore(on_status) }
}

pub fn take() -> Vec<Event> {
    std::mem::take(&mut *EVENTS.lock().unwrap())
}
