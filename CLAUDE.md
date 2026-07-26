# CLAUDE.md

Instructions for Claude Code (and any other contributor, human or AI) working in this repository.

## Documentation

Every feature must be documented in `README.md`. When you add, change, or remove a user-facing feature, update the README's Features section (and any other affected section, such as Project layout) in the same change — do not leave it to a follow-up.

## Testing

Every feature must be implemented alongside a test that verifies it independently of the others. Before considering a feature done:

- Write a test that exercises that feature on its own, without depending on other features' state or behavior.
- Prefer testing pure logic (config load/save, window-mode selection, etc.) directly rather than through the GUI where possible.
- Run `cargo test` and confirm it passes before considering the work complete.
