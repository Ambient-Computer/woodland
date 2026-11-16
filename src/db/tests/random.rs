//! Devices doing things in an order nobody planned.

use std::collections::BTreeMap;

use proptest::prelude::*;

use super::*;
use crate::model::{Allowances, ColumnId, Table, Value};

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
    Number {
        device: usize,
    },
    Spend {
        device: usize,
        amount: u64,
    },
    Give {
        device: usize,
        to: usize,
        amount: u64,
    },
    Accept {
        device: usize,
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
        3 => device().prop_map(|device| Action::Number { device }),
        3 => (device(), 0..4_u64).prop_map(|(device, amount)| Action::Spend { device, amount }),
        2 => (device(), device(), 0..9_u64)
            .prop_map(|(device, to, amount)| Action::Give { device, to, amount }),
        2 => device().prop_map(|device| Action::Accept { device }),
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

/// Rows that are numbered as they become final.
fn tickets(group: GroupId) -> ModelId {
    ModelId::table(group, [3; 32])
}

fn ticket_number(group: GroupId) -> ColumnId {
    ColumnId::integer(tickets(group), [1; 32])
}

/// Something there are only so many of.
fn stock(group: GroupId) -> ModelId {
    ModelId::allowances(group, [4; 32])
}

const STOCK: u64 = 40;

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

            Action::Number { device } => {
                let (table, column) = (tickets(group), ticket_number(group));
                let numbered = Transaction::new().insert_numbered(table, value, [], column);
                self.attempt(device, numbered);
            }

            Action::Spend { device, amount } => {
                let mine = self
                    .allowances(device, false)
                    .of("stock", self.sim.id(device));
                self.attempt(
                    device,
                    Transaction::new().spend(stock(group), &mine, amount),
                );
            }

            Action::Give { device, to, amount } => {
                let mine = self
                    .allowances(device, false)
                    .of("stock", self.sim.id(device));
                let to = self.sim.id(to);
                // Only if a note is still what this device reads, which
                // any write to it elsewhere undoes: so that gifts are
                // accepted, and spent from, that were never given.
                let seen = self.sim.shown(device, group, notes_of(group, 0), KEYS[0]);
                let guarded = match seen {
                    Some(seen) => Transaction::new().expect(notes_of(group, 0), KEYS[0], seen),
                    None => Transaction::new().expect_absent(notes_of(group, 0), KEYS[0]),
                };
                self.attempt(device, guarded.give(stock(group), &mine, to, amount));
            }

            Action::Accept { device } => {
                let mine = self
                    .allowances(device, false)
                    .of("stock", self.sim.id(device));
                self.attempt(device, Transaction::new().accept(stock(group), &mine));
            }

            Action::Tick { device } => self.sim.tick(device),
            Action::Deliver { messages } => {
                for _ in 0..messages {
                    if !sim.deliver() {
                        sim.resume();
                    }
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

                // Partway through a checkpoint, what shows as committed
                // may be that checkpoint's.
                let reached = self.sim.devices[device].groups[&self.group].reached();
                let first = self.claims.entry(key);
                let (first, _) = first.or_insert_with(|| (held.clone(), reached));
                prop_assert_eq!(&*first, &held, "the committed claim on {} changed", key);
            }
        }

        Ok(())
    }

    fn allowances(&self, device: usize, committed: bool) -> Allowances {
        let query = Query::prefix(self.group, stock(self.group), b"");
        let query = if committed { query.committed() } else { query };

        Allowances::read(&self.sim.read(device, &query))
    }

    /// No device ever reads more stock than there was, in either kind
    /// of read: nothing is spent twice, and nothing given is kept.
    fn check_stock(&self) -> Result<(), TestCaseError> {
        for device in 0..DEVICES {
            for committed in [false, true] {
                let left = self.allowances(device, committed).left("stock");
                prop_assert!(left <= STOCK, "device {device} reads {left} in stock");
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
        let columns = [(ticket_number(group), "number")];
        sim.write_to(PHONE, group, Transaction::new().create_table(tickets(group), columns));
        let phone = sim.id(PHONE);
        sim.write_to(PHONE, group, Transaction::new().open_allowance(stock(group), "stock", STOCK, phone));
        sim.settle();
        let electors = (0..3).collect();
        let mut run = Run { sim, group, written: 0, claims: Claims::new(), electors };
        for action in actions {
            run.act(action);
            run.check_finality()?;
            run.check_stock()?;
        }

        // The network heals, and stays healed for a while.
        run.sim.connect_all();
        for _ in 0..4 {
            run.sim.settle();
            run.check_finality()?;
            run.check_stock()?;
        }

        let everything = |device: usize, committed: bool| {
            let last = KEYS.len() - 1;
            let models = [
                notes_of(group, 0),
                names_of(group, 0),
                notes_of(group, last),
                names_of(group, last),
                tickets(group),
                stock(group),
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

        // Every row that made it has a number, and no number is
        // missing or given twice.
        let table = Query::table(group, tickets(group)).committed();
        let table = Table::read(&run.sim.read(0, &table));
        let rows = table.iter().flat_map(Table::rows);
        let mut given: Vec<Value> = rows.map(|row| row.get(ticket_number(group)).clone()).collect();
        given.sort_by_key(|number| match number {
            Value::Integer(number) => *number,
            _ => 0,
        });
        let dense: Vec<Value> = (1..).map(Value::Integer).take(given.len()).collect();
        prop_assert_eq!(given, dense, "the numbers have a gap");

        // Nobody with a say lost its store, so nobody went back on its
        // word, and nothing honest should look as if it had.
        for device in 0..DEVICES {
            let caught = run.sim.devices[device].groups[&group].caught_a_conflict();
            prop_assert!(!caught, "device {} convicted an honest member", device);
        }
    }
}
