//! Requests to change who's in the group.
//!
//! A change happens when a checkpoint carries it, and whoever proposes
//! the checkpoint decides what it carries. So a member that wants a
//! change says so, signed, and that travels like everything else: the
//! member needn't be the one to propose, or be around when someone
//! does.

use crate::actor::{Identity, Peer, Signed, verify_all};
use crate::chain::Electorate;

use super::super::message::Request;
use super::Group;

impl Group {
    /// Whether `request`, made by `author`, could still be carried out:
    /// its author has a say, it leaves a group that can carry on, and
    /// the group is made up as it was when the request was made.
    pub(super) fn stands(&self, author: Peer, request: &Request) -> bool {
        let tip = self.chain.tip();
        let sealable = !self.is_sealed() || request.electorate.is_sealable();
        let mut since = self.chain.electorates_since(request.since);

        request.group == self.id
            && request.since <= tip.height
            && tip.electorate.includes(author)
            && request.electorate.is_viable()
            && sealable
            && since.all(|electorate| *electorate == tip.electorate)
    }

    /// Signs a request of this device's own.
    pub(super) fn request(&mut self, identity: &Identity, electorate: Electorate) {
        let request = Request {
            group: self.id,
            since: self.chain.tip().height,
            electorate,
        };
        let signed = Signed::sign(identity, request);
        self.requests
            .insert(identity.peer(), signed.signed().clone());
    }

    /// Takes in what a peer passed on. Each member has one request at
    /// a time: its latest.
    pub(super) fn hear_requests(&mut self, from: Peer, requests: Vec<Signed<Request>>) {
        for request in verify_all(requests) {
            let author = request.author();
            if let Some(remote) = self.remotes.get_mut(&from) {
                remote.asked(author);
            }

            let held = self.requests.get(&author);
            let newer = held.is_none_or(|held| held.payload().since < request.payload().since);
            if newer && self.stands(author, request.payload()) {
                self.requests.insert(author, request.signed().clone());
                self.said = true;
            }
        }
    }

    /// What the group should be made up of next, by this device's own
    /// request or, failing that, someone else's.
    pub(super) fn asked_for(&self) -> Option<Electorate> {
        let standing = |(author, request): (&Peer, &Signed<Request>)| {
            let request = request.payload();

            self.stands(*author, request)
                .then(|| request.electorate.clone())
        };

        self.wanted
            .clone()
            .or_else(|| self.requests.iter().find_map(standing))
    }
}
