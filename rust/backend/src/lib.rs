//! Backend logic for the standalone Rust service migration.
#![deny(unsafe_op_in_unsafe_fn)]

pub mod layout;
pub mod paste;

pub mod linux;
pub mod rex;
pub mod service;
pub mod state;
pub mod uploads;
pub mod vt;
