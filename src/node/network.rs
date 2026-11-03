use std::collections::{BTreeMap, BTreeSet};

use crate::actor::Peer;
use crate::agreement::AgreementId;

/// This device's view of the network.
///
/// Ordered maps, not hash maps, so a batch's sends always come out in the
/// same order. Deterministic output makes the state machine testable.
#[derive(Debug, Default)]
pub(crate) struct Network {
    /// Peers with an open connection to this device.
    connected: BTreeMap<Peer, Connection>,
}

#[derive(Debug, Default)]
struct Connection {
    /// The agreements the peer said it replicates.
    replicates: BTreeSet<AgreementId>,
    /// The agreements we last told the peer about, if we have yet.
    announced: Option<BTreeSet<AgreementId>>,
}

impl Network {
    pub(crate) fn connect(&mut self, peer: Peer) {
        self.connected.entry(peer).or_default();
    }

    pub(crate) fn disconnect(&mut self, peer: &Peer) {
        self.connected.remove(peer);
    }

    pub(crate) fn connected(&self) -> impl Iterator<Item = Peer> + '_ {
        self.connected.keys().copied()
    }

    /// The peer said hello. Ignored unless it's connected.
    pub(crate) fn heard_hello(&mut self, peer: Peer, replicates: BTreeSet<AgreementId>) {
        if let Some(connection) = self.connected.get_mut(&peer) {
            connection.replicates = replicates;
        }
    }

    /// Connected peers that replicate `agreement`.
    pub(crate) fn subscribers(&self, agreement: AgreementId) -> BTreeSet<Peer> {
        self.connected
            .iter()
            .filter(|(_, connection)| connection.replicates.contains(&agreement))
            .map(|(&peer, _)| peer)
            .collect()
    }

    /// Our hello for `peer`, if it hasn't heard about `ours` yet.
    pub(crate) fn hello_for(
        &mut self,
        peer: Peer,
        ours: &BTreeSet<AgreementId>,
    ) -> Option<BTreeSet<AgreementId>> {
        let connection = self.connected.get_mut(&peer)?;
        if connection.announced.as_ref() == Some(ours) {
            return None;
        }

        connection.announced = Some(ours.clone());
        Some(ours.clone())
    }
}
