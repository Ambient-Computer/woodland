use std::collections::BTreeMap;

use proptest::prelude::*;

use super::*;
use crate::agreement::Decision;
use crate::lattice::Lattice;
use crate::log::Cut;

#[derive(Clone, Copy, Debug)]
enum Step {
    Write {
        device: usize,
        key: u8,
        value: u8,
    },
    /// Claims a name, and notes whose it is, in one transaction.
    Claim {
        device: usize,
        name: u8,
    },
    Connect {
        left: usize,
        right: usize,
    },
    Disconnect {
        left: usize,
        right: usize,
    },
    /// One device processes everything waiting for it.
    Deliver {
        device: usize,
    },
    Restart {
        device: usize,
    },
    AddFriend {
        device: usize,
    },
    RemoveNas {
        device: usize,
    },
}

fn step() -> impl Strategy<Value = Step> {
    let device = 0..4usize;
    prop_oneof![
        4 => (device.clone(), 0..3u8, any::<u8>())
            .prop_map(|(device, key, value)| Step::Write { device, key, value }),
        3 => (device.clone(), 0..3u8).prop_map(|(device, name)| Step::Claim { device, name }),
        2 => (device.clone(), device.clone())
            .prop_map(|(left, right)| Step::Connect { left, right }),
        2 => (device.clone(), device.clone())
            .prop_map(|(left, right)| Step::Disconnect { left, right }),
        6 => device.clone().prop_map(|device| Step::Deliver { device }),
        1 => device.clone().prop_map(|device| Step::Restart { device }),
        1 => (0..3usize).prop_map(|device| Step::AddFriend { device }),
        1 => (0..3usize).prop_map(|device| Step::RemoveNas { device }),
    ]
}

fn run(cluster: &mut Cluster, step: Step) {
    let agreement = genesis(cluster).id();
    match step {
        Step::Write { device, key, value } => {
            write(cluster, device, &key.to_string(), &value.to_string());
        }
        Step::Claim { device, name } => {
            claim(cluster, device, &name.to_string(), &device.to_string());
        }
        Step::Connect { left, right } => cluster.connect(left, right),
        Step::Disconnect { left, right } => cluster.disconnect(left, right),
        Step::Deliver { device } => cluster.deliver(device),
        Step::Restart { device } => cluster.restart(device),
        Step::AddFriend { device } => {
            let friend = cluster.key(FRIEND);
            cluster.run(device, |batch| batch.add_contributor(agreement, friend));
        }
        Step::RemoveNas { device } => {
            let nas = cluster.key(NAS);
            cluster.run(device, |batch| batch.remove_contributor(agreement, nas));
        }
    }
}

/// Everything the convergence test watches while the steps run.
#[derive(Default)]
struct Observed {
    /// Every decision any device learned.
    learned: Vec<Decision>,
    /// Every cut any device reached, and the settled state it held there.
    settled: BTreeMap<usize, Vec<(Cut, Stored)>>,
}

type Stored = Vec<(Vec<u8>, Vec<u8>)>;

impl Observed {
    fn observe(&mut self, cluster: &Cluster) {
        for device in [PHONE, LAPTOP, NAS, FRIEND] {
            let group = group(cluster, device);
            self.learned.push(group.agreement().learned().clone());

            let seen = self.settled.entry(device).or_default();
            let cut = group.log().cut().clone();
            if seen.last().is_none_or(|(last, _)| last != &cut) {
                seen.push((cut, stored_settled(cluster, device)));
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Whatever happened (partitions, crashes, concurrent writes and
    /// reconfigurations), once everyone reconnects they agree on
    /// everything: data, membership, and what storage holds. And at
    /// every step along the way, what had settled was the same history
    /// on every device.
    #[test]
    fn reconnected_devices_converge(steps in prop::collection::vec(step(), 0..60)) {
        let mut cluster = household();
        let mut observed = Observed::default();
        for step in steps {
            run(&mut cluster, step);
            observed.observe(&cluster);
        }

        cluster.connect_everyone();
        cluster.settle();
        observed.observe(&cluster);

        // Lattice agreement's promise holds end to end, through real
        // sync, storage and restarts: no two devices ever learned
        // decisions that disagree.
        for left in &observed.learned {
            for right in &observed.learned {
                prop_assert!(left.leq(right) || right.leq(left));
            }
        }

        // A device's settled history only ever grows.
        for cuts in observed.settled.values() {
            for pair in cuts.windows(2) {
                prop_assert!(pair[0].0.settled.leq(&pair[1].0.settled));
            }
        }

        // No two devices ever settled histories that disagree: one is
        // always a prefix of the other, and the same prefix always
        // leaves the same state.
        let cuts: Vec<_> = observed.settled.values().flatten().collect();
        for (left, left_stored) in &cuts {
            for (right, right_stored) in &cuts {
                prop_assert!(left.settled.leq(&right.settled) || right.settled.leq(&left.settled));
                if left.settled == right.settled {
                    prop_assert_eq!(left_stored, right_stored);
                }
            }
        }

        let reference = group(&cluster, PHONE);
        for device in [LAPTOP, NAS, FRIEND] {
            let other = group(&cluster, device);
            prop_assert_eq!(
                stored_settled(&cluster, device),
                stored_settled(&cluster, PHONE),
                "device {}", device,
            );
            prop_assert_eq!(
                stored_frontier(&cluster, device),
                stored_frontier(&cluster, PHONE),
                "device {}", device,
            );
            prop_assert_eq!(
                other.agreement().learned(),
                reference.agreement().learned(),
                "device {}", device,
            );
        }

        // A claim and the note written with it happen together, or not
        // at all.
        for name in 0..3u8 {
            let name = name.to_string();
            let note = format!("owner/{name}");
            prop_assert_eq!(holder(&cluster, PHONE, &name), read(&cluster, PHONE, &note));
        }

        // Settling really forgets: nothing settled stays in the log.
        for device in [PHONE, LAPTOP, NAS, FRIEND] {
            let settled = group(&cluster, device).log().settled().clone();
            for key in cluster.devices[device].store.keys() {
                if let Some(Record::Log { author, seq, .. }) = keys::parse(key) {
                    let author = VerifyingKey::from_bytes(&author).map(Peer::from);
                    prop_assert!(author.is_ok_and(|author| !settled.contains(author, seq)));
                }
            }
        }
    }
}
