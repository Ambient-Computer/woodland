//! How contributors agree on the next commit: classic Paxos, run once per
//! epoch. Any contributor may propose.

mod message;
mod role;
mod sender;

pub use message::{Accept, Accepted, Ballot, Certificate, Message, Prepare, Promise, Proposal};
pub use role::{Acceptor, Phase, Proposer};
pub use sender::Error;
