//! hotseat — hand one monitor between the machines on your desk.
//!
//! Discovery is the hard part of switching monitor inputs, so it is the part
//! this crate automates: which panel is attached, whether its DDC reads can be
//! believed, and which input values it will actually accept.
//!
//! The DDC surface lives behind the [`vcp::Vcp`] trait so every piece of
//! discovery logic is unit-testable without hardware attached.

pub mod config;
pub mod edid;
pub mod hotkey;
pub mod inputs;
pub mod monitor;
pub mod platform;
pub mod report;
pub mod session;
pub mod switch;
pub mod trust;
pub mod vcp;
