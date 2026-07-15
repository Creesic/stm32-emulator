# External I/O Interface

How an external program talks to the emulator while it runs vendor
firmware. The emulator exposes two independent, localhost-only TCP
endpoints; everything an outside process can feed or observe goes
through them. Design rationale lives in
`docs/superpowers/specs/2026-07-15-external-io-interface-design.md`.

| Endpoint | Default address | Carries | Config key |
|---|---|---|---|
| USB CDC bridge | `127.0.0.1:29000` | Raw CDC payload bytes (e.g. the TunerStudio/rusEFI console protocol) | `devices.usb_cdc_tcp` |
| ECU I/O bridge | `127.0.0.1:29002` | Line-oriented `name=value` signal traffic | `devices.ecu_io` |

Both are plain TCP — connect with anything (`ncat`, a Python socket, a
real tuning tool). Ports and the whole signal map are set per example
in its `config.yaml` (see `proteus_f7/config.yaml`); the values below
are the Proteus F7 defaults.

## USB CDC bridge (:29000)

A byte-transparent stand-in for the USB host: whatever you write to
the socket arrives at firmware as CDC OUT data, and firmware's CDC IN
data is written back to the socket. No framing is added or removed —
TunerStudio can be pointed straight at this port.

Connection rules:

- One client at a time.
- A new connection while a client is held is **rejected for the first
  10 seconds** of the held client's life (protects an in-flight
  session from its own fast retries), and **replaces** the held client
  after that (recovers from silently-dead sockets without restarting
  the emulator).
- Data queued for a disconnected client is dropped, not replayed.

## ECU I/O bridge (:29002)

Drives the signals firmware reads as sensor inputs and reports the
output pins firmware drives. The emulator generates none of these
signals itself — it reflects exactly what it is told, when it is told.

### Protocol

One message per line, newline-terminated, both directions:

```
name=value
```

- `value` is a decimal integer (`i32`); whitespace around name and
  value is trimmed.
- **Analog inputs** (`at*`, `av*`, `vbatt`): value is millivolts *at
  the MCU pin*, clamped to 0–3300 (VREF+) at conversion time. Any
  divider/scaling the firmware's tune configures (battery divider,
  temp-sensor pullup math) is applied by firmware on top — send
  pin-level voltage, not engineering units.
- **Digital inputs** (`vr*`, `din*`): `0` or `1` (any nonzero value
  reads high). Level changes raise the pin's EXTI interrupt path, so
  edges are events to firmware, not just polled levels.
- **Outputs** (`ls*`, `hs*`, `ign*`): sent *to* you, one
  `name=0`/`name=1` line per **level change** (repeated same-level
  drives are deduplicated). There is nothing to send for these. If no
  client is connected the events are dropped — this is a live view,
  not a log. Only GPIO-driven writes are observable; outputs firmware
  routes through hardware timer PWM (idle, boost) produce no events.

Error handling — all non-fatal, connection stays up:

- Unknown signal name → `WARN ECU IO: unknown signal name ...`,
  ignored, **not stored**. A typo'd name never silently succeeds.
- Malformed line (no `=`, non-integer value) → warned and ignored.

Connection rules: one client at a time; additional connections are
dropped while one is held; disconnecting leaves all previously fed
values in place and the listener ready for the next client.

### Signal map (Proteus F7)

Names are physical harness positions, not tune roles — the tune
decides which position carries MAP or injector 3. Canonical sources
are `proteus_f7/config.yaml` and `src/launcher/boards/proteus_f7.rs`
(kept identical by `tests/launcher_profile.rs`); this table mirrors
them.

| Group | Names | Pins (in order) | Direction |
|---|---|---|---|
| Lowside outputs | `ls1`–`ls16` | PD7, PG9–PG14, PB4–PB9, PE0–PE2 | out (observed) |
| Highside outputs | `hs1`–`hs4` | PA9, PA8, PD15, PD14 | out (observed) |
| Ignition outputs | `ign1`–`ign12` | PD4, PD3, PC9, PC8, PC7, PG8–PG2 | out (observed) |
| Analog Temp | `at1`–`at4` | PC4, PC5, PB0, PB1 | in (mV) |
| Analog Volt | `av1`–`av11` | PC0–PC3, PA0–PA6 | in (mV) |
| Battery sense | `vbatt` | PA7 | in (mV) |
| VR triggers | `vr1`, `vr2` | PE7, PE8 | in (0/1) |
| Digital inputs | `din1`–`din6` | PC6, PE11, PE12, PE14, PE13, PE15 | in (0/1) |

With a stock Proteus tune: MAP=`av1`, TPS=`av2`, CLT=`at3`, IAT=`at2`,
crank trigger=`din1`, cam=`din2`, injector 1=`ls1`, coil 1=`ign1`.

### Timing model — read before writing a feeder

The emulator has **no real-time pacing**: it executes instructions as
fast as the host allows, and firmware's sense of time comes from
instruction-count-driven timers (SysTick fires every 216,000
instructions ≈ firmware's 1 ms). Wall-clock seconds on your side do
not map to firmware seconds at any fixed rate, and the rate varies
with host load.

Consequences:

- Static or slowly-varying values (sensor levels) work naturally: a
  value applies from the moment it is received until replaced.
- Precisely-timed waveforms (crank/cam tooth patterns) **cannot** be
  generated accurately by sleeping wall-clock time between edges. A
  trigger feeder needs a feedback signal to pace against; today the
  practical proxy is coarse (e.g. pacing against observed output
  events). Treat high-fidelity trigger simulation as an open design
  problem, not a solved one — see the design doc's Future Work.

### Minimal client

```python
import socket, time

s = socket.create_connection(("127.0.0.1", 29002))
s.sendall(b"av1=1500\n")      # MAP pin at 1.5 V
s.sendall(b"at3=2000\n")      # CLT pin at 2.0 V
s.sendall(b"vbatt=1800\n")    # pin-level; tune's divider scales it up
s.sendall(b"din1=1\n")        # crank input high (fires EXTI edge)

s.settimeout(0.5)             # then read any output events
while True:
    try:
        data = s.recv(4096)
    except socket.timeout:
        break
    for line in data.decode().splitlines():
        print("firmware drove:", line)   # e.g. "ls1=1"
```

## Known limitations

- Output names are currently also *accepted* inbound: sending `ls1=1`
  stores a value that can mask (deduplicate away) the next genuine
  firmware-driven event for that name. Don't write to output names.
- TIM-PWM-driven outputs are invisible (no timer output model yet).
- One client per endpoint; there is no multiplexing or replay.
- The protocol is unversioned plain text; see the design doc for the
  compatibility rules external programs should follow.
