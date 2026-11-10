//! GTA V archive cryptography.
//!
//! OpenRAGE never ships Rockstar key material. All keys are derived at runtime
//! from files the user already owns (the game executable, or a key blob the
//! user points us to) and cached locally in the user's config directory.

mod aes_ecb;
mod dotnet_random;
pub mod key_hashes;
mod keys;
mod magic;
mod ng;
mod scan;

pub use aes_ecb::{decrypt_aes, encrypt_aes};
pub use keys::{GtaVKeys, KeyCacheError};
pub use magic::{MagicError, decode_codewalker_magic};
pub use ng::{NG_KEY_COUNT, NgKey, NgTables, decrypt_ng};
pub use scan::{ExeScan, scan_executable};
