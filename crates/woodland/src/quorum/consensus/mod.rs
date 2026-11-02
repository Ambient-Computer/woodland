//! How contributors agree on the next commit: classic Paxos, run once per
//! epoch. Any contributor may propose.

mod acceptor;
mod certificate;
mod message;
mod proposer;
mod sender;
mod tally;

pub use acceptor::{Acceptor, Refusal};
pub use certificate::{Certificate, InvalidCertificate};
pub use message::{Accept, Accepted, Ballot, Message, Prepare, Promise, Proposal, Rejected};
pub use proposer::{Phase, Proposer};
pub use sender::Error;
pub use tally::Tally;
