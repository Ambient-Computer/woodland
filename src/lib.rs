// Pieces are built bottom-up within each slice and wired into `Node` at the
// end. Each `expect` comes off once its module is reachable from `Node`.
#[expect(
    dead_code,
    unused_imports,
    reason = "scaffolding until wired into Node"
)]
mod actor;
#[expect(
    dead_code,
    unused_imports,
    reason = "scaffolding until wired into Node"
)]
mod clock;
#[expect(dead_code, reason = "scaffolding until wired into Node")]
mod encoding;
mod io;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "scaffolding until wired into Node")
)]
mod lattice;
mod node;
#[cfg(test)]
mod testing;

pub use actor::Identity;
pub use io::{Action, NetworkAction};
pub use node::{Batch, Node};
