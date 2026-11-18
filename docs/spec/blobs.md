# Blobs

A blob is bytes too large or too opaque to be a value. A [transaction](transactions.md) writes a blob reference; the bytes are kept and moved apart from history, by whichever [peers](peers.md) choose to hold them.

## Blob references

- A blob is immutable bytes with a name: a BLAKE3 hash of them, taken as a tree so that it vouches for any 16 KiB of them without the rest.
- A blob reference is a value that names a blob and says how long it is. On this page it is a reference; it has nothing to do with a [table's](tables.md) references between rows.
- A reference is written, ordered, judged, and committed like any other value. [Models](models.md) says where one may be written.
- A reference says what the bytes are. It never says that anyone has them.
- The length is part of what is checked. A blob is its name and its length together, and a reference with the wrong length is to a blob that does not exist and can never be fetched.
- A reference says what kind of thing it names. A blob is the only kind; a peer that meets another treats the operation as one it [does not know](models.md).
- A transaction refers only to a blob its author holds whole, even to pass on a reference it was sent. No peer can check that of another; it is the author's to keep, like not [forking](log.md), and breaking it leaves no proof.
- A reference describes nothing of what a blob contains, and neither do its bytes. A name, a type, and a place in a directory are ordinary data beside the reference, and a directory is a [table](tables.md).

## Lists

- An [entry](log.md) lists the name and length of each blob it refers to. A peer that cannot read the entry, or lacks its model's kind, takes the list for what the entry refers to.
- A peer that can read an entry counts what it reads, and ignores the list.
- An entry whose list differs from what it refers to is still an entry. Peers that took the list are put right when a checkpoint covers it.
- A [checkpoint](checkpoints.md) commits to the blobs each clock's state refers to. Committed state handed to a peer that cannot read it comes with their names and lengths, checked whole against that commitment before anything is let go on its strength.
- A peer that cannot read is therefore never told what to drop by whoever hands it state.

## Holding

- A peer holds all of a blob, part of it, or none. Which is not state: it is never committed, never voted on, and differs between peers.
- A [vote](checkpoints.md) vouches for a reference and not for bytes. A contributor may commit a reference to a blob it has never held.
- A blob is held once per store, however many values and groups refer to it.
- A peer holds bytes only for a blob that something it holds refers to: a committed value, an entry whatever its [verdict](transactions.md), or an [import](#importing).
- A pruned transaction's reference counts for as long as its entry is held, since the transaction may be revived.
- A value [kept](library.md) while a checkpoint or snapshot is put in place counts until it is discarded.
- When nothing refers to a blob any longer, its bytes are deleted, across [steps](library.md). After a committed deletion, that is when [compaction](compaction.md) sheds the entries that referred to it.
- A blob that is referred to again while its bytes are being deleted is a blob held in part. Deleting stops, and what is left is kept.
- A peer that lets go of everything it holds of a group, to be sent it again, keeps the blobs the group referred to until it holds again all that some peer does. What came back refers to them from then on.
- What refers to each blob is a derived structure, kept current as entries arrive and are let go, and as committed state changes.
- The core never lets go of the bytes of a blob that is referred to, unasked. An application may let go of any, and the reference remains.
- A blob the application let go of is fetched again only when it asks, for as long as anything refers to it.
- No bound on a blob's size is assumed. Every operation on a blob does bounded work per step.

## Policy

- A policy says which blobs a peer fetches unasked: every one, those up to a size, or none.
- A policy is local, like an [index](tables.md). It needs no agreement, and the devices of one group differ.
- A device states one policy for itself, and may state another for any model. The library has no default: a device holds no group until its policy is stated.
- A policy covers the blobs a [read](reads.md) at either level would find a reference to. A blob only shed, pruned, or replaced values refer to is not fetched unasked.
- A peer that goes by [lists](#lists) applies the device's policy to every blob listed.
- A blob the policy leaves out is fetched when the application asks for it.
- References whose blobs are wanted differently belong in different models.

## Availability

- A blob is available while a reachable peer holds it. Nothing guarantees that one does.
- A reference whose only holder is lost refers to nothing, for good. No peer can tell that from a holder that is away.
- An application treats every blob as possibly absent. It is told how much of a blob the device holds, and when that changes.
- A group that wants its blobs kept has a peer whose policy fetches every one. That peer need not be able to read them.
- History [restored](compaction.md) from an archive comes without the bytes it refers to, unless they are asked for and some peer still holds them.

## Importing

- Bytes become a blob by import. The application supplies them in order, a bounded piece per step, and is given the reference when the last is in.
- An import says when it begins whether it is for [sealed](#sealed-groups) groups or open ones.
- An import is durable from its first step. A device that stops resumes it where it was, and the blob is the one it would have been.
- A device says which imports are unfinished, and which are finished and not yet referred to. The application resumes, writes, or abandons each; none lapses by itself.
- What imports hold is the application's to bound. It is the one thing a peer stores that nothing else in the library lets go of.
- Where the bytes come from is the application's concern.
- Importing bytes the store already holds under the same name keeps one copy.

## Reading

- A blob is read a bounded range at a time, from what the peer holds. A range that is not held says so.
- Reading a blob is not a [read](reads.md) of state and has no level. The bytes under a name are the same wherever and whenever they are read.

## Fetching

- A peer asks a connected peer for a blob, or for the parts of one it lacks. It is answered with bytes, or told there are none.
- A peer serves a blob only within a group both sides [share](sync.md) on that connection, and only if something it holds in that group refers to the blob. Knowing a name is not enough.
- Bytes arrive a piece at a time, each with what is needed to check it against the name and length. A piece is checked before it is stored, and one that fails is dropped.
- What has arrived is durable and is not fetched again. A fetch continues from any peer, and pieces from several combine.
- A peer serves whatever part it holds and has made durable, whether or not it can read it.
- Blob bytes are not part of the [exchange](sync.md), and nothing in the exchange waits for them.
- A peer that is serving releases one bounded piece each time its application asks for the next, apart from its other messages. An application that gives blobs a stream of their own never delays sync with them.
- A fetch that stops is taken up again when a peer connects or [time](library.md) passes.

## Sealed groups

- A blob written in a [sealed](confidentiality.md) group is encrypted under a key of its own, made at import from randomness the application supplies.
- Its name is the hash of the encrypted bytes. Its key is in the reference, sealed with the value.
- A peer without keys therefore fetches, checks, holds, and serves a sealed blob like any other.
- Replacing the group's secret re-encrypts no blob and renames none.
- The name is what proves the bytes are the ones the author wrote. The encryption carries no check of its own.
- A key encrypts one blob. Importing the same bytes again makes another blob; copying a reference names the same one.
- In an open group a name depends on the bytes alone, so the same bytes are the same blob in every open group.
- A reference written in a sealed group has a key, and one written in an open group has none. Neither is valid in the other kind of group; the bytes are imported again.
