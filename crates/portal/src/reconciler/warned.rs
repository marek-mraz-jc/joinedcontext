//! Warnings a sync finds again every cycle, logged when they change rather than each cycle: the
//! reconciler syncs every few seconds, and one line repeated per cycle (a group member who is not
//! a realm user yet: 450 lines in two minutes on a rehearsal) hides every other line.

use std::collections::BTreeSet;
use std::sync::Mutex;

/// One family of warnings and the ones the last cycle found.
#[derive(Default)]
pub struct Warned(Mutex<BTreeSet<String>>);

/// What changed since the last cycle: the warnings it did not have, how many it had that this
/// cycle has not, and how many stand now.
#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    pub new: Vec<String>,
    pub cleared: usize,
    pub standing: usize,
}

impl Warned {
    /// Takes this cycle's warnings; the next cycle compares with them.
    pub fn cycle(&self, current: impl IntoIterator<Item = String>) -> Change {
        let current: BTreeSet<String> = current.into_iter().collect();
        let mut seen = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let change = Change {
            new: current.difference(&seen).cloned().collect(),
            cleared: seen.difference(&current).count(),
            standing: current.len(),
        };
        *seen = current;
        change
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_warning_is_new_once_and_again_only_after_it_cleared() {
        let warned = Warned::default();
        let first = warned.cycle(names(&["a", "b"]));
        assert_eq!(
            first,
            Change {
                new: names(&["a", "b"]),
                cleared: 0,
                standing: 2
            }
        );
        for _ in 0..100 {
            let again = warned.cycle(names(&["b", "a"]));
            assert_eq!(
                again,
                Change {
                    new: vec![],
                    cleared: 0,
                    standing: 2
                }
            );
        }
        assert_eq!(
            warned.cycle(names(&["b"])),
            Change {
                new: vec![],
                cleared: 1,
                standing: 1
            }
        );
        assert_eq!(
            warned.cycle(names(&["a", "b"])),
            Change {
                new: names(&["a"]),
                cleared: 0,
                standing: 2
            }
        );
        assert_eq!(
            warned.cycle(Vec::new()),
            Change {
                new: vec![],
                cleared: 2,
                standing: 0
            }
        );
    }
}
