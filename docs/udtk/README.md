# The `udtk` book: the UniDTK CLI bridge

This is the book about `udtk`, the command-line bridge of **UniDTK** (Unified Desktop Täglich Kit). It teaches the mechanism step by step: what runs on the machine, which channel carries each request, which code decides whether a request is allowed, and why it is built that way. It follows the documentation principle in [architecture.md](../reference/README.md): English, low level, like the Rust Book. The entry point of the whole book is [README.md](../../README.md). `udtk doc` renders that book and its linked pages into the browser.

> **Source of truth.** The code. Every fact in this book was checked against the files named in it on 2026-11-02. Where the code and an older note disagree, this book follows the code and says so in [Chapter 18](ch18-known-gaps-and-open-decisions.md#chapter-18-known-gaps-and-open-decisions). Session records and acceptance reports from the migration are kept, condensed, in [Part V: History](../history/rfcs/udtk-migration.md).

## Chapters


**Part I. Foundations**

- [Chapter 1. What `udtk` is](ch01-what-udtk-is.md)
- [Chapter 2. How a command travels](ch02-how-a-command-travels.md)
- [Chapter 3. The two doors, and why writes stay on the UI door](ch03-the-two-doors-and-why-writes-stay-on-the.md)
- [Chapter 4. The WebSocket door](ch04-the-websocket-door.md)
- [Chapter 5. Controlling the WebView over CDP](ch05-controlling-the-webview-over-cdp.md)
- [Chapter 6. Design decisions](ch06-design-decisions.md)

**Part II. Commands**

- [Chapter 7. The command map](ch07-the-command-map.md)
- [Chapter 8. `invoke` and `logs`](ch08-invoke-and-logs.md)
- [Chapter 9. `gui`: driving the page](ch09-gui-driving-the-page.md)
- [Chapter 10. `shot` and `cdp-list`](ch10-shot-and-cdp-list.md)
- [Chapter 11. `doctor`: the diagnostic loop](ch11-doctor-the-diagnostic-loop.md)
- [Chapter 12. `tasks`, `autostart`, and `path`](ch12-tasks-autostart-and-path.md)
- [Chapter 13. `run`, `stop`, `doc`, `help`, and the splash](ch13-run-stop-doc-help-and-the-splash.md)
- [Chapter 14. `build`, step by step](ch14-build-step-by-step.md)

**Part III. Rules for modules and the install**

- [Chapter 15. How a module joins the bridge](ch15-how-a-module-joins-the-bridge.md)
- [Chapter 16. Install layout and data](ch16-install-layout-and-data.md)

**Part IV. Verification and open items**

- [Chapter 17. How to verify a change](ch17-how-to-verify-a-change.md)
- [Chapter 18. Known gaps and open decisions](ch18-known-gaps-and-open-decisions.md)

The design history of the bridge is in [the udtk migration record](../history/rfcs/udtk-migration.md).
