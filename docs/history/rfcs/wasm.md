# Decision: No WASM (History)

Recorded in the old `docs/architecture.md`. The rule that follows from it is [modules.compiled-crates](../../reference/modules.md#r-modules.compiled-crates).


**WASM is not used.** Reasons:
- an extra layer (a second runtime);
- isolation is already handled by native Rust modules (the Lua sandbox mentioned at the time was never built);
- there is no need to run third-party untrusted code — modules are trusted.
