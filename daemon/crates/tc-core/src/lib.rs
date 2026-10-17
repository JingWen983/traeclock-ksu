//! TraeClock core — platform-agnostic port of TraeWorkAssistant's
//! check-in + anti-risk-control protocol layer (Trae / WorkBuddy / Qoder).
//!
//! Everything in this crate must build without any OS- or desktop-specific
//! dependency so the same code can run in the Android daemon and be unit
//! tested on any host.

pub mod util;
pub mod jwt;
pub mod fingerprint;
pub mod config;
pub mod store;
pub mod envelope;
pub mod error;
pub mod http;
pub mod events;

pub mod trae;
pub mod trae_oauth;
pub mod workbuddy;
pub mod qoder;
pub mod credits;

mod alias;
pub use alias::trae_checkin;

pub mod schedule;
pub mod runner;
