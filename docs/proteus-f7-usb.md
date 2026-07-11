# Proteus F7 Virtual USB CDC

The emulator models enough of the STM32F767 OTG-FS device controller for the
unmodified Proteus firmware's real USB CDC stack to run, then bridges CDC
bulk traffic to one local TCP client. There is no physical USB device, no
Windows USB driver, and no Windows COM port involved — this is CDC-over-TCP.

## Launch

```
cd proteus_f7
cargo run --release --bin stm32-emulator -- config.yaml -v
```

`config.yaml`'s `devices.usb_cdc_tcp` entry opens a listener on
`127.0.0.1:29000` as soon as the emulator starts, independent of when
firmware itself gets around to configuring OTG-FS.

## Connecting

Connect with any raw TCP client — this is not a serial port:

```
ncat 127.0.0.1 29000
```

A Windows COM port or serial terminal will not work here; there is no COM
device to open. Bytes written to the socket become CDC bulk OUT data once
firmware has enumerated and configured the USB interface; CDC bulk IN bytes
firmware sends are written back to the socket unmodified, with no framing
beyond the payload itself.

## One-client rule and disconnects

Only one client may be connected at a time. A second connection attempt
while one client is active is accepted then immediately dropped. If the
active client disconnects, the listener stays open and accepts the next
connection; USB enumeration state is unaffected by TCP disconnects.

## Verified

Connecting one TCP client and sending a byte produces exactly this in the
emulator's log (`-v`):

```
[clk=145620993 pc=0x002793e0] INFO  USB CDC TCP client connected from 127.0.0.1:55343
[clk=145620993 pc=0x002793e0] INFO  Virtual USB host attached
[clk=218761551 pc=0x00234a90] INFO  USB CDC TCP client disconnected
```

"Virtual USB host attached" confirms the listener accepted the connection
and the modeled OTG-FS controller raised a bus reset toward firmware.

## Current limitation

The emulator's virtual USB host drives bus reset and endpoint-zero control
transfers (`GET_DESCRIPTOR`, `SET_ADDRESS`, `SET_CONFIGURATION`,
`SET_LINE_CODING`, `SET_CONTROL_LINE_STATE`) deterministically, and this has
been confirmed byte-for-byte against the real firmware's register accesses
(see `proteus_f7/usb_trace_notes.md`). Bulk IN/OUT forwarding is wired to
endpoint 2 (the real CDC data endpoint, confirmed from firmware source —
see usb_trace_notes.md's "Bulk endpoints" section), but whether the virtual
host's 5-stage control sequence actually reaches "configured" against real
firmware is not yet confirmed end to end: reading the firmware's own ChibiOS
USB driver source surfaced an open question about whether this project
advances past the `GET_DESCRIPTOR` stage prematurely (usb_trace_notes.md's
"Open question found while reading the source"). A real TunerStudio-style
protocol exchange has not yet been attempted.
