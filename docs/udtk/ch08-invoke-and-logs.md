# Chapter 8. `invoke` and `logs`

## 8.1 `udtk invoke`

```bash
udtk invoke <cmd> [json-args]
```

`json-args` is a JSON object. The reply is printed pretty. Examples:

```bash
udtk invoke ping
udtk invoke vpn_status
udtk invoke logs '{"limit":10,"level":"ERROR"}'
```

| Failure | Message | Exit |
|---|---|---|
| No command given | `usage: udtk invoke <cmd> [json-args]` | 1 |
| Arguments are not JSON | `bad json args: ...` | 1 |
| Name not in the inventory | `unknown cmd: <name>` | 1 |
| A write over the socket | `command '<name>' is not available on the socket door: it changes state, and write commands stay on the UI door` | 1 |
| A UI-only read over the socket | `command '<name>' is not available on the socket door: it is served by the UI door only` | 1 |
| The module does not handle the command | `module '<id>' is registered but does not handle '<cmd>'`, or the module's own error text | 1 |

## 8.2 `udtk logs`

```bash
udtk logs [--limit N] [--since ID] [--level LEVEL] [--source SOURCE]
```

The default limit is **50**. `--since` returns the records after a journal id. `--level` and `--source` are passed as strings, for example `--level ERROR` or `--source frontend`. The command calls the socket command `logs` (its name comes from the inventory constant `SIMULATE_LOGS`, so a rename cannot leave `logs` talking to a dead name). The journal itself is `unidtk_logs.redb`, read by [core/kernel/log_store.rs](../../core/kernel/log_store.rs).

Examples:

```bash
udtk logs --level ERROR          # expect [] when the app is healthy
udtk logs --source frontend --limit 30
```

`udtk logs` answers only as far as the journal goes. A page error that happened in the page itself is collected by the page harness and read with `getErrors`, which `gui` does for every step (see [Chapter 9](ch09-gui-driving-the-page.md#chapter-9-gui-driving-the-page)).
