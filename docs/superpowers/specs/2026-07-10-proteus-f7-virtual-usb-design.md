# Proteus F7 Virtual USB Design

## Goal

Run the existing Proteus F7 firmware’s real STM32 USB device stack without a
physical USB bus. The emulator will model the STM32F767 OTG-FS device
controller closely enough for firmware enumeration and CDC operation, then
bridge CDC bulk traffic to one local raw TCP client.

The host connection is TCP only. This milestone does not create a Windows USB
device, a Windows COM port, or a physical-device passthrough path.

## Scope and Boundary

The current firmware must first pass its known FLASH_ACR startup wait. USB
modeling begins only after a trace proves the firmware writes the OTG-FS
registers.

The emulator owns:

- OTG-FS global, device, endpoint, FIFO, and interrupt state that the
  observed firmware accesses.
- A deterministic virtual USB host state machine.
- Standard USB reset/enumeration and CDC control requests needed for the
  firmware to make its serial USB channel ready.
- A binary-safe TCP listener, restricted by default to 127.0.0.1.

The firmware owns:

- USB descriptors.
- CDC class behavior.
- TunerStudio protocol framing and application responses.

No firmware-memory patch or ChibiOS object injection may be used to shortcut
USB readiness or protocol traffic.

## Architecture

The new controller is a peripheral implementation named OtgFs. It receives
MMIO reads and writes for the F767 OTG-FS SVD ranges and exposes a narrowly
modeled device-side DWC2 state machine.

The controller has four internal parts:

1. Register model: global and device-mode configuration, interrupt status and
   masks, endpoint control/size/interrupt registers, and power/clock state.
2. FIFO model: endpoint zero setup/control payloads plus CDC bulk IN and OUT
   payload queues.
3. Virtual host: deterministic connect, bus reset, descriptor requests,
   configuration selection, and CDC line-state control transfers.
4. TCP bridge: one nonblocking loopback client. Firmware-to-host CDC bytes
   flow to TCP; TCP bytes are queued into CDC OUT and delivered through the
   modeled endpoint interrupt path.

The existing NVIC implementation delivers the OTG-FS interrupt selected from
the SVD/vector table. USB transfer completion is represented by controller
interrupt bits, not by direct firmware callbacks.

## Configuration

A new external device configuration attaches to OTG_FS. It provides:

- Listener address, default 127.0.0.1:29000.
- Maximum buffered host-to-device and device-to-host byte count.
- Optional explicit endpoint pair only when trace evidence shows a
  non-default CDC layout.

The Proteus F7 example configuration will opt in only after the USB
initialization trace identifies the exact OTG-FS instance and interrupt.

## State Flow

1. Firmware enables and configures OTG-FS.
2. The virtual host signals connection and reset through modeled global and
   device interrupt status.
3. Endpoint zero receives setup packets for descriptor/configuration and CDC
   line-state requests.
4. The firmware replies through endpoint FIFO writes; the controller records
   completions and raises the corresponding IRQ.
5. Once configured, the TCP listener accepts one loopback client.
6. CDC bulk IN bytes are sent to TCP. TCP bytes are queued as CDC bulk OUT
   packets and become visible only when endpoint state and interrupt masks
   permit delivery.

When no TCP client is connected, USB enumeration still completes. Firmware
transmit data is retained only within the configured bounded buffer, then
dropped with a trace warning rather than blocking emulation indefinitely.

## Error Handling

- Any unmodeled OTG register access remains trace-visible and does not claim
  successful USB behavior.
- Configuration with a non-loopback listener requires an explicit address;
  no network exposure is implicit.
- A second TCP client is rejected while the first is active.
- TCP disconnect retains USB configuration and returns the bridge to
  listening state.
- FIFO overflow and invalid endpoint state set modeled error/interrupt state
  where observed behavior supports it; otherwise they produce a trace warning.

## Verification

Automated tests cover:

- OTG-FS reset/configuration and interrupt-mask behavior.
- Endpoint zero setup and response sequencing.
- CDC ready transition after the modeled host control sequence.
- Bulk IN FIFO to TCP and TCP to bulk OUT FIFO transfer using a loopback
  client on an ephemeral port.
- One-client enforcement and disconnect recovery.

Bring-up verification records the firmware’s observed OTG-FS register access,
the chosen interrupt vector, endpoint numbers, and the transition to its
USB-ready condition. A manual smoke test connects a local TCP client,
exchanges binary data, and confirms the firmware’s TunerStudio channel
responds without a physical USB device.

## Non-Goals

- USB host mode, USB HS, hubs, isochronous endpoints, mass storage, HID, and
  a virtual Windows USB/COM driver.
- Fabricating unobserved device behavior merely to pass initialization.
- Claiming TunerStudio compatibility before a real protocol exchange has been
captured and verified.
