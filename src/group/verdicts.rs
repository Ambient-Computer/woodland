//! Verdicts: which transactions in the frontier happen.
//!
//! # The rule
//!
//! An operation can require something of its key (see
//! [`Requirement`]). The requirement is checked against the model's
//! state at the transaction's place in the timeline: what has settled,
//! plus what every earlier transaction in the frontier wrote. A
//! transaction with a requirement that doesn't hold is **pruned**. It
//! stays in the log, because others were written on top of it, but none
//! of its operations happen, in any model.
//!
//! Every operation also requires its model to be alive: a transaction
//! that names a model after it was dropped is pruned.
//!
//! Nothing here knows what a model is. Operations declare what they
//! write and what they require, and that's all that's read.
//!
//! # Verdicts change
//!
//! A transaction that arrives late lands in the middle of the timeline,
//! and can change the verdict of anything after it: a claim that had won
//! now loses, and a transaction that had lost to *that* one now wins. So
//! every verdict in the frontier is provisional, and they're all worked
//! out again whenever the frontier changes.
//!
//! Storage holds a transaction's versions exactly while its verdict is
//! [`Live`], so readers never see what a pruned transaction wrote, and
//! never need to know why.
//!
//! Once a transaction settles, nothing can land before it anymore, and
//! its verdict is final. Every device reaches the same one, because it
//! only depends on the timeline up to there.
//!
//! # Waiting for storage
//!
//! Whether a key holds a settled value is in storage, not in memory. The
//! first time a requirement asks, the answer has to be read, and until
//! it's back the transaction is [`Pending`]: not yet live, and not
//! settled either. What's been read is remembered for as long as some
//! transaction in the frontier cares, and kept up to date as
//! transactions settle.
//!
//! [`Live`]: Verdict::Live
//! [`Pending`]: Verdict::Pending

use std::collections::{BTreeMap, BTreeSet};

use crate::actor::{Peer, Verified};
use crate::agreement::AgreementId;
use crate::log::Transaction;
use crate::model::{ModelId, Operation, Requirement, Write};
use crate::node::Outbox;
use crate::storage::keys;

/// Whether a transaction's operations happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    /// Every requirement holds. Its writes are in storage.
    Live,
    /// A requirement doesn't hold. Nothing it wrote is in storage.
    Pruned,
    /// A requirement depends on something that hasn't been read yet.
    Pending,
}

/// A key in a model.
type Place = (ModelId, Vec<u8>);

#[derive(Debug, Default)]
pub(super) struct Verdicts {
    /// The verdict of each transaction in the frontier.
    reached: BTreeMap<(Peer, u64), Verdict>,
    /// The verdict storage reflects for each: the last one it reached
    /// that wasn't pending.
    stored: BTreeMap<(Peer, u64), Verdict>,
    /// Whether a place holds a settled value, where that's known.
    settled: BTreeMap<Place, bool>,
    /// The places some requirement in the frontier asks about.
    wanted: BTreeSet<Place>,
    /// Reads that haven't been answered. `true` once the answer is out of
    /// date, because the place settled again after it was asked for.
    reading: BTreeMap<Place, bool>,
}

/// What the frontier has written, partway along the timeline.
#[derive(Default)]
struct Written {
    /// Whether the last live write to a place left a value.
    holds: BTreeMap<Place, bool>,
    /// Places a pending transaction wrote to. Nobody knows what they hold.
    unsure: BTreeSet<Place>,
    /// Models dropped so far, settled drops included.
    dropped: BTreeSet<ModelId>,
    /// Models a pending transaction dropped. Nobody knows if they're alive.
    unsure_models: BTreeSet<ModelId>,
}

impl Verdicts {
    /// Picks up after a restart. A transaction that requires nothing was
    /// live from the moment it was integrated, so storage already holds
    /// its writes. The others are worked out again.
    pub(super) fn resume(frontier: &[&Verified<Transaction>]) -> Self {
        let requires_nothing = |it: &&&Verified<Transaction>| {
            let mut operations = it.payload().operations.iter();

            operations.all(|operation| operation.belongs() && operation.requires().is_none())
        };

        let live = frontier.iter().filter(requires_nothing);
        let reached: BTreeMap<_, _> = live.map(|it| (key(it), Verdict::Live)).collect();
        Self {
            stored: reached.clone(),
            reached,
            ..Self::default()
        }
    }

    /// Works out every verdict in the frontier, given in timeline order.
    /// Returns the transactions whose verdict changed to one storage has
    /// to reflect.
    pub(super) fn judge(
        &mut self,
        id: AgreementId,
        dropped: &BTreeSet<ModelId>,
        frontier: &[&Verified<Transaction>],
        outbox: &mut Outbox,
    ) -> Vec<(Verified<Transaction>, Verdict)> {
        let mut written = Written {
            dropped: dropped.clone(),
            ..Written::default()
        };
        let mut changed = Vec::new();
        let stored = std::mem::take(&mut self.stored);
        self.reached.clear();
        self.wanted.clear();

        for &transaction in frontier {
            let verdict = self.weigh(id, transaction, &written, outbox);
            let operations = &transaction.payload().operations;
            match verdict {
                Verdict::Live => written.apply(operations),
                Verdict::Pending => written.doubt(operations),
                Verdict::Pruned => {}
            }

            // While it's pending, storage keeps whatever it had.
            let key = key(transaction);
            let before = stored.get(&key).copied();
            let after = if verdict == Verdict::Pending {
                before
            } else {
                Some(verdict)
            };

            if before != after {
                changed.push((transaction.clone(), verdict));
            }

            self.reached.insert(key, verdict);
            self.stored.extend(after.map(|verdict| (key, verdict)));
        }

        self.settled.retain(|place, _| self.wanted.contains(place));

        changed
    }

    /// Whether a transaction's verdict can't be worked out yet. It can't
    /// settle until it can.
    pub(super) fn is_pending(&self, transaction: &Verified<Transaction>) -> bool {
        let verdict = self.reached.get(&key(transaction));

        !matches!(verdict, Some(Verdict::Live | Verdict::Pruned))
    }

    pub(super) fn is_live(&self, transaction: &Verified<Transaction>) -> bool {
        self.reached.get(&key(transaction)) == Some(&Verdict::Live)
    }

    /// A read came back: whether `place` holds a settled value.
    pub(super) fn got(&mut self, model: ModelId, key: Vec<u8>, holds: bool) {
        let place = (model, key);
        if self.reading.remove(&place) == Some(false) {
            self.settled.entry(place).or_insert(holds);
        }
    }

    /// A live transaction settled, and this is one of its writes.
    pub(super) fn folded(&mut self, write: &Write) {
        let place = (write.model, write.key.clone());
        if let Some(stale) = self.reading.get_mut(&place) {
            *stale = true;
        }

        if self.wanted.contains(&place) {
            self.settled.insert(place, write.value.is_some());
        }
    }

    /// A snapshot replaced everything settled. Nothing read before it
    /// can be trusted.
    pub(super) fn absorbed(&mut self) {
        self.settled.clear();
        for stale in self.reading.values_mut() {
            *stale = true;
        }
    }

    /// One transaction's verdict, given what the frontier wrote before
    /// it. Its own operations apply in order, so it can meet a
    /// requirement itself, or break one.
    fn weigh(
        &mut self,
        id: AgreementId,
        transaction: &Verified<Transaction>,
        written: &Written,
        outbox: &mut Outbox,
    ) -> Verdict {
        let operations = &transaction.payload().operations;
        if !operations.iter().all(Operation::belongs) {
            return Verdict::Pruned;
        }

        let mut own: BTreeMap<Place, bool> = BTreeMap::new();
        let mut own_drops = BTreeSet::new();
        let mut pending = false;
        for operation in operations {
            let model = operation.model;
            if written.dropped.contains(&model) || own_drops.contains(&model) {
                return Verdict::Pruned;
            }

            pending |= written.unsure_models.contains(&model);
            let Some(write) = operation.write() else {
                own_drops.insert(model);
                continue;
            };

            let place = (write.model, write.key);
            if let Some(Requirement::Vacant) = operation.requires() {
                let holds = match own.get(&place) {
                    Some(&holds) => Some(holds),
                    None => self.holds(id, &place, written, outbox),
                };

                match holds {
                    Some(true) => return Verdict::Pruned,
                    Some(false) => {}
                    None => pending = true,
                }
            }

            own.insert(place, write.value.is_some());
        }

        if pending {
            Verdict::Pending
        } else {
            Verdict::Live
        }
    }

    /// Whether `place` holds a value: what the frontier last wrote there,
    /// or else what has settled. `None` if that isn't known yet.
    fn holds(
        &mut self,
        id: AgreementId,
        place: &Place,
        written: &Written,
        outbox: &mut Outbox,
    ) -> Option<bool> {
        if written.unsure.contains(place) {
            return None;
        }

        if let Some(&holds) = written.holds.get(place) {
            return Some(holds);
        }

        self.wanted.insert(place.clone());
        if let Some(&holds) = self.settled.get(place) {
            return Some(holds);
        }

        if !self.reading.contains_key(place) {
            outbox.get(keys::settled_value(id, place.0, &place.1));
            self.reading.insert(place.clone(), false);
        }

        None
    }
}

impl Written {
    fn apply(&mut self, operations: &[Operation]) {
        for operation in operations {
            let Some(write) = operation.write() else {
                self.dropped.insert(operation.model);
                continue;
            };

            let place = (write.model, write.key);
            self.unsure.remove(&place);
            self.holds.insert(place, write.value.is_some());
        }
    }

    fn doubt(&mut self, operations: &[Operation]) {
        for operation in operations {
            match operation.write() {
                Some(write) => self.unsure.insert((write.model, write.key)),
                None => self.unsure_models.insert(operation.model),
            };
        }
    }
}

fn key(transaction: &Verified<Transaction>) -> (Peer, u64) {
    (transaction.author(), transaction.payload().seq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::Signed;
    use crate::agreement::Genesis;
    use crate::clock::VersionVector;
    use crate::testing::peers::{identity_of, laptop, nas, phone};

    fn agreement() -> AgreementId {
        Genesis::new([7; 32], [phone().id()]).id()
    }

    fn names() -> ModelId {
        ModelId::claims(agreement(), [1; 32])
    }

    fn profiles() -> ModelId {
        ModelId::kv(agreement(), [2; 32])
    }

    fn claim(key: &str) -> Operation {
        Operation::claim(names(), key.into(), b"mine".to_vec())
    }

    /// A transaction at `lamport` in the timeline.
    fn transaction(author: Peer, lamport: u64, operations: &[Operation]) -> Verified<Transaction> {
        let transaction = Transaction {
            agreement: agreement(),
            seq: lamport,
            after: VersionVector::default(),
            lamport,
            operations: operations.to_vec(),
        };

        Signed::sign(&identity_of(author), transaction)
    }

    /// Judges a frontier, answering every read with "nothing settled
    /// there". Returns each transaction's verdict, in timeline order.
    fn judge(verdicts: &mut Verdicts, frontier: &[&Verified<Transaction>]) -> Vec<Verdict> {
        let mut outbox = Outbox::default();
        verdicts.judge(agreement(), &BTreeSet::new(), frontier, &mut outbox);
        for place in verdicts.reading.clone().into_keys() {
            verdicts.got(place.0, place.1, false);
        }

        verdicts.judge(agreement(), &BTreeSet::new(), frontier, &mut outbox);
        frontier
            .iter()
            .map(|it| verdicts.reached[&key(it)])
            .collect()
    }

    #[test]
    fn the_first_claim_in_the_timeline_wins() {
        let first = transaction(phone(), 1, &[claim("ada")]);
        let second = transaction(laptop(), 2, &[claim("ada")]);
        let other = transaction(nas(), 3, &[claim("grace")]);

        assert_eq!(
            judge(&mut Verdicts::default(), &[&first, &second, &other]),
            [Verdict::Live, Verdict::Pruned, Verdict::Live],
        );
    }

    #[test]
    fn a_pruned_transaction_takes_all_of_its_writes_with_it() {
        let first = transaction(phone(), 1, &[claim("ada")]);
        let loses = transaction(laptop(), 2, &[claim("grace"), claim("ada")]);
        let third = transaction(nas(), 3, &[claim("grace")]);

        // The laptop never got "grace", so the nas can have it.
        assert_eq!(
            judge(&mut Verdicts::default(), &[&first, &loses, &third]),
            [Verdict::Live, Verdict::Pruned, Verdict::Live],
        );
    }

    #[test]
    fn a_late_arrival_changes_the_verdicts_after_it() {
        let mut verdicts = Verdicts::default();
        let late = transaction(phone(), 1, &[claim("ada")]);
        let second = transaction(laptop(), 2, &[claim("ada"), claim("grace")]);
        let third = transaction(nas(), 3, &[claim("grace")]);
        assert_eq!(
            judge(&mut verdicts, &[&second, &third]),
            [Verdict::Live, Verdict::Pruned],
        );

        let mut outbox = Outbox::default();
        let frontier = [&late, &second, &third];
        let changed = verdicts.judge(agreement(), &BTreeSet::new(), &frontier, &mut outbox);

        assert_eq!(
            changed,
            [
                (late.clone(), Verdict::Live),
                (second.clone(), Verdict::Pruned),
                (third.clone(), Verdict::Live),
            ],
        );
    }

    #[test]
    fn a_claim_waits_to_hear_what_has_settled() {
        let mut verdicts = Verdicts::default();
        let mut outbox = Outbox::default();
        let wants = transaction(phone(), 1, &[claim("ada")]);
        let after = transaction(laptop(), 2, &[claim("ada")]);

        // Nobody knows who has "ada" until storage answers, so nobody
        // knows what either transaction's verdict is.
        let changed = verdicts.judge(
            agreement(),
            &BTreeSet::new(),
            &[&wants, &after],
            &mut outbox,
        );
        assert_eq!(changed, []);
        assert!(verdicts.is_pending(&wants));
        assert!(verdicts.is_pending(&after));

        // Taken, long ago.
        verdicts.got(names(), b"ada".to_vec(), true);
        let changed = verdicts.judge(
            agreement(),
            &BTreeSet::new(),
            &[&wants, &after],
            &mut outbox,
        );
        assert_eq!(
            changed,
            [(wants, Verdict::Pruned), (after, Verdict::Pruned)]
        );
    }

    #[test]
    fn a_settled_claim_is_remembered_without_reading_again() {
        let mut verdicts = Verdicts::default();
        let first = transaction(phone(), 1, &[claim("ada")]);
        let second = transaction(laptop(), 2, &[claim("ada")]);
        judge(&mut verdicts, &[&first, &second]);

        for write in first
            .payload()
            .operations
            .iter()
            .filter_map(Operation::write)
        {
            verdicts.folded(&write);
        }

        assert_eq!(judge(&mut verdicts, &[&second]), [Verdict::Pruned]);
        assert!(verdicts.reading.is_empty());
    }

    /// The answer to a read describes storage when it was asked. If the
    /// key settles in between, the answer is wrong by the time it's here.
    #[test]
    fn an_answer_from_before_a_key_settled_is_ignored() {
        let mut verdicts = Verdicts::default();
        let mut outbox = Outbox::default();
        let second = transaction(laptop(), 2, &[claim("ada")]);
        verdicts.judge(agreement(), &BTreeSet::new(), &[&second], &mut outbox);

        verdicts.absorbed();
        verdicts.got(names(), b"ada".to_vec(), false);
        verdicts.judge(agreement(), &BTreeSet::new(), &[&second], &mut outbox);

        assert!(verdicts.is_pending(&second));
        assert_eq!(
            verdicts.reading.get(&(names(), b"ada".to_vec())),
            Some(&false)
        );
    }

    #[test]
    fn an_operation_that_isnt_its_models_prunes_the_transaction() {
        let set = Operation::set(names(), b"ada".to_vec(), b"taken".to_vec());
        let profile = Operation::set(profiles(), b"ada".to_vec(), b"hello".to_vec());
        let wrong = transaction(phone(), 1, &[profile.clone(), set]);
        let right = transaction(laptop(), 2, &[profile]);

        assert_eq!(
            judge(&mut Verdicts::default(), &[&wrong, &right]),
            [Verdict::Pruned, Verdict::Live],
        );
    }

    #[test]
    fn a_dropped_model_takes_nothing_more() {
        let mut verdicts = Verdicts::default();
        let profile = Operation::set(profiles(), b"ada".to_vec(), b"hello".to_vec());
        let before = transaction(phone(), 1, std::slice::from_ref(&profile));
        let drops = transaction(laptop(), 2, &[Operation::drop(profiles())]);
        let after = transaction(nas(), 3, &[claim("ada"), profile.clone()]);
        assert_eq!(
            judge(&mut verdicts, &[&before, &drops, &after]),
            [Verdict::Live, Verdict::Live, Verdict::Pruned],
        );

        // Still dead once the drop has settled and left the frontier.
        let mut outbox = Outbox::default();
        let dropped = BTreeSet::from([profiles()]);
        verdicts.judge(agreement(), &dropped, &[&after], &mut outbox);
        assert!(!verdicts.is_live(&after));
        assert!(!verdicts.is_pending(&after));
    }

    #[test]
    fn transactions_that_require_nothing_are_live_after_a_restart() {
        let profile = Operation::set(profiles(), b"ada".to_vec(), b"hello".to_vec());
        let plain = transaction(phone(), 1, &[profile]);
        let claims = transaction(laptop(), 2, &[claim("ada")]);
        let mut verdicts = Verdicts::resume(&[&plain, &claims]);

        assert!(verdicts.is_live(&plain));
        assert!(verdicts.is_pending(&claims));

        // Only the claim is written again.
        let mut outbox = Outbox::default();
        verdicts.judge(
            agreement(),
            &BTreeSet::new(),
            &[&plain, &claims],
            &mut outbox,
        );
        verdicts.got(names(), b"ada".to_vec(), false);
        let changed = verdicts.judge(
            agreement(),
            &BTreeSet::new(),
            &[&plain, &claims],
            &mut outbox,
        );
        assert_eq!(changed, [(claims, Verdict::Live)]);
    }
}
