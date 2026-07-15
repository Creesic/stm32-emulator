# External I/O Interface Design

## Goal

Define the contract external programs use to interface with the
emulator — feeding the inputs a real ECU's harness would carry,
observing the outputs firmware drives, and talking to firmware's own
host-facing protocol — so that feeders, engine simulators, dashboards,
and tuning tools can be written against a stable, documented surface
instead of against emulator internals.

The user-facing reference is `docs/external-io-interface.md`; this
document records the design decisions behind it. It consolidates and
supersedes the interface-relevant parts of the 2026-07-11 ecu_io
design and the 2026-07-15 harness-routing design, which remain the
record for their implementation details.

## Principles

1. **Transport is plain localhost TCP.** No custom IPC, no shared
   memory, no library to link. Anything that can open a socket can
   integrate, in any language, including existing closed tools
   (TunerStudio attaches to the CDC bridge unmodified). Both listeners
   bind 127.0.0.1 by default; exposure beyond the machine is a
   deliberate config change, not a default.
2. **One endpoint per abstraction level.** Byte-transparent firmware
   protocol traffic (USB CDC) and emulator-level signal injection
   (ecu_io) never share a socket. A tool that speaks rusEFI's console
   protocol needs no knowledge of the signal protocol and vice versa.
3. **The emulator computes nothing.** Signal *values* and *waveforms*
   are entirely the external program's job; the emulator only reflects
   the most recently received value into the peripheral model. This
   keeps the emulator honest (it models hardware, not engines) and
   keeps simulation logic iterable without rebuilding the emulator.
4. **Signals are named by physical harness position** (`ls3`, `av7`),
   never by tune role (`injector3`, `map`). Tune roles move when the
   user re-pins the tune; harness positions are wired once. External
   programs that need role names map them themselves, exactly like a
   human wiring a real harness against a tune sheet.
5. **Fail soft, fail loud.** No inbound line can crash or disconnect
   the bridge; every rejected line (unknown name, malformed value) is
   logged at warn level. External programs get forward compatibility
   (a feeder built for a future, larger signal set degrades to
   warnings on an older emulator) without silent data loss going
   unnoticed.

## The Contract

### USB CDC endpoint

Byte-transparent in both directions; no framing added or removed. The
emulator holds one client; a newcomer is rejected during the held
client's first 10 seconds and replaces it afterward. Rationale: a
tuning tool retrying every few seconds must not preempt its own
in-flight session (live-observed livelock), but a silently-dead socket
must not wedge the bridge until emulator restart. Queued data for a
vanished client is dropped — firmware's protocol layers (and real USB)
already tolerate in-flight loss on reconnect.

### ecu_io endpoint

Line protocol, `name=value\n`, decimal `i32`, both directions:

| Direction | Signals | Semantics |
|---|---|---|
| in | analog (`at*`, `av*`, `vbatt`) | pin millivolts, clamped 0–3300 at conversion; firmware's tune applies dividers on top |
| in | digital (`vr*`, `din*`) | 0/1 level; changes raise the pin's EXTI interrupt |
| out | `ls*`, `hs*`, `ign*` | one line per level *change* (deduplicated); dropped when no client |

Values persist across client disconnects (a feeder crash must not
yank all sensors to zero under a running firmware); unknown names are
warned and not stored; one client at a time, extras dropped on accept.

The signal map itself is per-board configuration, not protocol: it
lives in the example's `config.yaml` and the launcher's compiled-in
board module, kept entry-for-entry identical by tests. The protocol
grammar carries no board assumptions, so new boards reuse it as-is.

### Compatibility rules for external programs

- Treat the signal *set* as board-config-defined; discoverability is
  by documentation/config, not negotiation (there is deliberately no
  handshake — see Non-Goals).
- Never send to output names. Today they are accepted and stored
  (known residue: this can mask the next real output event); rely on
  it and a future fix will break you.
- Tolerate unknown *incoming* names: a future emulator may report
  output groups yours doesn't know.
- Do not infer wall-clock timing from event arrival: the emulator is
  not real-time paced (SysTick = 216,000 instructions ≈ firmware
  1 ms, at whatever instruction rate the host sustains).

## Alternatives considered

- **Binary/framed protocol (length-prefixed, protobuf):** rejected.
  The traffic is low-rate scalar updates; `name=value` lines are
  debuggable with `ncat` and trivially producible from any language.
- **Handshake/discovery message listing configured signals:**
  rejected for now (YAGNI) — every integration so far ships alongside
  a known config file. Revisit if third-party tools need to attach to
  arbitrary configs blind.
- **Timestamped/batched messages for waveforms** (e.g.
  `din1=1@+500us`): deliberately deferred, not rejected. It is the
  probable shape of the trigger-simulation answer (Future Work), but
  designing it before an engine simulator exists to consume it would
  be speculation.

## Error Handling

- Inbound: no line is fatal; unknown name / malformed value / missing
  `=` each warn and drop the line, connection stays up.
- Outbound: the event queue is size-capped; overflow drops oldest
  rather than growing unbounded; no client means events are discarded.
- Both endpoints: peer resets and FINs mark the client disconnected
  and re-arm the listener; the emulator never exits on socket errors.

## Verification

- The contract's Rust side is unit-tested (`ecu_io::` suite: parsing,
  rejection, clamping, output dedup, second-client, disconnect
  persistence) and the signal map is pinned name→pin by
  `tests/launcher_profile.rs`, including the config.yaml drift guard.
- Live evidence (2026-07-15): fed `av1`/`at3`/`vbatt`/`din1` plus an
  unknown name over :29002 against running rusEFI firmware — clean
  connect/parse/disconnect, no errors, boot (`usbStart()`) unaffected;
  recorded in `docs/proteus-f7-ecu-io.md`.
- Outstanding: firmware-level gauge confirmation via TunerStudio
  (human-in-the-loop) — the emulator-side contract does not depend on
  it, but an engine-simulator milestone should start by closing it.

## Future Work (explicitly out of scope here)

- **Engine simulator feeder:** an external process generating
  crank/cam tooth patterns and coherent sensor curves, reacting to
  injector/ignition events. Its central unsolved problem is pacing
  waveforms against a non-real-time emulator; candidate mechanisms
  (emulated-time feedback signal on the wire, timestamped scheduling
  extension above, or an instruction-count query channel) should be
  designed with that milestone.
- **TIM output modeling**, so PWM-driven functions (idle, boost)
  become observable output signals.
- **Rejecting inbound writes to output names**, closing the masking
  residue once nothing depends on the current permissiveness.
