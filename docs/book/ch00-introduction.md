# Introduction

UniDTK is a desktop shell for Windows. It draws its own window behind the desktop icons, and the widgets on that window are small programs called modules. This book explains how the shell works, from the first click that starts it to the last line of a module, and it does so in the order the machine runs the work. It is written for a programmer who is new to this project. You do not need to know Win32 or WebView2 before you start.

## What UniDTK is

The shell is one program, `unidtk.exe`, which the book calls the kernel. The kernel starts a WebView2 window, which is the browser engine that Microsoft Edge uses, and attaches that window to the desktop so that it sits underneath the icons. Inside the window, a page draws the widgets. Each widget is backed by a module on the Rust side. The modules in the source tree are `glyph-bar`, `explorer`, `vpn`, `telegram`, `mp3-player`, and `micrographics`, listed in `core/kernel/registry.rs`.

A second program, `udtk.exe`, is the command-line bridge. It asks the running kernel questions, reads its log, takes screenshots of the page, and runs the release build. The kernel and the bridge share one list of commands, so they cannot disagree about what exists.

The kernel does very little on its own. It owns the window, the command registry, the event bus, and the two databases. Everything the user sees as a feature is a module. The rule that draws this line is [kernel.core-invisible](../reference/kernel.md#r-kernel.core-invisible): if turning a feature off would not stop the shell from being a shell, the feature is a module.

## Who this book is for

The book assumes that you can read Rust and that you have seen a web page built with HTML, CSS, and JavaScript. It does not assume knowledge of the Windows message queue, the desktop's internal windows, or the Tauri framework. Each of those is explained where it first matters, and the words that appear again and again are collected in [Appendix A, Glossary](appendix-a-glossary.md).

If you only want to use UniDTK, read [Chapter 1](ch01-installing-and-running.md) and [Chapter 9](ch09-when-things-fail.md). If you want to change the shell, read the concept chapters in order. If you want to write a new widget, read [Chapter 7](ch07-project-a-module.md) after [Chapter 4](ch04-modules.md).

## How the book is organized

The book has ten chapters and two appendices. Chapters 1 through 6, 8, and 9 each explain one part of the system. Chapter 7 is different: it is a project that builds a module from start to finish, and it uses what the earlier chapters taught.

| Chapter | Subject |
| :--- | :--- |
| 1 | [Installing and running](ch01-installing-and-running.md) |
| 2 | [The desktop window](ch02-the-desktop-window.md) |
| 3 | [The kernel and its commands](ch03-the-kernel-and-its-commands.md) |
| 4 | [Modules](ch04-modules.md) |
| 5 | [Storage](ch05-storage.md) |
| 6 | [The page](ch06-the-page.md) |
| 7 | [Project: building a module](ch07-project-a-module.md) |
| 8 | [Themes and motion](ch08-themes-and-motion.md) |
| 9 | [When things fail](ch09-when-things-fail.md) |
| A | [Glossary](appendix-a-glossary.md) |
| B | [Ports, paths and tasks](appendix-b-ports-paths-tasks.md) |

Read the chapters from the front for the first time. Each one assumes the ones before it, but after that you can open any chapter alone, because each one links back to the words it uses.

## How to read it

Every chapter has the same shape. It opens with a paragraph that says what the chapter covers and what you will be able to do afterwards. The body follows one mechanism step by step: which function runs, on which thread, what it writes, and how it fails. It ends with a summary and a link to the next chapter.

The book teaches. The rules that the system must keep are stated separately, in the [reference](../reference/README.md). When the book and the reference seem to disagree, the reference is the one that holds the contract, and the code is what must match it. You can read the code yourself at any point, because every claim in the book names the file it comes from.

## Where things live in the repository

The repository has a flat root, which means that there is no wrapper folder between the checkout and the project. The rule is [repo.flat-root](../reference/kernel.md#r-repo.flat-root). The folders that matter for this book are these.

- `core/kernel/` holds the kernel: the window code, the IPC servers, the event broker, and the storage glue. Chapters 2 and 3 read from here.
- `core/api/` holds the contract crate `unidtk-api`. It is the only thing a module may depend on, and it holds the `Module` trait, the command types, and the event functions.
- `core/ui/` holds the page: the HTML, the CSS, and the JavaScript that run inside the window. Chapter 6 and Chapter 8 read from here.
- `modules/` holds one folder per module. A module is a vertical slice, with its Rust half and its page half next to each other.
- `opt/cli/` holds the source of `udtk`, the command-line bridge.
- `toolchain/` holds the one Cargo project that builds the binaries, plus the Tauri configuration and the installer hooks.
- `scheduler-tasks/` holds the Windows task templates, which contain the placeholder `@INSTALL_DIR@` and no absolute paths.
- `docs/` holds this book, the reference, the command-line book, and the session log.

Note: `target/` is the build cache. It is not part of the source, and it is not shipped. The only artifact that leaves the machine is the installer that Tauri writes under `target/release/bundle/`.

## Conventions

Code listings have a caption line above them. A caption such as `Filename: core/kernel/wallpaper.rs` names the file the listing comes from. Paths are written from the root of the repository, with forward slashes, unless they start with a drive letter or with an environment variable such as `%LOCALAPPDATA%`. A listing that has been shortened says so with a `// ...` line, and it is never the full text of a file.

A paragraph that starts with `Note:` gives a side remark that is useful but not required for the main line of the explanation. A paragraph that starts with `Warning:` describes a trap, usually a mistake that looks reasonable and breaks something.

When a sentence depends on a rule, the book cites the rule by its identifier and links to it, for example [ipc.socket-read-only](../reference/ipc.md#r-ipc.socket-read-only). The identifier is written as `area.short-name`. The text of the rule lives only in the reference, so the book links to it instead of repeating it. Commands written as `udtk doctor -ping` are real commands of the bridge, and the list of them is in [Chapter 1](ch01-installing-and-running.md) and in the [udtk book](../udtk/README.md).

Note: the `udtk doc` command opens this book in a browser. It renders the same Markdown files that are in the repository, so the links in the book work from there as well.
