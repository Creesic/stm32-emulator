# Proteus F7 ECU I/O Design

## Goal

Let an external process drive the digital and analog signals the Proteus F7
rusEFI firmware reads as engine sensor inputs (crank/cam trigger, MAP, TPS,
CLT, IAT, vbatt), and observe the outputs it drives in response (injector and
ignition driver pins), without any physical hardware. The external process
already computes these signals by its own means; the emulator's job is only
to expose them to firmware and report firmware's output pin state back out.

The host connection is TCP only, following the same pattern as the existing
`usb_cdc_tcp` bridge.

## Scope and Boundary

This milestone covers a first, deliberately small signal set, not the full
board pinout:

- Digital inputs: crank trigger (PC6), cam trigger.
- Analog inputs: MAP (PC0), TPS (PC1), CLT (PB0), IAT (PC5), vbatt (PA7).
- Digital outputs (observed only): a small number of injector/ignition driver
  pins, enough to confirm firmware is computing and issuing commands.

The emulator owns:

- A minimal STM32F7 ADC register model, sufficient for ChibiOS's ADC driver
  to complete a conversion and read back a configured value — not a
  cycle-accurate or fully-featured ADC.
- Reflecting an externally supplied level onto a named GPIO input pin, using
  the existing named-pin callback mechanism in `gpio.rs`.
- Reporting a named GPIO output pin's firmware-driven level back to the
  external process when it changes.
- A binary-safe TCP listener, restricted by default to 127.0.0.1, single
  client at a time.

The external process owns:

- Computing the actual signal waveforms (trigger tooth pattern and timing,
  sensor voltage curves, etc.) — this is explicitly not the emulator's job.

Explicitly out of scope for this milestone:

- Any trigger-wheel pattern generation inside the emulator. The emulator
  does not know or care what tooth pattern a signal represents; it only
  reflects whatever level it's told.
- Real-time pacing of the instruction loop. This emulator runs as fast as
  the host allows and has no notion of a CPU clock rate (SysTick and DWT
  CYCCNT both simply increment once per instruction). Aligning the
  emulator's sense of time with the external process's real-time pacing was
  considered and explicitly rejected as unnecessary for this milestone —
  signals are applied as soon as they're received, whatever that happens to
  correspond to in firmware's perceived timeline.
- Modeling the full board pinout. Only the signals listed above.

## Architecture

Three new pieces, following patterns already established in this codebase:

1. **`src/ext_devices/ecu_io.rs`** — a new `EcuIo` ext_device. Owns a single
   nonblocking TCP listener (same pattern as `UsbCdcTcp`), parses incoming
   `name=value` lines, and stores the current value per named signal. Also
   registers GPIO write-callbacks (via `GpioPorts`) on configured output
   pins so it can push `name=value` lines back out when firmware drives them.
2. **GPIO extension** — no new mechanism needed. `EcuIo` uses the *existing*
   named-pin read/write callback registration in `gpio.rs` (already present,
   currently used only by `sw_spi`/`touchscreen`) to supply input pin levels
   and observe output pin levels.
3. **`src/peripherals/adc.rs`** — a new `Adc` peripheral, one instance per
   SVD-declared ADC (ADC1/2/3), registered through
   `Peripherals::register_peripheral`'s existing constructor-chain pattern.
   Each configured channel's value is looked up from `EcuIo` by name and
   converted to a 12-bit raw count for `DR`. The exact register sequence
   (CR2/SR/SQR/DR interaction) will be modeled by reading ChibiOS's actual
   STM32F7 `hal_adc_lld.c` first, then verified against a live firmware
   capture — the same methodology used for the OTG-FS work, rather than
   guessed upfront.

One open question to resolve during implementation, not before: does
rusEFI's trigger input driver read the crank/cam pins via EXTI edge
interrupts, or by polling GPIO IDR? If EXTI-driven, `EcuIo` setting a new
level on a change needs to also raise the corresponding EXTI interrupt
(via the existing NVIC `set_intr_pending` mechanism) so firmware notices the
edge; if polled, updating the read-callback's return value is sufficient on
its own. This will be settled by reading ChibiOS's EXTI driver source and,
if needed, a live capture — not speculated here.

## Configuration

A new external device configuration, keyed by device kind `ecu_io` (not
tied to a single peripheral name, since it spans GPIO ports and ADC
instances):

```yaml
devices:
  ecu_io:
    listen: 127.0.0.1:29002
    pins:
      - { name: crank, pin: PC6, direction: input }
      - { name: cam,   pin: PE7, direction: input }
      - { name: inj1,  pin: PD7, direction: output }
      - { name: ign1,  pin: PD4, direction: output }
    adc_channels:
      - { name: map, pin: PC0 }
      - { name: tps, pin: PC1 }
      - { name: clt, pin: PB0 }
      - { name: iat, pin: PC5 }
      - { name: vbatt, pin: PA7 }
```

`name` is the alias used in the TCP protocol; `pin` resolves through the
existing `Pin::from_str` mechanism already used by `SoftwareSpiConfig`.

## Protocol

Simple line-oriented text over TCP, one message per line:

- External process → emulator: `name=value`. For digital input pins, `value`
  is `0` or `1`. For ADC channels, `value` is millivolts.
- Emulator → external process: `name=value`, pushed whenever an observed
  output pin's driven level changes. Same `0`/`1` format.

Malformed lines (unknown name, non-numeric value) are logged and ignored,
not treated as a fatal error — consistent with this project's general
approach to unexpected firmware/external input (see the existing unmapped-
memory-access handling in `emulator.rs`).

Connection semantics match `UsbCdcTcp`: one client at a time; a second
connection attempt is accepted then immediately dropped; disconnecting
leaves the emulator's internal signal state untouched and the listener
ready for the next client.

## State Flow

1. The listener opens as soon as the emulator starts, independent of
   firmware's own boot sequence. Before any message is received for a given
   name, digital input pins read low (0) and ADC channels read 0mV — there
   is no separate "default value" config field in this milestone.
2. At any point, a connected client may send `name=value` lines. Each
   updates `EcuIo`'s stored value for that name immediately.
3. On the next relevant GPIO IDR read or ADC conversion, firmware sees the
   most recently received value for that pin/channel.
4. When firmware drives a configured output pin to a new level, `EcuIo`'s
   GPIO write-callback fires and a `name=value` line is sent to the
   connected client, if any. If no client is connected, the event is
   dropped (not queued) — this is a live view, not a log.

## Error Handling

- No TCP client connected: input pins/ADC channels hold their last known
  (or configured default) value; output pin changes are simply not
  reported anywhere but the existing `-v` trace logging.
- A second TCP client while one is active is rejected, matching
  `UsbCdcTcp`.
- Malformed protocol lines are logged and ignored.
- An ADC conversion requested on an unconfigured channel reads back 0,
  with a trace warning — consistent with how unmodeled peripherals already
  behave in this codebase.
- An ADC millivolt value outside the real 0–3300mV range (VREF+) is clamped
  to the nearest valid 12-bit count (0 or 4095) rather than wrapping or
  panicking.

## Verification

Automated tests cover:

- `Adc` register-level behavior: starting a conversion, reading back a
  configured channel value, interrupt/status bit behavior matching what
  ChibiOS's driver expects.
- `EcuIo` command parsing and dispatch: valid and malformed `name=value`
  lines, output-change events pushed to a connected test client.

Live verification, made straightforward by the already-working TunerStudio
connection from the virtual-USB work: push a value (e.g. `map=1500`) over
the new TCP channel, then query rusEFI's live sensor data over the existing
USB CDC bridge and confirm it reports back the corresponding reading — a
real end-to-end check rather than only unit-level coverage.

## Non-Goals

- Trigger-wheel tooth pattern generation or any waveform computation inside
  the emulator.
- Real-time pacing or any CPU-clock-rate modeling.
- Full board pinout coverage — only the signals listed in Scope.
- A bidirectional negotiation protocol beyond plain `name=value` lines.
