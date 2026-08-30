# Phase 7.5: control plane + MCP

Make the running app remotely drivable through one general-purpose API,
then expose it to LLM clients via MCP. Design decision (user-approved
2026-08-30): the script action grammar IS the shared semantic layer; every
transport (CLI, MCP, future REST) translates into it. The MCP surface is a
curated task-shaped façade over that layer, not a 1:1 mirror.

## Decisions

- New crate `neowon-mcp` with deps **rmcp + tokio + serde/serde_json +
  schemars** (user-approved; confined to this crate). The app gains only
  `std` networking; library crates stay dependency-clean.
- The control socket speaks the script grammar for commands and adds
  `get …` queries with JSON replies. Queries live in the socket protocol,
  NOT in the NEOWON_SCRIPT file grammar (a file script has nowhere to
  send a reply) — deviation from the first sketch, recorded here.
- JSON emitted by hand in the app (dump_json style) — no serde in the app.

## Hard rules

- Sim only for all automated runs; MCP demo spawns `neowon-app --sim`.
- Socket is OFF by default; `NEOWON_CONTROL=<port>` binds 127.0.0.1 only.
- Script grammar and preset names stay stable; new verbs are additive.
- Every MCP tool maps onto existing script actions/queries — no scope
  logic in the MCP crate.

## Work items

### 1. Control socket (`neowon-app/src/control.rs`)

`NEOWON_CONTROL=<port>` → accept thread on 127.0.0.1:<port>, line
protocol: one request per line, one JSON object per line back.

- Command lines (anything the script grammar accepts): parsed, injected
  into the Script queue, ack `{"ok":true}` (or `{"ok":false,"error":…}`
  on parse failure). Fire-and-ack: effects apply on the next frame.
- Query lines:
  - `get status` → running, frames seen, backend caps/serial, stimulus,
    recorder frames, history position, last export.
  - `get config` → sample rate, trigger position, acq, per-channel
    settings, trigger (kind-specific), holdoff; display (mode, persist,
    palette, crt), math, fft, pf.
  - `get measure` → per slot: the 18 metrics (value or null) + stats
    (mean/min/max/σ/n) at full float precision.
- Plumbing: `Request { line, reply: SyncSender<String> }` over a
  crossbeam channel; a `control::poll` system drains it each frame
  (before `run_script` so commands land the same frame).

### 2. `neowon-mcp` crate (bin)

rmcp stdio server. Connects to the app's control socket
(`--connect 127.0.0.1:PORT`), or spawns `neowon-app --sim` itself with
`NEOWON_CONTROL` set (`--spawn-sim`, the zero-setup path).

Tools (schemars-described):
- `scope_status`, `scope_config`, `measurements` — the three queries.
- `configure_channel { ch, enabled?, volts_div?, coupling?, probe?, offset? }`
- `configure_trigger { source, kind, slope?/condition?/width?…, level?, sweep }`
- `configure_horizontal { sample_rate?, trigger_position? }`
- `run { on }`, `autoset`, `set_stimulus { name }` (sim)
- `screenshot { roi? }` → `shot` to a temp PNG, wait for the file,
  return MCP image content (Claude sees the screen).
- `record { on }` / `export { format }`
- `exec_script { script }` — the escape hatch: full grammar, 100%
  control coverage, documented in the tool description.

### 3. Tests

- Unit: JSON emitters (shape + escaping); MCP arg→script-line mapping.
- Integration (`--ignored`, opens a window): spawn app `--sim` with
  `NEOWON_CONTROL`, drive it over TCP — set vdiv, read it back via
  `get config`, check `get measure` sanity, bad line → error reply.
- MCP end-to-end (`--ignored`): spawn `neowon-mcp --spawn-sim`, speak
  raw JSON-RPC over its stdio (initialize → tools/list → tools/call),
  assert a measurement round-trip.

## Done when

Workspace green (`fmt`, `clippy -D warnings`, tests incl. new ignored
suites); README gains an MCP section; PLAN.md §4 status updated.
