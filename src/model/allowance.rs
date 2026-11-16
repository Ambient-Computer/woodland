//! Allowances: a quantity several devices spend from, without asking
//! each other and without ever spending more than there is.
//!
//! Stock, seats, credit. A number in a cell can't do it: two devices
//! that each take the last one both succeed until they hear of each
//! other, and then one of them is told it didn't happen, perhaps days
//! after it told someone it had.
//!
//! # The rule
//!
//! A quantity is split up front. Each contributor holds an allowance
//! of it, and spends only its own. An allowance is written by nobody
//! but its owner, so the owner always knows what's left, offline or
//! not, and a spend it makes within that happens whatever anyone else
//! does. The total is never exceeded because no allowance is.
//!
//! # Moving it
//!
//! Whoever opens a quantity holds all of it. Giving some to another
//! contributor takes it out of the giver's allowance and leaves a
//! gift for the other, which becomes part of theirs when they accept
//! it. Two steps, because one would have two devices writing to the
//! same allowance: the giver can't add to another's without getting
//! in the way of what its owner is spending.
//!
//! All of it is ordinary guarded transactions. A spend names what its
//! author's allowance was and requires it to be exactly that. Only the
//! author changes it, in the order it wrote, so the requirement fails
//! only if something of the author's own before it didn't happen:
//! say, it accepted a gift whose giving was pruned. Then what it
//! spent out of that doesn't happen either, and nothing is overdrawn.
//!
//! # Who may
//!
//! Every operation names the allowance it takes from, and only its
//! owner's are taken: one that names another device's is turned down
//! by everyone.

use std::collections::BTreeMap;

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

use super::{Fingerprint, Requirement};
use crate::actor::Peer;
use crate::encoding;

/// Tells one gift from another to the same device.
type GiftId = [u8; 16];

/// A gift, and how much it's of.
type Gift = (GiftId, u64);

/// The ID a gift has before its transaction is written.
const UNSEALED: GiftId = [0; 16];

const TOTAL: u8 = 0;
const BALANCE: u8 = 1;
const GIFT: u8 = 2;

fn total(quantity: &[u8]) -> Vec<u8> {
    [&[TOTAL][..], quantity].concat()
}

fn balance(owner: Peer, quantity: &[u8]) -> Vec<u8> {
    [&[BALANCE][..], owner.as_bytes(), quantity].concat()
}

fn gift(to: Peer, id: GiftId, quantity: &[u8]) -> Vec<u8> {
    [&[GIFT][..], to.as_bytes(), &id, quantity].concat()
}

fn amount(amount: u64) -> Vec<u8> {
    amount.to_be_bytes().to_vec()
}

fn is(held: u64) -> Requirement {
    Requirement::Is(Fingerprint::of(&amount(held)))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Operation {
    /// Starts a quantity, all of it in its author's allowance.
    Open {
        quantity: Vec<u8>,
        total: u64,
        owner: Peer,
    },
    /// Uses some up.
    Spend {
        quantity: Vec<u8>,
        owner: Peer,
        /// What the allowance was, as its owner read it.
        had: u64,
        amount: u64,
    },
    /// Moves some out of the author's allowance, for another device to
    /// accept. The gift's ID is filled in when the transaction is
    /// written.
    Give {
        quantity: Vec<u8>,
        owner: Peer,
        had: u64,
        amount: u64,
        to: Peer,
        gift: GiftId,
    },
    /// Takes a gift into the author's allowance.
    Accept {
        quantity: Vec<u8>,
        owner: Peer,
        /// `None` if it had no allowance yet.
        had: Option<u64>,
        amount: u64,
        gift: GiftId,
    },
}

impl Operation {
    fn owner(&self) -> Peer {
        match self {
            Self::Open { owner, .. }
            | Self::Spend { owner, .. }
            | Self::Give { owner, .. }
            | Self::Accept { owner, .. } => *owner,
        }
    }

    /// Whether `author` may: an allowance is its owner's alone.
    pub(crate) fn permits(&self, author: Peer) -> bool {
        self.owner() == author
    }

    /// Whether the sums come out: nothing is spent that wasn't there.
    pub(crate) fn belongs(&self) -> bool {
        match self {
            Self::Open { .. } => true,
            Self::Spend { had, amount, .. } | Self::Give { had, amount, .. } => amount <= had,
            Self::Accept { had, amount, .. } => had.unwrap_or(0).checked_add(*amount).is_some(),
        }
    }

    /// Gives a gift an ID no other has, because no other is this
    /// operation of this transaction.
    pub(crate) fn seal(&mut self, author: Peer, seq: u64, index: usize) {
        if let Self::Give { gift, .. } = self {
            let hash = encoding::hash("woodland/gift", &(author, seq, index));
            gift.copy_from_slice(&hash[..16]);
        }
    }

    pub(crate) fn requires(&self) -> Vec<(Vec<u8>, Requirement)> {
        match self {
            Self::Open {
                quantity, owner, ..
            } => vec![
                (total(quantity), Requirement::Vacant),
                (balance(*owner, quantity), Requirement::Vacant),
            ],

            Self::Spend {
                quantity,
                owner,
                had,
                ..
            } => vec![(balance(*owner, quantity), is(*had))],

            Self::Give {
                quantity,
                owner,
                had,
                to,
                gift: id,
                ..
            } => vec![
                (balance(*owner, quantity), is(*had)),
                (gift(*to, *id, quantity), Requirement::Vacant),
            ],

            Self::Accept {
                quantity,
                owner,
                had,
                amount,
                gift: id,
            } => vec![
                (gift(*owner, *id, quantity), is(*amount)),
                (
                    balance(*owner, quantity),
                    had.map_or(Requirement::Vacant, is),
                ),
            ],
        }
    }

    pub(crate) fn writes(&self) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
        match self {
            Self::Open {
                quantity,
                total: all,
                owner,
            } => vec![
                (total(quantity), Some(amount(*all))),
                (balance(*owner, quantity), Some(amount(*all))),
            ],

            Self::Spend {
                quantity,
                owner,
                had,
                amount: spent,
            } => vec![(
                balance(*owner, quantity),
                Some(amount(had.saturating_sub(*spent))),
            )],

            Self::Give {
                quantity,
                owner,
                had,
                amount: given,
                to,
                gift: id,
            } => vec![
                (
                    balance(*owner, quantity),
                    Some(amount(had.saturating_sub(*given))),
                ),
                (gift(*to, *id, quantity), Some(amount(*given))),
            ],

            Self::Accept {
                quantity,
                owner,
                had,
                amount: given,
                gift: id,
            } => vec![
                (gift(*owner, *id, quantity), None),
                (
                    balance(*owner, quantity),
                    Some(amount(had.unwrap_or(0).saturating_add(*given))),
                ),
            ],
        }
    }
}

/// What an allowances model holds, decoded from a read of all of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Allowances {
    totals: BTreeMap<Vec<u8>, u64>,
    balances: BTreeMap<(Vec<u8>, Peer), u64>,
    gifts: BTreeMap<(Vec<u8>, Peer), Vec<Gift>>,
}

/// One device's allowance of one quantity, as it was read: what a
/// transaction that spends from it goes by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allowance {
    quantity: Vec<u8>,
    owner: Peer,
    balance: Option<u64>,
    gifts: Vec<Gift>,
}

impl Allowances {
    /// Decodes the results of a query for everything in the model.
    #[must_use]
    pub fn read(results: &[(Vec<u8>, Vec<u8>)]) -> Self {
        let mut read = Self::default();
        for (key, value) in results {
            let Some((&kind, rest)) = key.split_first() else {
                continue;
            };
            let Some(held) = value.as_slice().try_into().ok().map(u64::from_be_bytes) else {
                continue;
            };

            let owned = |rest: &[u8]| {
                let (owner, rest) = rest.split_first_chunk::<32>()?;

                Some((Peer::from_bytes(owner)?, rest.to_vec()))
            };
            match kind {
                TOTAL => {
                    read.totals.insert(rest.to_vec(), held);
                }

                BALANCE => {
                    if let Some((owner, quantity)) = owned(rest) {
                        read.balances.insert((quantity, owner), held);
                    }
                }

                GIFT => {
                    let Some((to, rest)) = owned(rest) else {
                        continue;
                    };
                    if let Some((id, quantity)) = rest.split_first_chunk::<16>() {
                        let gifts = read.gifts.entry((quantity.to_vec(), to)).or_default();
                        gifts.push((*id, held));
                    }
                }

                _ => {}
            }
        }

        read
    }

    /// How much of `quantity` there was to begin with. `None` if
    /// nobody has opened it.
    #[must_use]
    pub fn total(&self, quantity: impl AsRef<[u8]>) -> Option<u64> {
        self.totals.get(quantity.as_ref()).copied()
    }

    /// How much of `quantity` nobody has spent: every allowance, and
    /// every gift not yet accepted.
    #[must_use]
    pub fn left(&self, quantity: impl AsRef<[u8]>) -> u64 {
        let quantity = quantity.as_ref();
        let held = self.balances.iter().filter(|((it, _), _)| it == quantity);
        let held: u64 = held.map(|(_, amount)| amount).sum();
        let given = self.gifts.iter().filter(|((it, _), _)| it == quantity);
        let given: u64 = given.flat_map(|(_, gifts)| gifts).map(|gift| gift.1).sum();

        held.saturating_add(given)
    }

    /// `owner`'s allowance of `quantity`, to read or to spend from.
    #[must_use]
    pub fn of(&self, quantity: impl AsRef<[u8]>, owner: VerifyingKey) -> Allowance {
        let key = (quantity.as_ref().to_vec(), Peer::from(owner));

        Allowance {
            balance: self.balances.get(&key).copied(),
            gifts: self.gifts.get(&key).cloned().unwrap_or_default(),
            quantity: key.0,
            owner: key.1,
        }
    }
}

impl Allowance {
    /// How much its owner can spend.
    #[must_use]
    pub fn balance(&self) -> u64 {
        self.balance.unwrap_or(0)
    }

    /// How much its owner has been given and hasn't accepted.
    #[must_use]
    pub fn offered(&self) -> u64 {
        self.gifts.iter().map(|gift| gift.1).sum()
    }

    pub(crate) fn open(quantity: Vec<u8>, total: u64, owner: Peer) -> Operation {
        Operation::Open {
            quantity,
            total,
            owner,
        }
    }

    pub(crate) fn spend(&self, amount: u64) -> Operation {
        Operation::Spend {
            quantity: self.quantity.clone(),
            owner: self.owner,
            had: self.balance(),
            amount,
        }
    }

    pub(crate) fn give(&self, to: Peer, amount: u64) -> Operation {
        Operation::Give {
            quantity: self.quantity.clone(),
            owner: self.owner,
            had: self.balance(),
            amount,
            to,
            gift: UNSEALED,
        }
    }

    /// One operation for each gift, each going by what the one before
    /// it leaves.
    pub(crate) fn accept(&self) -> Vec<Operation> {
        let mut had = self.balance;
        let accept = |&(gift, amount): &(GiftId, u64)| {
            let operation = Operation::Accept {
                quantity: self.quantity.clone(),
                owner: self.owner,
                had,
                amount,
                gift,
            };
            had = Some(had.unwrap_or(0).saturating_add(amount));

            operation
        };

        self.gifts.iter().map(accept).collect()
    }
}
