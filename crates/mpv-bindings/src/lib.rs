//! Safe Rust bindings to libmpv's client API.
//!
//! Architecture:
//!   - `sys`      : raw FFI from bindgen (re-export of OUT_DIR/bindings.rs)
//!   - `error`    : `MpvError` enum + `MpvResult<T>`
//!   - `handle`   : owned `MpvHandle` (creates + initializes libmpv)
//!   - `command`  : typed command builder
//!   - `property` : typed property get/set with serde-style adapters
//!   - `event`    : safe `Event` enum parsed from `mpv_event`
//!
//! Design rules:
//!   1. `MpvHandle` is `Send` but NOT `Clone` — there is exactly one owner.
//!      Multiple consumers must use the property/command API, not raw handle sharing.
//!   2. All FFI calls that return `int` are converted to `MpvResult<()>` via `Error::from_code`.
//!   3. Strings from libmpv are copied into `CString`/`String` immediately on the
//!      engine thread and never held across the FFI boundary.
//!   4. We never expose raw `*mut` pointers in the public API.

// bindgen emits constants like `mpv_format_MPV_FORMAT_FLAG` which trigger
// non_upper_case_globals warnings. They're idiomatic to the C API.
#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

pub mod sys {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

pub mod error;
pub mod handle;
pub mod command;
pub mod property;
pub mod event;

pub use error::{MpvError, MpvResult};
pub use handle::MpvHandle;
pub use event::{Event, EventId, LogLevel};
pub use property::{Format, Property};

/// Library version string from build time.
pub const LIBMPV_VERSION: &str = env!("CARGO_PKG_VERSION");
