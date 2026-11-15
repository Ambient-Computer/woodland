# Woodland Context

Why Woodland exists and why it is shaped the way it is. The [spec](../spec/README.md) says what is decided; these pages say for whom, where, and on what evidence.

## The problem

- Local-first applications are limited by their database more than by anything else.
- Replicated stores that need no central authority offer simple structures: a map, a document, a log.
- Stores that offer relational structure, constraints, and transactions keep an authority somewhere: a server that orders, validates, or arbitrates.
- Woodland is an attempt at both: relational data, with the authority spread over the few devices that own it.

## Pages

- [domain](domain.md): the kinds of application, peer, and topology this is for, and the ones it is not.
- [environments](environments.md): where it runs, what each place provides, and what each place takes away.
- [rationale](rationale.md): the alternatives that were tried or considered, and why the spec chose otherwise.
