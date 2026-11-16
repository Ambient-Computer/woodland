//! Devices doing things in an order nobody planned.

use std::collections::BTreeMap;

use proptest::prelude::*;

use super::*;

const DEVICES: usize = 4;
const KEYS: [&str; 3] = ["a", "b", "c"];

#[derive(Clone, Debug)]
enum Action {
    Set {
        device: usize,
        key: usize,
    },
    Claim {
        device: usize,
        key: usize,
    },
    Expect {
        device: usize,
        key: usize,
    },
    Tick {
        device: usize,
    },
    Deliver {
        messages: usize,
    },
    Connect {
        a: usize,
        b: usize,
    },
    Disconnect {
        a: usize,
        b: usize,
    },
    Restart {
        device: usize,
    },
    Compact {
        device: usize,
    },
    Forget {
        device: usize,
    },
    Reconfigure {
        device: usize,
        members: Vec<(usize, usize)>,
    },
}

fn action() -> impl Strategy<Value = Action> {
    let device = || 0..DEVICES;
    let key = || 0..KEYS.len();
    let members = proptest::collection::vec((0..DEVICES, 0..6_usize), 1..=DEVICES);

    prop_oneof![
        6 => (device(), key()).prop_map(|(device, key)| Action::Set { device, key }),
        6 => (device(), key()).prop_map(|(device, key)| Action::Claim { device, key }),
        3 => (device(), key()).prop_map(|(device, key)| Action::Expect { device, key }),
        6 => device().prop_map(|device| Action::Tick { device }),
        12 => (1..12_usize).prop_map(|messages| Action::Deliver { messages }),
        3 => (device(), device()).prop_map(|(a, b)| Action::Connect { a, b }),
        3 => (device(), device()).prop_map(|(a, b)| Action::Disconnect { a, b }),
        1 => device().prop_map(|device| Action::Restart { device }),
        2 => device().prop_map(|device| Action::Compact { device }),
        3 => device().prop_map(|device| Action::Forget { device }),
        1 => (device(), members).prop_map(|(device, members)| Action::Reconfigure { device, members }),
    ]
}

/// The last key lives in a clock of its own, so every run has two
/// histories under one chain.
fn clock_of(group: GroupId, key: usize) -> ClockId {
    if key + 1 == KEYS.len() {
        ClockId::new(group, [9; 32])
    } else {
        group.into()
    }
}

fn notes_of(group: GroupId, key: usize) -> ModelId {
    ModelId::kv(clock_of(group, key), [1; 32])
}

fn names_of(group: GroupId, key: usize) -> ModelId {
    ModelId::claims(clock_of(group, key), [2; 32])
}

/// Who holds each claim for good, as far as any device has said, and
/// how many checkpoints that device had committed when it said so. The
/// device may since have lost its store, and be the only one that knew.
type Claims = BTreeMap<&'static str, (String, u64)>;

struct Run {
    sim: Sim,
    group: GroupId,
    written: u64,
    claims: Claims,
    /// Every device that has ever been offered a say. One that loses
    /// its store forgets what it voted for, and no protocol survives
    /// that: it's why a device that loses its store comes back as
    /// someone new. A device with no say can only fork its own chain.
    electors: BTreeSet<usize>,
}

impl Run {
    fn attempt(&mut self, device: usize, transaction: Transaction) {
        match block_on(self.sim.devices[device].write(self.group, transaction)) {
            Ok(outputs) => self.sim.absorb(device, outputs),
            Err(Error::NotAContributor) => {}
            Err(error) => panic!("a write failed: {error}"),
        }
    }

    fn act(&mut self, action: Action) {
        let (group, sim) = (self.group, &mut self.sim);
        self.written += 1;
        let value = format!("{}", self.written);
        match action {
            Action::Set { device, key } => {
                self.attempt(device, set(notes_of(group, key), KEYS[key], &value));
            }

            Action::Claim { device, key } => {
                let wins = Transaction::new()
                    .claim(names_of(group, key), KEYS[key], value.as_str())
                    .set(notes_of(group, key), KEYS[key], value.as_str());
                self.attempt(device, wins);
            }

            Action::Expect { device, key } => {
                let seen = sim.shown(device, group, notes_of(group, key), KEYS[key]);
                let transaction = match seen {
                    Some(seen) => Transaction::new().expect(notes_of(group, key), KEYS[key], seen),
                    None => Transaction::new().expect_absent(notes_of(group, key), KEYS[key]),
                };
                self.attempt(
                    device,
                    transaction.set(notes_of(group, key), KEYS[key], value.as_str()),
                );
            }

            Action::Tick { device } => sim.tick(device),
            Action::Deliver { messages } => {
                for _ in 0..messages {
                    sim.deliver();
                }
            }

            Action::Connect { a, b } if a != b => sim.connect(a, b),
            Action::Disconnect { a, b } if a != b => sim.disconnect(a, b),
            Action::Restart { device } => sim.restart(device),
            Action::Compact { device } => sim.compact(device, group),
            Action::Forget { device } if !self.electors.contains(&device) => {
                sim.forget(device, group);
            }

            Action::Connect { .. } | Action::Disconnect { .. } | Action::Forget { .. } => {}
            Action::Reconfigure { device, members } => {
                // Mostly contributors to everything. Now and then a
                // witness, a contributor to one clock, or one with no say.
                let role = |kind: usize| match kind {
                    0 => Role::witness(1),
                    1 => Role::contributor(1).only([clock_of(group, 0)]),
                    2 => Role::contributor(0).only([clock_of(group, KEYS.len() - 1)]),
                    _ => Role::contributor(1),
                };
                let counted = members.iter().filter(|(_, kind)| *kind != 2);
                self.electors.extend(counted.map(|&(it, _)| it));
                let weights: Vec<_> = members
                    .iter()
                    .map(|&(it, kind)| (sim.id(it), role(kind)))
                    .collect();
                match block_on(sim.devices[device].reconfigure(group, weights)) {
                    Ok(outputs) => sim.absorb(device, outputs),
                    Err(Error::NotAContributor) => {}
                    Err(error) => panic!("a request failed: {error}"),
                }
            }
        }
    }

    /// A claim that any device has shown as committed is held by the
    /// same value, on every device, from then on.
    fn check_finality(&mut self) -> Result<(), TestCaseError> {
        for device in 0..DEVICES {
            for (index, key) in KEYS.into_iter().enumerate() {
                let names = names_of(self.group, index);
                let held = self.sim.committed(device, self.group, names, key);
                let folded = self.folded(device);
                let Some(held) = held else {
                    let known = self.claims.get(key);
                    prop_assert!(
                        known.is_none_or(|(_, since)| folded < *since),
                        "device {device} lost the committed claim on {key}"
                    );
                    continue;
                };

                let first = self.claims.entry(key);
                let (first, _) = first.or_insert_with(|| (held.clone(), folded));
                prop_assert_eq!(&*first, &held, "the committed claim on {} changed", key);
            }
        }

        Ok(())
    }

    /// How many checkpoints `device` has committed.
    fn folded(&self, device: usize) -> u64 {
        self.sim.devices[device].groups[&self.group].folded()
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn devices_agree_on_what_is_committed_and_end_up_the_same(
        actions in proptest::collection::vec(action(), 1..120),
    ) {
        // Three devices decide. The fourth writes, has no say, and is
        // the one that can lose its store without breaking agreement.
        let (mut sim, group) = Sim::group(DEVICES, 3);
        let roles = (0..DEVICES).map(|it| (it, Role::contributor(u32::from(it < 3))));
        sim.assign(PHONE, group, roles);
        sim.settle();
        let electors = (0..3).collect();
        let mut run = Run { sim, group, written: 0, claims: Claims::new(), electors };
        for action in actions {
            run.act(action);
            run.check_finality()?;
        }

        // The network heals, and stays healed for a while.
        run.sim.connect_all();
        for _ in 0..4 {
            run.sim.settle();
            run.check_finality()?;
        }

        let everything = |device: usize, committed: bool| {
            let last = KEYS.len() - 1;
            let models = [
                notes_of(group, 0),
                names_of(group, 0),
                notes_of(group, last),
                names_of(group, last),
            ];
            let read = |model: ModelId| {
                let query = Query::prefix(group, model, b"");
                let query = if committed { query.committed() } else { query };

                run.sim.read(device, &query)
            };

            models.map(read)
        };
        for device in 1..DEVICES {
            prop_assert_eq!(everything(device, false), everything(0, false), "device {} shows something else", device);
            prop_assert_eq!(everything(device, true), everything(0, true), "device {} committed something else", device);
        }
        prop_assert_eq!(everything(0, false), everything(0, true), "something was never committed");

        // Nobody with a say lost its store, so nobody went back on its
        // word, and nothing honest should look as if it had.
        for device in 0..DEVICES {
            let caught = run.sim.devices[device].groups[&group].caught_a_conflict();
            prop_assert!(!caught, "device {} convicted an honest member", device);
        }
    }
}
