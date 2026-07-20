# PM diagnostic codes

When part of PM fails in a way worth pinpointing, it shows a short **diagnostic code** like
`SBX-2104`. These codes are **stable** — quoting one in a bug report maps it to the exact spot in
the code, which turns "it didn't work" into something we can find and fix quickly.

**When you see a code, include it** — verbatim, with the message next to it and your operating
system — in your report. For a normal bug, open a GitHub issue; for a **security vulnerability**,
follow [`SECURITY.md`](SECURITY.md) instead of opening a public issue.

## `SBX-####` — sidecar worker sandbox

PM reads and converts your files in a background **worker** process. On supported systems that
worker runs inside an OS **sandbox**: no network access, and read access to only a handful of
folders — so even a malicious file can't use PM's file reader to reach the internet or your vault.
If the sandbox can't be set up, PM keeps working with the worker **unconfined** (this is a
defence-in-depth layer on top of the already-offline worker and at-rest encryption, not a gate) and
reports *why* with one of these codes. You can see the live state any time in **Developer mode →
Sidecar sandbox**, and every fall-back is written to the app log as `running unconfined — [SBX-…]`.

Ranges: `1xxx` cross-platform · `2xxx` Windows · `3xxx` macOS · `4xxx` Linux.

| Code | What it means |
|------|---------------|
| `SBX-1101` | Couldn't locate the worker's runtime folder, so the sandbox couldn't be anchored. |
| `SBX-1102` | Couldn't resolve the on-device model-cache folder. |
| `SBX-1103` | Couldn't read the base Python interpreter's location from the virtual environment. |
| `SBX-1104` | Couldn't create the staging folder the worker reads its inputs from. |
| `SBX-1105` | Couldn't copy an input file into the staging folder (that one request runs unsandboxed). |
| `SBX-1106` | The sandboxed worker process failed to launch. |
| `SBX-2101` | Windows: couldn't create the AppContainer profile or derive its identity. |
| `SBX-2102` | Windows: couldn't grant the worker access to the staging folder. |
| `SBX-2103` | Windows: couldn't grant the worker access to the sidecar-script folder. |
| `SBX-2104` | Windows: couldn't grant the worker access to the model-cache folder. |
| `SBX-2105` | Windows: couldn't grant the worker access to the Python / virtual-environment folders. |
| `SBX-4101` | Linux: couldn't build the Landlock filesystem ruleset on a kernel that supports it. |
| `SBX-4102` | Linux: no network-blocking filter is available for this CPU architecture. |
| `SBX-4105` | Linux: **degraded, not failed** — Landlock isn't available (kernel older than 5.13, or its security module is off), so the worker's network is still blocked but its filesystem is *not* restricted. |
| `SBX-4106` | Linux: the sandboxed worker couldn't load its libraries under the sandbox, so it was re-run unconfined. |

`3xxx` (macOS) codes are added as sandboxing lands on that system.

> On Linux, "confined" means both **Landlock** (filesystem) and **seccomp** (network) are active;
> `SBX-4105` is the honest middle state where only the network half applies.

---

*Maintainers: the authoritative list is the `sbx` module in
[`src-tauri/src/sidecar.rs`](src-tauri/src/sidecar.rs). Keep this table in sync when you add a code,
and **never renumber or reuse a shipped one** — they travel in logs and bug reports.*
