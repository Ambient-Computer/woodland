//! A copy of what's committed, kept somewhere else: read once, and
//! kept up from what each step says it changed.

use std::collections::BTreeMap;

use super::*;

/// A copy of the household's notes as the NAS has them committed.
#[derive(Default)]
struct Mirror {
    notes: BTreeMap<Vec<u8>, Vec<u8>>,
    /// The checkpoint the copy was last exactly the state of.
    at: Option<u64>,
}

impl Mirror {
    fn everything(cluster: &Sim) -> Query {
        Query::prefix(cluster.home, household_notes(cluster), b"").committed()
    }

    /// Takes in what the NAS has reported since the last time.
    fn follow(&mut self, cluster: &mut Sim) {
        let notes = household_notes(cluster);
        for change in std::mem::take(&mut cluster.changes[NAS]) {
            match change {
                Changed::Key {
                    model,
                    key,
                    committed: true,
                    ..
                } if model == notes => {
                    let query = Query::key(cluster.home, notes, &key).committed();
                    match cluster.read(NAS, &query).pop() {
                        Some((_, value)) => self.notes.insert(key, value),
                        None => self.notes.remove(&key),
                    };
                }
                Changed::Checkpoint { height, .. } => {
                    assert!(self.at.is_none_or(|at| at < height));
                    self.at = Some(height);
                }
                Changed::Everything { .. } => panic!("nothing here starts over"),
                _ => {}
            }
        }
    }

    fn is_current(&self, cluster: &Sim) -> bool {
        let query = Self::everything(cluster);
        let answer = block_on(cluster.devices[NAS].read(&query)).expect("read");
        let reached = answer.reached().expect("committed");
        let found: BTreeMap<_, _> = answer.into_found().into_iter().collect();

        Some(reached.checkpoint) == self.at && found == self.notes
    }
}

fn write_some(cluster: &mut Sim, round: u8) {
    for (device, key) in [(PHONE, "a"), (LAPTOP, "b"), (PHONE, "c"), (LAPTOP, "d")] {
        let notes = household_notes(cluster);
        let key = [key, &round.to_string()].concat();
        cluster.write(device, set(notes, &key, "written"));
    }

    let notes = household_notes(cluster);
    let last = ["a", &round.saturating_sub(1).to_string()].concat();
    cluster.write(PHONE, Transaction::new().remove(notes, last));
}

#[test]
fn a_copy_follows_what_each_step_reports() {
    let mut cluster = household();
    let mut mirror = Mirror::default();
    for round in 0..4 {
        write_some(&mut cluster, round);
        cluster.settle();
        mirror.follow(&mut cluster);

        assert!(mirror.is_current(&cluster));
    }

    assert_eq!(mirror.notes.len(), 13);
}

/// A copy can be started while the device is busy. It's read a part
/// at a time, and whatever is committed between two parts is in what
/// the steps between them reported.
#[test]
fn a_copy_is_begun_while_checkpoints_arrive() {
    let mut cluster = household();
    write_some(&mut cluster, 0);
    write_some(&mut cluster, 1);
    cluster.settle();
    cluster.changes[NAS].clear();

    let mut mirror = Mirror::default();
    let mut next = Some(Mirror::everything(&cluster));
    let mut round = 2;
    while let Some(query) = next {
        let part = block_on(cluster.devices[NAS].read_some(&query, 3)).expect("read");
        assert!(part.answer.reached().is_some());
        mirror.notes.extend(part.answer.into_found());
        next = part.rest;

        write_some(&mut cluster, round);
        cluster.settle();
        round += 1;
    }

    assert!(round > 3);
    assert!(!mirror.is_current(&cluster));
    mirror.follow(&mut cluster);
    assert!(mirror.is_current(&cluster));
}

/// A copy that was at a checkpoint, and finds the device settled at
/// the same one, missed nothing while it wasn't looking. One that
/// finds it further on has.
#[test]
fn a_copy_knows_whether_it_missed_anything() {
    let mut cluster = household();
    let mut mirror = Mirror::default();
    write_some(&mut cluster, 0);
    cluster.settle();
    mirror.follow(&mut cluster);
    let reached = |cluster: &Sim| {
        let query = Mirror::everything(cluster);
        let answer = block_on(cluster.devices[NAS].read(&query)).expect("read");

        answer.reached().expect("committed")
    };

    cluster.tick(NAS);
    cluster.settle();
    assert_eq!(Some(reached(&cluster).checkpoint), mirror.at);

    write_some(&mut cluster, 1);
    cluster.settle();
    assert!(Some(reached(&cluster).checkpoint) > mirror.at);
}

/// A checkpoint too big for a step is reported once it shows: its
/// keys across the steps after, and the checkpoint itself last.
#[test]
fn a_copy_follows_a_checkpoint_that_takes_many_steps() {
    let mut cluster = household();
    let mut mirror = Mirror::default();
    write_some(&mut cluster, 0);
    cluster.settle();
    mirror.follow(&mut cluster);
    assert!(mirror.is_current(&cluster));
    let was = mirror.at;

    cluster.isolate(NAS);
    for round in 1..8 {
        write_some(&mut cluster, round);
    }
    cluster.settle();
    for round in 8..12 {
        write_some(&mut cluster, round);
    }
    cluster.settle();

    // The copy is never told of a checkpoint before it's told of
    // everything the checkpoint changed.
    cluster.connect(NAS, PHONE);
    let mut steps = 0;
    while cluster.deliver() || cluster.resume() {
        let before = mirror.at;
        mirror.follow(&mut cluster);
        assert!(mirror.at == before || mirror.is_current(&cluster));
        steps += 1;
    }

    assert!(steps > 6);
    assert!(mirror.at > was);
    assert!(mirror.is_current(&cluster));
}
