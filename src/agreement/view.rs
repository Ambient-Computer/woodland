use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::Configuration;
use crate::clock::Progress;
use crate::encoding;
use crate::lattice::Lattice;

/// Everything one replica has heard about an agreement. This is what
/// replicas gossip, and merging is the only thing they ever do with it.
///
/// It follows the per-process state in Kuznetsov et al.'s RLA (Figure 2):
///
/// | Paper | Here         | Meaning                                       |
/// |-------|--------------|-----------------------------------------------|
/// | `v_p` | `learned`    | the greatest value known to be learned        |
/// | `obj_p` | `candidate` | the join of every value anyone has proposed |
/// | `T_p` | `proposals`  | proposed configurations not yet superseded   |
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct View {
    pub(crate) learned: Decision,
    pub(crate) candidate: Progress,
    pub(crate) proposals: BTreeSet<Configuration>,
}

/// A learned value: a contributor set and the progress reports agreed on
/// alongside it.
///
/// Every decision ever learned, by any replica, is comparable with every
/// other. That's lattice agreement's whole promise. It doesn't give a
/// single sequence of decisions, but it gives a chain: no two replicas ever
/// learn things that disagree.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Decision {
    pub(crate) progress: Progress,
    pub(crate) configuration: Configuration,
}

/// A hash of the parts of a [`View`] that a replica can witness.
///
/// Two replicas with the same digest hold the same candidate and the same
/// configurations, which is exactly what learning needs to count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Digest([u8; 32]);

impl View {
    pub(crate) fn new(genesis: Configuration) -> Self {
        Self {
            learned: Decision {
                progress: Progress::default(),
                configuration: genesis,
            },
            candidate: Progress::default(),
            proposals: BTreeSet::new(),
        }
    }

    /// Joins `other` into this view. Returns whether anything changed.
    pub(crate) fn merge(&mut self, other: &View) -> bool {
        let before = self.clone();
        self.learned.join(&other.learned);
        self.candidate.join(&other.candidate);
        self.proposals.extend(other.proposals.iter().cloned());
        self.forget_superseded();

        *self != before
    }

    /// Adds a proposed configuration, unless what's learned already
    /// includes it.
    pub(crate) fn propose(&mut self, configuration: Configuration) {
        self.proposals.insert(configuration);
        self.forget_superseded();
    }

    /// Adopts a new decision and drops the proposals it covers.
    pub(crate) fn learn(&mut self, decision: &Decision) {
        self.learned.join(decision);
        self.forget_superseded();
    }

    /// The configuration this replica would learn next: everything learned
    /// so far plus every pending proposal.
    pub(crate) fn target_configuration(&self) -> Configuration {
        let mut target = self.learned.configuration.clone();
        for proposal in &self.proposals {
            target.join(proposal);
        }

        target
    }

    /// Every configuration that might be in use, so every configuration
    /// whose quorum must agree before anything is learned.
    ///
    /// That's the learned configuration joined with each subset of the
    /// pending proposals. Another replica may have heard any subset of them,
    /// so it could be counting quorums in any of these configurations.
    /// Waiting on a quorum of each one guarantees an overlap with whichever
    /// one it uses.
    ///
    /// This is exponential in the number of pending proposals. The paper
    /// notes the same cost. Proposals clear as soon as they're learned, so in
    /// practice there are very few.
    pub(crate) fn configurations_in_play(&self) -> BTreeSet<Configuration> {
        let mut in_play = BTreeSet::from([self.learned.configuration.clone()]);
        for proposal in &self.proposals {
            let extended: Vec<_> = in_play
                .iter()
                .map(|configuration| {
                    let mut joined = configuration.clone();
                    joined.join(proposal);
                    joined
                })
                .collect();
            in_play.extend(extended);
        }

        in_play
    }

    pub(crate) fn digest(&self) -> Digest {
        let witnessed = (
            &self.candidate,
            &self.learned.configuration,
            &self.proposals,
        );

        Digest(encoding::hash("woodland/agreement/view", &witnessed))
    }

    /// A configuration is superseded once something learned includes it.
    /// Its quorum no longer matters, so stop waiting on it. This is how
    /// removed or long-offline devices stop blocking progress.
    fn forget_superseded(&mut self) {
        let learned = &self.learned.configuration;
        self.proposals.retain(|proposal| !proposal.leq(learned));
    }
}

impl Lattice for Decision {
    fn join(&mut self, other: &Self) -> bool {
        let progress = self.progress.join(&other.progress);
        let configuration = self.configuration.join(&other.configuration);

        progress || configuration
    }

    fn leq(&self, other: &Self) -> bool {
        self.progress.leq(&other.progress) && self.configuration.leq(&other.configuration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agreement::Change;
    use crate::testing::peers::{friend, laptop, nas, phone};

    fn household() -> Configuration {
        Configuration::founded_by([phone(), laptop(), nas()])
    }

    #[test]
    fn plays_every_combination_of_proposals() {
        let mut view = View::new(household());
        let with_friend = household().with(Change::Add(friend()));
        let without_nas = household().with(Change::Remove(nas()));
        view.propose(with_friend.clone());
        view.propose(without_nas.clone());

        let mut both = with_friend.clone();
        both.join(&without_nas);
        assert_eq!(
            view.configurations_in_play(),
            BTreeSet::from([household(), with_friend, without_nas, both]),
        );
    }

    #[test]
    fn learning_supersedes_proposals() {
        let mut view = View::new(household());
        let with_friend = household().with(Change::Add(friend()));
        view.propose(with_friend.clone());

        view.learn(&Decision {
            progress: Progress::default(),
            configuration: with_friend,
        });

        assert!(view.proposals.is_empty());
    }

    #[test]
    fn digest_ignores_learned_progress() {
        let mut view = View::new(household());
        let before = view.digest();
        view.learned.progress = Progress::report(phone(), [(phone(), 1)].into_iter().collect());

        assert_eq!(view.digest(), before);
    }
}
