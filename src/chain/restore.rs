//! Getting back checkpoints that were shed.
//!
//! Every checkpoint names the one before it, so a checkpoint held
//! vouches for the one that goes in the gap below it. They come back
//! latest first, each checked against the one above.

use super::{Certificate, Chain};

impl Chain {
    /// The height of the latest checkpoint that's missing from below
    /// the tip, if any is.
    pub(crate) fn lacks(&self) -> Option<u64> {
        let mut expected = self.tip().height;
        for link in self.decided.iter().rev() {
            if link.checkpoint.height != expected {
                break;
            }
            expected -= 1;
        }

        (expected > 0).then_some(expected)
    }

    /// Takes back a checkpoint that was shed, if it's the one the
    /// checkpoint above it names, and it comes with the votes that
    /// decided it. Without those it couldn't be handed on.
    pub(crate) fn fill(&mut self, link: Certificate) -> bool {
        let height = link.checkpoint.height;
        let above = self.at(height + 1);
        let below = self
            .checkpoints()
            .take_while(|it| it.height < height)
            .last();
        let (Some(above), Some(below)) = (above, below) else {
            return false;
        };
        if self.at(height).is_some() || !link.fills(below, above) {
            return false;
        }

        let place = self
            .decided
            .partition_point(|it| it.checkpoint.height < height);
        self.decided.insert(place, link);

        true
    }

    /// The checkpoints at `height` and below that are held one after
    /// another, latest first, and no more than `count` of them.
    pub(crate) fn older(&self, height: u64, count: usize) -> Vec<Certificate> {
        let mut expected = height;
        let mut older = Vec::new();
        for link in self.decided.iter().rev() {
            if link.checkpoint.height > expected {
                continue;
            }
            if link.checkpoint.height < expected || older.len() == count {
                break;
            }

            older.push(link.clone());
            expected -= 1;
        }

        older
    }
}
