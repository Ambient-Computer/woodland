# Prototype

Learnings from the prototype phase:

- Schema types are relatively easy to implement.
- Optional types are annoying to implement.
- TypeScript types are *miserable* to sync up here. Lots of indirection.
- It will be hard to build out the views unless I use a doubly-linked list.
- I need another data structure to incrementally index total order. This is required for integrating remote changes, where I don't know what needs to be recomputed and whether to update the latest pointer.
- Prefer objects over IDs. It's okay to have cycles in memory.
- I don't need a `Delta` type. I need an `Envelope` type.

## TODO

- Prototype schema versions
- Prototype object hashing
- Try your hand at the message format
- Explore migrations
- Refine the data structures
- Experiment with a low-level view binding

