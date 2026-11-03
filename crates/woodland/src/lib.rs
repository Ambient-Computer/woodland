/// Represents this device. Applications should only create one.
///
/// Manages current state, updates, and subscriptions.
#[derive(Debug, Default)]
pub struct Node;

impl Node {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}
