//! Admission: when a device may start writing.
//!
//! # The rule
//!
//! A device writes once it has **learned** that it's a contributor, and
//! has **caught up** to what the contributors before it agree they hold.
//! Until then its writes are held, unsigned. They're signed when it's
//! admitted, on top of everything it caught up to.
//!
//! # Why a newcomer can't write sooner
//!
//! A transaction names what it was written on top of, and that never
//! changes once it's signed. A device that signed while it was still
//! waiting for access would produce transactions written on top of
//! nothing: concurrent with all of the agreement's history, however old.
//!
//! That history is meant to settle. Once every contributor holds a stretch
//! of the timeline, nothing new should be able to land inside it, so its
//! order can be treated as final. Contributors keep that promise by
//! construction, because everything they write follows what they last
//! reported. A newcomer wasn't counted when the others settled, so it has
//! to make the same promise before its first write: build on what they had
//! settled, never beside it.
//!
//! # What "caught up" means
//!
//! The device must have integrated the [`baseline`] of the decision it
//! learned: what every other contributor has reported holding. Nothing
//! settled before the newcomer was admitted can go beyond that. A
//! contributor who was there reported at least as much, and a newcomer
//! who wasn't had to catch up the same way before it reported anything.
//!
//! Founders are admitted from the start. They're contributors in every
//! decision, and a contributor who hasn't reported yet holds the floor at
//! zero, so nothing settles without them.
//!
//! A newcomer that integrated a removed contributor's writes while it was
//! only observing also waits until those are vouched for. Its first report
//! would vouch for them otherwise (see `removal`).
//!
//! Admission is remembered (see `log::Clock`). A contributor that learns
//! of newer history while offline keeps writing: it's already counted, so
//! the floor waits for it.
//!
//! [`baseline`]: crate::clock::Progress::baseline

use std::collections::VecDeque;

use crate::actor::Peer;
use crate::agreement::{AgreementId, Decision};
use crate::clock::VersionVector;
use crate::lattice::Lattice;
use crate::model::Operation;
use crate::node::Outbox;
use crate::storage::keys;

/// Whether a device that has learned `learned` and integrated `integrated`
/// may be admitted.
pub(super) fn caught_up(learned: &Decision, me: Peer, integrated: &VersionVector) -> bool {
    let contributors = learned.configuration.members();
    let baseline = learned.progress.baseline(&contributors, me);

    contributors.contains(&me) && baseline.leq(integrated)
}

/// Writes made before this device was admitted, in the order they were
/// made. They're stored, so they survive a restart.
#[derive(Debug, Default)]
pub(super) struct Held {
    writes: VecDeque<HeldWrite>,
    /// The storage index of the next write to hold.
    next: u64,
}

/// A write as the application made it: not yet a transaction.
pub(super) type HeldWrite = Vec<Operation>;

impl Held {
    /// Rebuilds from storage. `stored` is in storage order, by index.
    pub(super) fn restore(stored: Vec<(u64, HeldWrite)>) -> Self {
        let next = stored.last().map_or(0, |(index, _)| index + 1);
        let writes = stored.into_iter().map(|(_, write)| write).collect();

        Self { writes, next }
    }

    pub(super) fn hold(&mut self, id: AgreementId, write: HeldWrite, outbox: &mut Outbox) {
        outbox.put(keys::held(id, self.next), &write);
        self.writes.push_back(write);
        self.next += 1;
    }

    /// Takes every held write, oldest first, and removes them from storage.
    pub(super) fn release(&mut self, id: AgreementId, outbox: &mut Outbox) -> Vec<HeldWrite> {
        if self.writes.is_empty() {
            return Vec::new();
        }

        let (start, end) = keys::held_through(id, self.next);
        outbox.delete_range(start, end);

        self.writes.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agreement::{Change, Configuration};
    use crate::clock::Progress;
    use crate::testing::peers::{friend, laptop, phone};

    fn clock(entries: &[(Peer, u64)]) -> VersionVector {
        entries.iter().copied().collect()
    }

    /// The phone and laptop have each reported, and the friend was added.
    fn friend_was_added() -> Decision {
        let mut progress = Progress::report(phone(), clock(&[(phone(), 5), (laptop(), 2)]));
        progress.join(&Progress::report(
            laptop(),
            clock(&[(phone(), 3), (laptop(), 4)]),
        ));

        Decision {
            progress,
            configuration: Configuration::founded_by([phone(), laptop()])
                .with(Change::Add(friend())),
            ..Decision::default()
        }
    }

    #[test]
    fn a_newcomer_waits_until_it_holds_what_the_others_do() {
        let learned = friend_was_added();
        let behind = clock(&[(phone(), 3), (laptop(), 1)]);
        let level = clock(&[(phone(), 3), (laptop(), 2)]);

        assert!(!caught_up(&learned, friend(), &behind));
        assert!(caught_up(&learned, friend(), &level));
    }

    #[test]
    fn nobody_is_admitted_before_learning_theyre_a_contributor() {
        let mut learned = friend_was_added();
        learned.configuration = Configuration::founded_by([phone(), laptop()]);
        let everything = clock(&[(phone(), 5), (laptop(), 4)]);

        assert!(!caught_up(&learned, friend(), &everything));
    }
}
