//! Library half of `survivors-lobby`, split out so its modules carry unit tests even though the
//! `[[bin]]` itself runs with `test = false` — the same shape `wenilla-host` uses.

pub mod rest;
pub mod save;
pub mod state;
pub mod ws;
