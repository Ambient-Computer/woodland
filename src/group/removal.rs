//! Removal: which of a removed contributor's writes still count.
//!
//! # The problem
//!
//! A removed device may hold writes nobody has seen: made offline, before
//! it heard it was removed, or after. They could surface at any time, and
//! they were written on top of whatever the device had back then. If they
//! always counted, no stretch of the timeline could ever be final, because
//! one of them might still land inside it. If they stopped counting at
//! "the moment of removal", replicas would disagree, because each learns
//! of a removal at a different time.
//!
//! # The rule
//!
//! A removed contributor's transaction counts if, and only if, **a current
//! contributor has reported integrating it**. Everyone can evaluate that
//! from the learned decision alone, and learned decisions never disagree.
//!
//! Before a device learns of the removal, it takes the removed device's
//! writes like anyone else's. A contributor that did so vouches for them
//! in its next report, and the others then take them too. Writes that no
//! contributor took before hearing of the removal are never vouched for,
//! and everyone discards them.
//!
//! # When the answer is final
//!
//! Once every contributor has acknowledged the removal, each one's report
//! covers everything it took while it didn't know. After that, a
//! contributor only takes what another already vouched for. So the set of
//! vouched writes can't grow anymore, and the floor may move (see
//! `Decision::floor`).
//!
//! # When every contributor is new
//!
//! If the contributors who could have vouched were all removed too, and
//! none of the current ones has reported yet, nobody vouches for
//! anything, and the newcomers could never catch up. Until one of them
//! reports, any report vouches, a removed contributor's included. That's
//! the same history the newcomers have to catch up to before they're
//! admitted (see `admission`), and nothing settles in the meantime,
//! because they haven't acknowledged anything yet.
//!
//! # Rewinding
//!
//! A device that isn't a contributor has no report to vouch with. If it
//! integrated a write that ends up unvouched (it synced with the removed
//! device directly, say), it holds history that the contributors agreed
//! doesn't exist. Unpicking one transaction from derived state isn't
//! possible in general, so it [rewinds]: goes back to what has settled,
//! and syncs everything since from its peers. What has settled never
//! holds an unvouched write, so it's never taken back. The same goes for a removed device's own
//! unvouched writes, and for a newcomer that hasn't been admitted yet.
//! An admitted contributor never needs to: whatever it holds, it vouches
//! for.
//!
//! [rewinds]: must_rewind

use crate::actor::Peer;
use crate::agreement::Decision;
use crate::clock::VersionVector;

/// Whether `author`'s transaction `seq` may be integrated, as far as
/// `learned` knows. For a current contributor, always. For a removed one,
/// only if it's vouched for. Once every contributor has acknowledged the
/// removal, the answer is final.
pub(super) fn counts(learned: &Decision, author: Peer, seq: u64) -> bool {
    let configuration = &learned.configuration;

    configuration.is_member(author)
        || (configuration.was_ever_member(author) && seq <= learned.vouched(author))
}

/// Whether this device has integrated a removed contributor's transaction
/// that nobody vouches for (yet).
pub(super) fn holds_unvouched(learned: &Decision, integrated: &VersionVector) -> bool {
    let mut removed = learned.configuration.removed();

    removed.any(|author| integrated.get(author) > learned.vouched(author))
}

/// Whether this device must drop what it has integrated and sync again:
/// it holds unvouched transactions, every contributor that could still
/// vouch for them has acknowledged the removal without doing so, and it
/// can't vouch for them itself.
pub(super) fn must_rewind(
    learned: &Decision,
    me: Peer,
    admitted: bool,
    integrated: &VersionVector,
) -> bool {
    let vouches = admitted && learned.configuration.is_member(me);

    !vouches && learned.acknowledged_by_all_but(me) && holds_unvouched(learned, integrated)
}

/// Whether a transaction that hasn't been integrated never will be: its
/// author was removed, and everyone acknowledged that without vouching for
/// it.
pub(super) fn abandoned(learned: &Decision, me: Peer, author: Peer, seq: u64) -> bool {
    let removed = !learned.configuration.is_member(author);

    removed && learned.acknowledged_by_all_but(me) && !counts(learned, author, seq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agreement::{Change, Configuration};
    use crate::clock::Progress;
    use crate::lattice::Lattice;
    use crate::testing::peers::{friend, laptop, nas, phone};

    fn clock(entries: &[(Peer, u64)]) -> VersionVector {
        entries.iter().copied().collect()
    }

    /// The nas was removed. The phone had taken two of its transactions
    /// and the laptop one. Only the phone has acknowledged the removal.
    fn nas_was_removed() -> Decision {
        let mut progress = Progress::report(phone(), clock(&[(phone(), 1), (nas(), 2)]));
        progress.join(&Progress::report(laptop(), clock(&[(nas(), 1)])));

        let founders = Configuration::founded_by([phone(), laptop(), nas()]);
        let configuration = founders.with(Change::Remove(nas()));
        Decision {
            progress,
            acknowledged: [(phone(), configuration.clone()), (laptop(), founders)].into(),
            configuration,
        }
    }

    fn everyone_acknowledged() -> Decision {
        let mut learned = nas_was_removed();
        let configuration = learned.configuration.clone();
        learned.acknowledged.insert(laptop(), configuration);

        learned
    }

    #[test]
    fn a_removed_contributors_writes_count_if_someone_vouches() {
        let learned = nas_was_removed();

        assert!(counts(&learned, nas(), 2));
        assert!(!counts(&learned, nas(), 3));
        assert!(counts(&learned, phone(), 9));
        assert!(!counts(&learned, friend(), 1));
    }

    #[test]
    fn a_removed_contributors_own_report_vouches_for_nothing() {
        let mut learned = nas_was_removed();
        let own = Progress::report(nas(), clock(&[(nas(), 5)]));
        learned.progress.join(&own);

        assert!(!counts(&learned, nas(), 3));
    }

    /// The phone and laptop were replaced by the friend, which hasn't
    /// reported yet. Somebody has to vouch for what they wrote.
    #[test]
    fn any_report_vouches_until_a_current_contributor_reports() {
        let mut learned = everyone_acknowledged();
        let replaced = [phone(), laptop()].map(Change::Remove);
        for change in [Change::Add(friend())].into_iter().chain(replaced) {
            learned.configuration = learned.configuration.with(change);
        }

        assert!(counts(&learned, nas(), 2));
        assert!(counts(&learned, phone(), 1));
        assert!(!counts(&learned, phone(), 2));

        // Once the friend has caught up and reported, only its word counts.
        let caught_up = clock(&[(phone(), 1), (nas(), 2)]);
        let late = Progress::report(phone(), clock(&[(nas(), 5)]));
        learned
            .progress
            .join(&Progress::report(friend(), caught_up));
        learned.progress.join(&late);
        assert!(counts(&learned, nas(), 2));
        assert!(!counts(&learned, nas(), 3));
    }

    #[test]
    fn an_observer_rewinds_once_nobody_can_vouch_anymore() {
        let held = clock(&[(nas(), 3)]);

        assert!(holds_unvouched(&nas_was_removed(), &held));
        assert!(!must_rewind(&nas_was_removed(), friend(), false, &held));
        assert!(must_rewind(
            &everyone_acknowledged(),
            friend(),
            false,
            &held
        ));
    }

    #[test]
    fn a_contributor_vouches_instead_of_rewinding() {
        let held = clock(&[(nas(), 3)]);
        let mut learned = nas_was_removed();
        let configuration = learned.configuration.clone();
        learned.acknowledged.insert(phone(), configuration);

        assert!(!must_rewind(&learned, laptop(), true, &held));
    }

    #[test]
    fn the_removed_device_rewinds_its_own_unvouched_writes() {
        let held = clock(&[(nas(), 3)]);

        assert!(must_rewind(&everyone_acknowledged(), nas(), true, &held));
    }

    #[test]
    fn unvouched_transactions_are_abandoned_once_everyone_knows() {
        assert!(!abandoned(&nas_was_removed(), friend(), nas(), 3));
        assert!(abandoned(&everyone_acknowledged(), friend(), nas(), 3));
        assert!(!abandoned(&everyone_acknowledged(), friend(), nas(), 2));
        assert!(!abandoned(&everyone_acknowledged(), friend(), phone(), 9));
    }
}
