//! hotseat — hand one monitor between the machines on your desk.
//!
//! Milestone 1 is deliberately read-only: it discovers what a display *is* and
//! what it can be told, without writing a single byte to it.
//!
//! The DDC surface lives behind the [`vcp::Vcp`] trait so every piece of
//! discovery logic is unit-testable without hardware attached.

pub mod inputs;
pub mod monitor;
pub mod report;
pub mod trust;
pub mod vcp;
