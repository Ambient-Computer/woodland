// Pieces are built bottom-up within each slice and wired into `Node` at the
// end. Each `expect` comes off once its module is reachable from `Node`.
#[expect(
    dead_code,
    unused_imports,
    reason = "scaffolding until wired into Node"
)]
mod actor;
#[expect(dead_code, reason = "scaffolding until wired into Node")]
mod agreement;
#[expect(dead_code, reason = "scaffolding until wired into Node")]
mod clock;
mod encoding;
mod io;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "scaffolding until wired into Node")
)]
mod lattice;
#[expect(
    dead_code,
    unused_imports,
    reason = "scaffolding until wired into Node"
)]
mod model;
mod node;
#[cfg(test)]
mod testing;

pub use actor::Identity;
pub use agreement::{AgreementId, Genesis};
pub use io::{Action, NetworkAction};
pub use model::ModelId;
pub use node::{Batch, Node};
