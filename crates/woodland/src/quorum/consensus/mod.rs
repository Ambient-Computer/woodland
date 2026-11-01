//! How contributors agree on the next commit: classic Paxos, run once per
//! epoch. Any contributor may propose.

mod certificate;
mod message;
mod role;
mod sender;
mod tally;

pub use certificate::{Certificate, InvalidCertificate};
pub use message::{Accept, Accepted, Ballot, Message, Prepare, Promise, Proposal};
pub use role::{Acceptor, Phase, Proposer};
pub use sender::Error;
pub use tally::Tally;
