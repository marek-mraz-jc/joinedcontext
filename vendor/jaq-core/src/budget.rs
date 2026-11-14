//! Evaluation budget (jc patch, vendor/SOURCES.md): a step count, a deadline and a stack floor,
//! held per thread, so a host running a query it did not write can stop it.
//!
//! [`limit`] arms the budget for one closure; every filter evaluation and every value the native
//! `range` yields spends one step. Once a step finds the budget spent, that and every later step
//! answers an error, so the query unwinds instead of looping or recursing on.

use core::cell::Cell;
use std::time::{Duration, Instant};

/// Why a budget stopped a query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exhausted {
    /// More evaluation steps than allowed.
    Steps,
    /// The deadline passed.
    Time,
    /// The evaluation went deeper into the stack than allowed.
    Stack,
    /// The values built (strings, arrays, JSON text) grew past the byte limit.
    Bytes,
}

#[derive(Clone, Copy)]
struct State {
    steps_left: u64,
    bytes_left: usize,
    deadline: Instant,
    stack_floor: usize,
    exhausted: Option<Exhausted>,
}

std::thread_local! {
    static BUDGET: Cell<Option<State>> = const { Cell::new(None) };
}

/// How often (in steps) the clock is read.
const CLOCK_EVERY: u64 = 256;

/// The limits of one [`limit`] call.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Evaluation steps.
    pub steps: u64,
    /// Wall time.
    pub time: Duration,
    /// Bytes of stack below the [`limit`] call.
    pub stack: usize,
    /// Bytes of strings, array slots and JSON text built (see [`alloc`]).
    pub bytes: usize,
}

/// Runs `f` within `limits` and returns its result with the reason the budget ran out, if it did.
pub fn limit<R>(limits: Limits, f: impl FnOnce() -> R) -> (R, Option<Exhausted>) {
    let Limits {
        steps,
        time,
        stack,
        bytes,
    } = limits;
    let here = 0u8;
    let base = core::ptr::addr_of!(here) as usize;
    let state = State {
        steps_left: steps,
        bytes_left: bytes,
        deadline: Instant::now() + time,
        stack_floor: base.saturating_sub(stack),
        exhausted: None,
    };
    let outer = BUDGET.with(|b| b.replace(Some(state)));
    let r = f();
    let exhausted = BUDGET.with(|b| b.replace(outer)).and_then(|s| s.exhausted);
    (r, exhausted)
}

/// Spends one step; `Err` once the budget is spent (never without [`limit`]).
pub fn step() -> Result<(), Exhausted> {
    BUDGET.with(|b| {
        let Some(mut s) = b.get() else { return Ok(()) };
        if let Some(e) = s.exhausted {
            return Err(e);
        }
        let here = 0u8;
        let sp = core::ptr::addr_of!(here) as usize;
        let e = if sp < s.stack_floor {
            Some(Exhausted::Stack)
        } else if s.steps_left == 0 {
            Some(Exhausted::Steps)
        } else if s.steps_left % CLOCK_EVERY == 0 && Instant::now() > s.deadline {
            Some(Exhausted::Time)
        } else {
            None
        };
        s.steps_left = s.steps_left.saturating_sub(1);
        s.exhausted = e;
        b.set(Some(s));
        e.map_or(Ok(()), Err)
    })
}

/// Spends `n` bytes of what the query builds; `Err` once that is more than is left, and from then
/// on every step fails too. A caller checks this before it allocates.
pub fn alloc(n: usize) -> Result<(), Exhausted> {
    BUDGET.with(|b| {
        let Some(mut s) = b.get() else { return Ok(()) };
        if let Some(e) = s.exhausted {
            return Err(e);
        }
        match s.bytes_left.checked_sub(n) {
            Some(left) => s.bytes_left = left,
            None => s.exhausted = Some(Exhausted::Bytes),
        }
        b.set(Some(s));
        s.exhausted.map_or(Ok(()), Err)
    })
}

impl core::fmt::Display for Exhausted {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.write_str(match self {
            Self::Steps => "evaluation step limit reached",
            Self::Time => "evaluation time limit reached",
            Self::Stack => "evaluation depth limit reached",
            Self::Bytes => "evaluation size limit reached",
        })
    }
}
