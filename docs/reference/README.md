# The UniDTK Reference

The reference is the law of the system. The [book](../book/ch00-introduction.md) teaches how UniDTK works, in order, from the first chapter to the last. The reference does not teach; it states what must stay true, one contract per chapter, and each chapter can be read alone.

When the reference and the code disagree, one of them has a bug. Fix the code, or change the rule here in the same commit that changes the contract.

## Rules and their identifiers

Every rule starts with an identifier on its own line, like this:

```text
r[storage.no-raw-db]
```

The identifier is stable: it does not change when a chapter is reordered. Code, the book, and the session log cite a rule by that identifier, never by a section number, because section numbers move. `udtk doc --check` fails when a cited identifier does not exist, when one is defined twice, or when one is defined outside this directory.

A rule's text lives in exactly one place, here. Other documents link to it, for example `[storage.no-raw-db](../reference/storage.md#r-storage.no-raw-db)`, instead of restating it.

## Chapters

- [The kernel](kernel.md): what the core is, what it owns, and how it starts.
- [Modules](modules.md): the module contract, lifecycle, and isolation.
- [Storage](storage.md): redb, the data and log files, module key slices.
- [IPC and the two doors](ipc.md): the socket, the interface door, and the read-only rule.
- [Install and runtime layout](install.md): Program Files, AppData, scheduled tasks, external binaries.
- [The desk, the menu, and themes](desk.md): positions, folders, nesting, tokens, and contrast.
- [Motion](motion.md) and [Surfaces](surfaces.md): animation tokens and the morph.

## The documentation principle

r[docs.english-low-level]

Documentation is written in English and is low level, like the Rust Book: it teaches the mechanism step by step, what happens on the machine and why. The book is `README.md` and the pages listed in [SUMMARY.md](../SUMMARY.md). `udtk doc` opens it, as `rustup doc` opens the Rust documentation. Code comments are written in English too.

r[docs.one-place]

Each kind of text has one home: teaching in `book/`, rules in `reference/`, one tool in `udtk/`, one module in `modules/`, work on the project itself in `dev/`, and what happened in `sessions.md` and `history/`. History is frozen: it is not rewritten to match the present.
