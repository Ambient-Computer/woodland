//! How contributors agree on the next commit: classic Paxos, run once per
//! epoch. Any contributor may propose.

mod message;
mod role;

pub use message::{Accept, Accepted, Ballot, Prepare, Promise, Proposal};
pub use role::{Acceptor, Phase, Proposer};
