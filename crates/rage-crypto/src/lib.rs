//! RAGE archive cryptography (GTA V, GTA IV).
//!
//! This crate ships no Rockstar key material. All keys are derived at runtime
//! from files the user already owns (the game executable, or a key blob the
//! user points to). Key caches are written to and read from a path the caller
//! chooses ([`GtaVKeys::save`], [`Gta4Key::load`], …).
#![forbid(unsafe_code)]

mod aes_ecb;
mod dotnet_random;
pub mod gta4;
pub mod key_hashes;
mod keys;
mod magic;
mod ng;
mod scan;
pub mod xxtea;

pub use aes_ecb::{decrypt_aes, encrypt_aes};
pub use gta4::Gta4Key;
pub use keys::{GtaVKeys, KeyCacheError};
pub use magic::{MagicError, decode_codewalker_magic};
pub use ng::{NG_KEY_COUNT, NgKey, NgTables, decrypt_ng};
pub use scan::{ExeScan, scan_executable};
