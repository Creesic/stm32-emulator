# ECU I/O trigger-wheel generator (instruction-clock-paced crank signal)

Date: 2026-07-16
Status: approved in-session (user: "lets fix the clock in AngeES")

## Problem

External programs cannot deliver a decodable crank waveform over the ecu_io
TCP bridge. The emulator has no real-time pacing: firmware time advances at
one CPU cycle per executed instruction (216 MHz convention), at whatever rate
the host manages, while a TCP feeder paces edges in wall-clock time. Two
independent failure modes follow:

1. Tooth intervals measured by the firmware (TIM5 time) bear no fixed
   relation to the feeder's wall-clock spacing, and jitter freely with host
   load — the rusEFI trigger decoder rejects the pattern.
2. `EcuIo::check_digital_edges` level-samples once per poll window, so any
   toggle pair that arrives batched (TCP coalescing) collapses into no edge.

Live evidence: with all transport bugs fixed (bug-142..146), a wall-clock
40 Hz din1 square wave reaches the decoder and is correctly rejected —
`primary trg error: too many teeth: exp 58/0 got 58/0`. The waveform, not the
transport, is now the blocker. This was documented as Future Work in
`docs/external-io-interface.md`.

## Design

Generate the crank waveform *inside* the emulator, paced by the instruction
clock (the same timebase as SysTick and TIM5, so tooth timing is exact by
construction), and let the external program command only the rotation speed.

### Emulator: `trigger_wheel` in the ecu_io config

```yaml
devices:
  ecu_io:
    - listen: 127.0.0.1:29002
      pins: [...]
      adc_channels: [...]
      trigger_wheel:
        signal: trigger_rpm   # inbound name=value line that sets the RPM
        pin_signal: din1      # existing input pin signal the wheel drives
        teeth: 60
        missing: 2
```

- The section is optional; absent means no generator (fully backward
  compatible).
- `signal` (`trigger_rpm=<i32>`) is accepted inbound like any other known
  name: integer RPM, clamped to 0..=30_000. `0` stops the wheel and holds the
  current level. It is added to `known_names` so it is not rejected.
- `pin_signal` names an existing `direction: input` pin entry (`din1` → PC6
  for the stock Proteus tune). The generator owns that signal's level while
  RPM > 0 by writing `values[pin_signal]` — the *existing*
  `check_digital_edges` pass then detects the change and raises the EXTI
  line, identical to a line received over TCP. No new edge-delivery path.
  A feeder that simultaneously streams `din1=...` lines fights the generator
  (last write per poll wins); documented as unsupported.

### Wheel math and pacing

- State: `position` (f64 fractional revolutions, wraps at 1.0),
  `last_instructions` (u64), `rpm` (i32).
- Advanced from `ExtDevices::poll` (every `PUMP_EVENT_INST_INTERVAL + 1` =
  1024 instructions) with the current `NUM_INSTRUCTIONS` passed in by the
  caller, keeping the generator pure enough to unit test:
  `position += delta_instr * rpm / (60 * 216e6)`.
- Level: slot = `floor(position * teeth)`; tooth present when
  `slot < teeth - missing`; level high during the first half of a slot
  (50% duty), matching AngeES's `EcuIoClient` wheel model.
- Timing error is bounded by the poll interval: 1024 instructions ≈ 4.7 µs
  firmware time, ≈1% of a half-slot at 1200 RPM, ≈9% at 10k RPM — well
  inside the decoder's gap tolerance.

### Launcher parity

`src/launcher/boards/proteus_f7.rs`'s generated YAML gains the same
`trigger_wheel` block; `tests/launcher_profile.rs`'s drift-guard keeps the
two surfaces identical.

### AngeES: `EcuIoClient` sends RPM, not edges

`EcuIoClient::update` no longer synthesizes din1 edges from the crank angle.
It receives the engine's RPM each step and sends `trigger_rpm=<rounded rpm>`
when the value changed by ≥1 RPM, rate-limited to one line per 100 ms (plus
one refresh after reconnect). The 60-2 math, per-step edge emission, and the
TDC-offset knob's effect on the wire go away; the F2 overlay shows the last
RPM sent and connection state. `trigger_rpm=0` is sent when the engine stops.

## Rejected alternatives

- **Pace edges in AngeES against an emulator-time feedback channel**: still
  subject to TCP batching/jitter, needs a new feedback protocol, and cannot
  beat the poll-window edge collapse. Strictly worse than generating at the
  source of truth.
- **Timestamped-edge protocol with emulator-side replay scheduling**: more
  faithful to arbitrary waveforms (future cam/VVT work may want it), but far
  more machinery than the single-value RPM interface AngeES needs today.
  YAGNI; the config schema leaves room to add it later.

## Testing

- Unit: wheel level math (tooth/gap/duty boundaries), position advance and
  wrap across RPM changes, `trigger_rpm` accepted/clamped/stop semantics,
  config deserialization, generator-drives-`values`-and-EXTI via the
  existing edge test pattern.
- Drift-guard: launcher YAML vs `proteus_f7/config.yaml` equality for the
  new section.
- Live: python client sends `trigger_rpm=1200` over :29002 → TS output
  channels read RPM ≈ 1200 with `trigErr` stable and sync counter climbing,
  with self-stimulation OFF.
