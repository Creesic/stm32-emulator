# Launcher Workspace Persistence Design

## Goal

Make the native launcher resume the user workspace across runs, without
opening Windows console windows for either the launcher or its emulator child.
The restored workspace includes ImGui panel placement, native window placement,
loaded files, profile selection, and manual-profile values.

## Architecture

The launcher owns one workspace directory under LOCALAPPDATA:
stm32-emulator\launcher. ImGui writes its dock and panel layout to imgui.ini in
that directory. A sibling YAML workspace file stores application state and the
native window size and position.

On startup, the launcher loads both files before constructing the native
window. It restores a saved native size and position when valid, then gives
ImGui its stable INI path. While running, state changes are persisted and the
final workspace is saved when the window closes. The state never represents or
restarts a previously running emulator process.

## Saved State

- Native launcher window size and screen position.
- Firmware, SVD, and emulator executable paths.
- Selected catalog variant and filter text.
- Manual-profile enabled state, CPU model, SVD path, vector table, flash
  start/size, and RAM start/size.
- ImGui dock and panel geometry through the dedicated ImGui INI.

If the YAML state is absent, malformed, or references unavailable display
coordinates, launch with the normal default geometry while retaining a usable
empty workspace.

## Console Behavior

The launcher binary uses the Windows GUI subsystem. Its output-captured
stm32-emulator child uses the Windows CREATE_NO_WINDOW creation flag. Both
changes are Windows-specific compilation branches; Linux and macOS retain the
existing process behavior.

## CPU Model Compatibility

The emulator now requires an explicit CPU model. Launcher profiles therefore
carry Cortex-M4 or Cortex-M7, serialize it into their YAML CPU section, and
the manual form exposes an explicit model choice. Proteus F7 uses Cortex-M7.
No profile guesses an MCU model from a firmware filename.

## Validation

- Unit tests cover workspace YAML round-tripping and malformed-state fallback.
- Profile tests assert generated YAML includes the correct CPU model.
- Release builds cover the Windows GUI-subsystem and child-process code paths.
- A Windows smoke test verifies the launcher and a launched emulator child run
  without console windows, while emulator output remains visible in the ImGui
  output panel.

## Non-Goals

- No registry storage, cloud sync, or runtime EpicEFI source checkout.
- No emulator-process session restoration.
- No automatic remapping of firmware to another screen or profile.
