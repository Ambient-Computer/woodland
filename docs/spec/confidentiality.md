# Confidentiality

A sealed [group](groups.md) encrypts what its contributors write, so that peers can carry it without reading it. Who can read follows the [checkpoint](checkpoints.md) chain, like who can write.

## Open and sealed

- A group is open or sealed from its founding, and never changes.
- An open group is readable by anyone who obtains it. It is for data meant to be public.
- A sealed group is readable by its contributors and its [readers](peers.md), and carried by anyone.
- Sealing hides contents. It does not hide that the group exists, who writes to it, or how much.

## What is sealed

- An [entry](log.md)'s payload is encrypted. Its header is not, and commits to the encrypted payload.
- A peer without keys therefore verifies every signature, link, and checkpoint, and relays a sealed group as faithfully as an open one.
- Committed state travels encrypted. A peer without keys cannot check it against a checkpoint, and does not need to in order to carry it.
- The state root in a checkpoint commits to contents without revealing them, even contents that are easy to guess. It does not change when keys do.

## Epochs

- A sealed group has a current secret. Every peer the [configuration](membership.md) lists as contributor or reader holds it.
- A contributor that lacks the secret neither writes nor votes until it has it.
- A checkpoint may replace the secret. One that removes a contributor or reader, or replaces a peer's key, always does.
- A replacement is derived for one set of readers. An attempt that names a different set never reveals it.
- The new secret is delivered inside the checkpoint, to each peer that should hold it and to nobody else.
- A contributor votes for such a checkpoint only after finding its own copy and checking it against what the checkpoint commits to.
- A secret belongs to the checkpoint that carries it. A secret from an attempt that was never decided is discarded and never used.
- An entry is encrypted under the secret of the checkpoint its header names.
- An entry that does not open under that secret is not an entry. Everyone who holds the secret agrees; a peer without it cannot tell, and carries it.
- Nothing is re-encrypted when the secret changes. Old data stays under old secrets; new writes and new snapshots use the new one.

## What removal guarantees

- A removed peer cannot read any entry whose header names the checkpoint that removed it, or a later one.
- It can still read entries written under secrets it held, including ones written after its removal by contributors who had not yet heard.
- It keeps everything it already had. Removal cannot unshare.
- The guarantee is relative to the chain, never to wall-clock time.

## History

- Each new secret gives access to the one before it. A peer that holds the current secret can read the group's whole past.
- A new contributor or reader is therefore given one secret, and reads everything.
- There is no forward secrecy. A secret that leaks exposes everything before it; the next replacement ends the exposure.
- The chain of secrets is kept as long as any history it protects is. An [archive](compaction.md) without it holds noise.

## Keys

- A peer encrypts with a key separate from its signing key, vouched for by its signing key and named in the configuration.
- A sealed group's configuration names a key for every contributor and reader. Nobody votes for one that does not.
- Replacing a peer's encryption key is a configuration change, and the only way a compromised device recovers without a new identity.
- A listed peer that did not receive a secret, by fault or malice, is given it by any peer that holds it. The configuration is the grant; delivery is only delivery.

## What remains visible

- Who the contributors, witnesses, and readers are, and their weights.
- Who wrote each entry, when it arrived, how large it is, and what it was written on top of.
- When secrets were replaced, which suggests when someone was removed.
- An application that must hide these needs more than this page provides.

## Cost

- Replacing a secret costs work and space proportional to the number of peers that hold it.
- A group with a large audience is open, or is read through a peer that republishes it; see [groups](groups.md).
