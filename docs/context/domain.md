# Domain

Woodland is for applications whose data belongs to a few devices and to no server. This page describes those applications by what they need, not by what they are.

## Who writes

- A handful of devices, usually one person's or one small team's. Dozens at the outside.
- Every writer is known by name to the others and admitted deliberately.
- Writers are trusted to be honest and expected to fail: to be lost, wiped, replaced, or left in a drawer.
- Writers are rarely online together. Two of them may never be online at the same moment.

## Kinds of peer

- An interactive device: used by a person, offline most of the time, short on storage, and liable to lose it. It wants the latest state now and little history.
- An always-on device: reachable, durable, and unattended. It is what makes commits frequent and data safe, and it is still only a peer.
- An automated writer: produces far more than a person does, on a schedule, about something narrow. Its volume must not become every other device's burden.
- A consumer: reads to report, display, or react, and never writes. There may be any number of them.
- A relay: carries data between peers that cannot reach each other, and need not be able to read it.
- An archive: keeps what others shed, so that history can be recovered and proven.
- One device is often several of these at once, and what it is changes over its life.

## What every application of this kind needs

- To start alone, on one device, with no account and no network.
- To add a device by linking it from one already trusted, and for the new device to catch up without replaying everything.
- To keep working on every device while none of them can reach another.
- For changes to arrive through whatever path exists, including through devices that cannot write or read.
- To remove a device: one retired, one lost, and one that is not cooperating.
- To give read access without write access, and to take it back.
- For interactive devices to stay small while others keep everything.
- To change its schema and features over years, while old devices keep running old versions.
- To know, where it matters, that what it is showing will never be taken back.

## Topologies

- One owner, several devices: the base case. Every other shape is built from groups like it.
- A small team: more contributors, the same rules. Disagreement between members is a human matter, not a protocol one.
- A publication: few writers, many readers. The audience is not known to the writers and does not burden them.
- A fan-in: one peer observes many groups and writes a combined view into its own. Each source stays small and independently owned; the combined view is ordinary data.
- A shared identity: a small group holding facts about its owner, which other groups' members observe instead of copying. The owner's devices are the only authority over it.
- A fan-in and a shared identity are compositions, not features. The library provides groups, observation, and verifiable data; an application arranges them.

## What it is not for

- Data with an open or large set of writers. Agreement among named members does not stretch to strangers.
- Writers that are adversaries of each other. Misbehaviour is provable here, not preventable.
- Data too large for the devices that subscribe to it. Every subscriber holds the whole of what it subscribes to.
- Decisions that must be final at once. Finality waits for people's devices to meet.
- Applications content with a server. A server that orders everything is simpler than anything here.
