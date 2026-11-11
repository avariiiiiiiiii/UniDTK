# How the Documentation Is Written

This page is the standard for every page of the book. The principle behind it is the rule [docs.english-low-level](../reference/README.md#r-docs.english-low-level): English, low level, the mechanism step by step, like the Rust Book.

## Where a text goes

| Kind of text | Directory | Read |
| :--- | :--- | :--- |
| Teaching, in order | `docs/book/` | front to back |
| Rules, one contract per chapter | `docs/reference/` | by chapter |
| One tool, `udtk` | `docs/udtk/` | by chapter |
| One module | `docs/modules/` | by module |
| Working on the project: build, internals, boards | `docs/dev/` | by task |
| What happened, and frozen plans | `docs/sessions.md`, `docs/history/` | as a record |

A fact lives in one place. A rule's text is in `reference/`; everything else links to it.

## How a chapter reads

- Start with one paragraph that says what the chapter covers and what the reader will be able to do after it.
- Follow the mechanism in the order the machine runs it: which function is called, on which thread, what it writes, what it returns, how it fails.
- Name real files and functions, for example `resolve_desktop_host()` in `core/kernel/wallpaper.rs`. Do not describe code that does not exist; read it first.
- Short code listings are welcome. Put a caption line before a listing: `Filename: core/kernel/wallpaper.rs`. Abridge with `// ...` and say that it is abridged.
- Explain why, not only what. A rule without its reason is not finished.
- Use a "Note:" paragraph for a side remark and a "Warning:" paragraph for a trap.
- Cite a rule by its identifier and link it: `[storage.no-raw-db](../reference/storage.md#r-storage.no-raw-db)`.
- No status banners, no emoji, no marketing words. Dates and session numbers belong in the session log, not in teaching text.

## Rules

A rule is defined only in `docs/reference/`, by a line that holds nothing but its identifier:

```text
r[area.short-name]
```

The identifier uses lowercase letters, digits, dots, and hyphens, and has at least one dot. Once published it does not change.

## Checking

`udtk doc --check` runs from a checkout and fails on: a broken link or anchor, a page that is not in `SUMMARY.md` or a listed page that is missing, an unknown `udtk` command, an unknown or duplicate rule identifier, a reference file over 350 lines, and a code comment that names a missing document or cites a section number instead of a rule. History (`docs/history/`, `docs/sessions.md`) is not checked: it records what was true then.
