//! WordCraft in the browser: the eframe web shell ([`web`], wasm only) and craft-live mode
//! ([`live`]: the web build as a live editor inside Nextcloud Office, through craft-live), whose
//! protocol is plain Rust, built and tested natively too.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod live;

#[cfg(target_arch = "wasm32")]
pub mod web;
