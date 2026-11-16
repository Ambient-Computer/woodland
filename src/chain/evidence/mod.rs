//! Evidence: two statements one signer couldn't both have made.
//!
//! Agreement holds because each signer keeps its word: one checkpoint
//! per attempt, and no vote in an attempt it promised against. A signer
//! that breaks either has signed the proof itself. The two statements
//! need no witness, and anyone who holds the slot they're about can
//! check them.
//!
//! This convicts a signer that lost its store as readily as one that
//! lied. From outside they're the same, and either way its word is no
//! longer worth counting.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Digest, Promise, Proposal, Stated, Vote};
use crate::actor::{Peer, Signable, Signed, Verified, verify_all};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Evidence {
    /// Two votes in one attempt, for different checkpoints.
    Votes(Signed<Vote>, Signed<Vote>),
    /// Two checkpoints named for one attempt.
    Proposals(Signed<Proposal>, Signed<Proposal>),
    /// A vote in an attempt lower than one it promised, that the
    /// promise doesn't account for.
    Promise(Signed<Promise>, Signed<Vote>),
}

/// What evidence shows, once its signatures hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Conflict {
    pub(crate) author: Peer,
    /// The slot both statements are about.
    pub(crate) slot: Digest,
    /// Whether the statements are a voter's, which mean something only
    /// from someone with a say. Anyone with a role may name a
    /// checkpoint.
    pub(crate) voter: bool,
}

impl Evidence {
    /// Checks the evidence on its own: both statements are signed by
    /// one key, about one slot, and no honest signer makes both.
    pub(crate) fn check(&self) -> Option<Conflict> {
        let (author, slot, conflicting) = match self {
            Self::Votes(first, second) => {
                let [first, second] = checked([first, second])?;
                let (author, a, b) = (first.author(), first.payload(), second.payload());
                let conflicting = a.ballot == b.ballot && a.checkpoint != b.checkpoint;

                (author, a.slot, a.slot == b.slot && conflicting)
            }

            Self::Proposals(first, second) => {
                let [first, second] = checked([first, second])?;
                let (author, a, b) = (first.author(), first.payload(), second.payload());
                let conflicting = a.ballot == b.ballot && a.checkpoint != b.checkpoint;

                (author, a.slot, a.slot == b.slot && conflicting)
            }

            Self::Promise(promise, vote) => {
                let [promise] = checked([promise])?;
                let [vote] = checked([vote])?;
                let same = promise.author() == vote.author();
                let broken = breaks(promise.payload(), vote.payload());

                (promise.author(), vote.payload().slot, same && broken)
            }
        };

        let voter = !matches!(self, Self::Proposals(..));

        conflicting.then_some(Conflict {
            author,
            slot,
            voter,
        })
    }
}

/// Verifies statements that must all be one signer's.
fn checked<T: Signable + Clone, const N: usize>(
    statements: [&Signed<T>; N],
) -> Option<[Verified<T>; N]> {
    let author = statements.first()?.author();
    if statements.iter().any(|it| it.author() != author) {
        return None;
    }

    let signed = statements.into_iter().cloned().collect();

    <[_; N]>::try_from(verify_all(signed)).ok()
}

/// Whether `vote` is one a signer that made `promise` couldn't have
/// cast: in a lower attempt, and not the vote the promise reported or
/// one before it. It was either cast after the promise or left out.
fn breaks(promise: &Promise, vote: &Vote) -> bool {
    let unreported = match &promise.accepted {
        None => true,
        Some((ballot, checkpoint)) => {
            vote.ballot > *ballot
                || (vote.ballot == *ballot && vote.checkpoint != checkpoint.digest())
        }
    };

    promise.slot == vote.slot && vote.ballot < promise.ballot && unreported
}

/// Looks through everything said, by anyone, for two statements one
/// signer couldn't both have made. At most one finding per signer.
pub(super) fn find(
    promises: &[&Signed<Promise>],
    proposals: &[&Signed<Proposal>],
    votes: &[&Signed<Vote>],
) -> Vec<Evidence> {
    let mut found: BTreeMap<Peer, Evidence> = BTreeMap::new();
    let twice = |a: &Vote, b: &Vote| same_attempt(a, b) && a.checkpoint != b.checkpoint;
    for (first, second) in pairs(votes) {
        if twice(first.payload(), second.payload()) {
            let evidence = Evidence::Votes(first.clone(), second.clone());
            found.entry(first.author()).or_insert(evidence);
        }
    }

    let twice = |a: &Proposal, b: &Proposal| same_attempt(a, b) && a.checkpoint != b.checkpoint;
    for (first, second) in pairs(proposals) {
        if twice(first.payload(), second.payload()) {
            let evidence = Evidence::Proposals(first.clone(), second.clone());
            found.entry(first.author()).or_insert(evidence);
        }
    }

    for promise in promises {
        let theirs = votes.iter().filter(|it| it.author() == promise.author());
        let mut broken = theirs.filter(|vote| breaks(promise.payload(), vote.payload()));
        if let Some(vote) = broken.next() {
            let evidence = Evidence::Promise((*promise).clone(), (*vote).clone());
            found.entry(promise.author()).or_insert(evidence);
        }
    }

    found.into_values().collect()
}

fn same_attempt<T: Stated>(a: &T, b: &T) -> bool {
    a.slot() == b.slot() && a.ballot() == b.ballot()
}

/// Every two statements by one signer.
fn pairs<'a, T>(
    statements: &'a [&'a Signed<T>],
) -> impl Iterator<Item = (&'a Signed<T>, &'a Signed<T>)>
where
    T: Signable,
{
    let later = |(index, first): (usize, &&'a Signed<T>)| {
        let rest = statements.iter().skip(index + 1);
        let first: &'a Signed<T> = first;

        rest.filter(move |it| it.author() == first.author())
            .map(move |second| (first, *second))
    };

    statements.iter().enumerate().flat_map(later)
}

#[cfg(test)]
mod tests;
