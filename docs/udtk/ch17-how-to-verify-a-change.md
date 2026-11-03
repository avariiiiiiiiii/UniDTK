# Chapter 17. How to verify a change

The rule in [AGENTS.md](../../AGENTS.md) is that a change is not done when it compiles. It is done when it has been run and checked through the bridge. The steps below use `udtk` only. Do not call `cargo` or `node` directly. The two exceptions are the build itself, which is `udtk build`, and the tests, which `udtk doctor -check` runs.

1. **Build and install:** `udtk build`. It must end with `build to install to start to verify OK` and an empty ERROR list.
2. **Health:** `udtk doctor -quick` and then `udtk doctor -all`. Both must exit 0.
3. **Journal:** `udtk logs --level ERROR` (expect `[]`) and `udtk logs --source frontend --limit 30`.
4. **Page:** `udtk gui tree`, and `udtk gui js:window.__UNIDTK_HARNESS__.getErrors(true)`. The page must have no errors.
5. **Probe:** `udtk doctor -simulate` with the app running. It must print `ALL CHECKS PASSED`.
6. **Visual:** `udtk shot`. The screenshot is the mandatory check of the layout after any change to the interface or the kernel. Open the file and look at it.
7. **Behaviour of a write:** change the state through the page (`udtk gui click:...`, or `js:` through the page's door), then read the consequence back with a read command.

For a change to the transport, add the `shadow` comparison of Chapter 4: set `sessionStorage["unidtk.transport"]` to `shadow` through `udtk gui js:...`, run the scenario, and read `getTransportState()`. The divergence count must be zero. Then reset the key and check that the transport is `tauri` again.
