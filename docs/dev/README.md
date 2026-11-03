# Working on UniDTK

This part is for whoever changes UniDTK itself, as the rustc development guide is for whoever changes the compiler. It explains internals that the book only names, the writing standard, and the working boards.

- [How the documentation is written](doc-style.md)
- [Storage internals](storage.md): the redb tables, transactions, and the SQLite migration.
- [Logging internals](logging.md): the queue, batched commits, and the two sinks.
- [The ASCII emblem and the splash](ascii-art.md): how the art in `udtk` is generated.
- [Ideas and technical debt](ideas-and-debt.md)
- Boards: [kanban](kanban.md), [architecture kanban](architecture-kanban.md), [customization menu audit](menu-audit.md).

Building, testing, and the release are the [`udtk` book](../udtk/README.md): see [build, step by step](../udtk/ch14-build-step-by-step.md) and [how to verify a change](../udtk/ch17-how-to-verify-a-change.md).
