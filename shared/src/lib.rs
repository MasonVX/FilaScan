// might need to put this under feature flag to compile with std
#![no_std]
#![feature(impl_trait_in_assoc_type)]
// might need to put this under feature flag to compile with std
extern crate alloc;

pub mod bambu_reader;
pub mod nfc;
pub mod pn532_ext;
