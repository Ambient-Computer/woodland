use serde::Serialize;

use crate::actor::Peer;
use crate::encoding;
use crate::io::{Action, NetworkAction};
use crate::storage::{StorageAction, StorageWrite};

/// Collects the storage work domains produce while processing a batch, then
/// turns it and the batch's messages into ordered actions.
///
/// Domains add work here as they go. They never produce actions directly,
/// because only the end of the batch knows the full picture: every write
/// becomes one atomic storage action, and it always precedes the sends that
/// might advertise what it stored.
#[derive(Debug, Default)]
pub(crate) struct Outbox {
    writes: Vec<StorageWrite>,
    scans: Vec<Vec<u8>>,
}

impl Outbox {
    pub(crate) fn put(&mut self, key: Vec<u8>, value: &impl Serialize) {
        let value = encoding::encode(value);
        self.writes.push(StorageWrite::Put { key, value });
    }

    /// Stores bytes that are already encoded.
    pub(crate) fn put_bytes(&mut self, key: Vec<u8>, value: Vec<u8>) {
        self.writes.push(StorageWrite::Put { key, value });
    }

    pub(crate) fn delete(&mut self, key: Vec<u8>) {
        self.writes.push(StorageWrite::Delete { key });
    }

    pub(crate) fn delete_range(&mut self, start: Vec<u8>, end: Vec<u8>) {
        self.writes.push(StorageWrite::DeleteRange { start, end });
    }

    pub(crate) fn scan(&mut self, prefix: Vec<u8>) {
        self.scans.push(prefix);
    }

    /// Writes first, then reads, then sends. Never advertise before it's
    /// durable, and never read around a pending write.
    pub(crate) fn into_actions(self, messages: Vec<(Peer, Vec<u8>)>) -> Vec<Action> {
        let write = (!self.writes.is_empty()).then_some(StorageAction::Write(self.writes));
        let scans = self
            .scans
            .into_iter()
            .map(|prefix| StorageAction::Scan { prefix });
        let sends = messages.into_iter().map(|(peer, message)| {
            Action::Network(NetworkAction::Send {
                peer: peer.id(),
                message,
            })
        });

        write
            .into_iter()
            .chain(scans)
            .map(Action::Storage)
            .chain(sends)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::peers::{laptop, phone};

    #[test]
    fn does_nothing_without_work() {
        assert_eq!(Outbox::default().into_actions(Vec::new()), []);
    }

    #[test]
    fn writes_once_before_scanning_and_sending() {
        let mut outbox = Outbox::default();
        outbox.put(b"a".to_vec(), &1u8);
        outbox.scan(b"p".to_vec());
        outbox.put(b"b".to_vec(), &2u8);

        let actions = outbox.into_actions(vec![(phone(), vec![1]), (laptop(), vec![2])]);

        assert!(
            matches!(&actions[0], Action::Storage(StorageAction::Write(writes)) if writes.len() == 2)
        );
        assert!(matches!(
            &actions[1],
            Action::Storage(StorageAction::Scan { .. })
        ));
        assert!(matches!(&actions[2], Action::Network(_)));
        assert!(matches!(&actions[3], Action::Network(_)));
        assert_eq!(actions.len(), 4);
    }
}
