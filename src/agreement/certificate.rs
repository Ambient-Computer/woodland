//! Proof that a decision was learned, for whoever wasn't there.
//!
//! A replica learns when a quorum of every configuration in play attests
//! to its exact view. Those attestations are signed, so the view and the
//! attestations together are something anyone can check, and it doesn't
//! matter who hands them over. A device with no write access can carry a
//! decision between contributors that never meet, and can't make one up.
//!
//! # Why checking against our own configuration is enough
//!
//! A certificate names the configuration its view had learned (its
//! `base`), and is only taken if ours includes that one. Then everyone
//! in the base was a contributor as far as we know, and its quorum has
//! at least one of them in it. Contributors are trusted to attest only
//! to views they hold, and they hold only what was learned. So the base
//! really was learned, the quorums counted really were the ones in play,
//! and the certificate records exactly what a replica holding that view
//! would have needed to learn the decision itself.
//!
//! A replica that's several configurations behind can't check the
//! latest certificate, because it doesn't know those contributors yet.
//! So replicas keep one certificate for each change of configuration,
//! and pass on the whole chain. Each one introduces the contributors
//! that vouch for the next.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::view::Acknowledged;
use super::{AgreementId, Attestation, Configuration, Decision, View};
use crate::actor::{self, Peer, Signed, Verified};
use crate::clock::Progress;

/// A view, and the attestations of those who held it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Certificate {
    /// The configuration the view had learned.
    base: Configuration,
    proposals: BTreeSet<Configuration>,
    candidate: Progress,
    acknowledged: Acknowledged,
    attestations: Vec<Signed<Attestation>>,
}

impl Certificate {
    /// Records that `witnesses` held `view`. Only the ones attesting to
    /// this exact view are kept.
    pub(crate) fn of<'a>(
        view: &View,
        witnesses: impl IntoIterator<Item = &'a Verified<Attestation>>,
    ) -> Self {
        let digest = view.digest();
        let matching = witnesses
            .into_iter()
            .filter(|witness| witness.payload().digest == digest);

        Self {
            base: view.learned.configuration.clone(),
            proposals: view.proposals.clone(),
            candidate: view.candidate.clone(),
            acknowledged: view.acknowledged.clone(),
            attestations: matching.map(|it| it.signed().clone()).collect(),
        }
    }

    /// The configuration whose contributors must be known before this
    /// can be checked.
    pub(crate) fn base(&self) -> &Configuration {
        &self.base
    }

    /// What was learned, if the certificate holds.
    pub(crate) fn decision(&self) -> Decision {
        self.view().pending()
    }

    /// Whether a quorum of every configuration in play attests to the
    /// view. This is the expensive part: it checks signatures.
    pub(crate) fn holds(&self, agreement: AgreementId) -> bool {
        let view = self.view();
        let digest = view.digest();
        let matching = self.attestations.iter().filter(|attestation| {
            let claim = attestation.payload();
            claim.agreement == agreement && claim.digest == digest
        });

        let verified = actor::verify_all(matching.cloned().collect());
        let witnesses: BTreeSet<Peer> = verified.iter().map(Verified::author).collect();

        // The base first. There's a configuration in play for every
        // subset of the proposals, and it's not worth working them all
        // out for a view that no contributor attests to.
        self.base.has_quorum(&witnesses)
            && view
                .configurations_in_play()
                .iter()
                .all(|configuration| configuration.has_quorum(&witnesses))
    }

    /// The view that was witnessed. What it had learned besides the
    /// configuration isn't part of what's attested, so it's left out.
    fn view(&self) -> View {
        let mut view = View::new(self.base.clone());
        view.proposals.clone_from(&self.proposals);
        view.candidate.clone_from(&self.candidate);
        view.acknowledged.clone_from(&self.acknowledged);

        view
    }
}
