// SPDX-License-Identifier: EUPL-1.2
//! What a write confirmation keeps in the store (T-7507): the open question
//! with the person's decision, or a spent nonce, so every replica and the
//! process after a restart answer alike.

use std::time::Duration;

/// Subject headers and their values, as the call that asked sent them.
pub type Subject = Vec<(String, Vec<u8>)>;

/// One open question and what was decided on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    /// The view the write goes through.
    pub view: String,
    /// The tool that would run.
    pub tool: String,
    /// What the person is asked.
    pub message: String,
    /// Who asked.
    pub subject: Subject,
    /// How long it still takes a decision, by the store's clock.
    pub left: Duration,
    /// `Some(true)` accepted, `Some(false)` declined, `None` undecided.
    pub decision: Option<bool>,
}

/// The most live rows of one sort (questions, or spent nonces).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// In one Tenant.
    pub tenant: u32,
    /// Of one subject in that Tenant, so one caller cannot fill it.
    pub subject: u32,
}

/// What the retry of a write finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    /// Accepted; the question is gone, so this one retry runs the write.
    Accepted,
    /// Declined; the question is gone.
    Declined,
    /// Open and undecided; it stays.
    Waiting,
    /// Expired, taken before, another caller's, or never asked.
    Unknown,
}
