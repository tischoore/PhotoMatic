# CLAUDE.md

Instructions for Claude Code (and any other contributor, human or AI) working in this repository.

## File format
Replace LF with CRLF

## Documentation

Every feature must be documented in `README.md`. When you add, change, or remove a user-facing feature, update the README's Features section (and any other affected section, such as Project layout) in the same change — do not leave it to a follow-up.

## Keyboard shortcuts

Every menu item, and every other actionable GUI control, must have a keyboard shortcut. Derive it from standard Windows conventions rather than inventing one:

- **Access-key mnemonic** — every menu and menu item gets an `&` in its text (e.g. `&File`, `&New`), unique among its siblings. This is free (a native Win32 text convention) and always required.
- **Accelerator** — if Windows convention already defines a standard key combination for the action (Ctrl+N for New, Ctrl+O for Open, Ctrl+S for Save, Ctrl+Shift+S for Save As, Alt+F4 for Exit, Ctrl+Z for Undo, etc.), wire it for real and show it as a right-aligned shortcut label (`\t`-suffixed text, e.g. `"&New\tCtrl+N"`). Don't add a label without the matching behavior, and don't invent a non-standard binding when no convention exists — a mnemonic alone is enough in that case.

Implementation note for this codebase (native-windows-gui): there is no accelerator-table API, so `\t`-suffixed shortcut text is cosmetic only — real Ctrl-key handling must be done by hand via the window's `OnKeyPress` event. Keep the key-combination-to-action mapping in a pure function (see `src/shortcuts.rs`) so it can be unit-tested independently of NWG, per the testing rule below, and have the event handler do nothing but read modifier state and call that function.

## UI alignment
When implementing UI ensure that the result always align and follows normal Windows application standards.

## Testing

Every feature must be implemented alongside a test that verifies it independently of the others. Before considering a feature done:

- Write a test that exercises that feature on its own, without depending on other features' state or behavior.
- Prefer testing pure logic (config load/save, window-mode selection, etc.) directly rather than through the GUI where possible.
- Run `cargo test` and confirm it passes before considering the work complete.
