## Summary

<!-- What changed and why -->

## Test plan

- [ ] `make test` (required: full app-host suite incl. auth, pollers, Cursor)
- [ ] Manual check: menu bar / sign-in / preferences (if applicable)
- [ ] Windows parity: ported in `windows/` or tracked in windows-parity issue #__ (required when `TokenMon/`, `TokenMonTests/` or `Tests/` change; see CONTRIBUTING.md)

`make test-core` is an optional CLT-only smoke; it does not cover auth, pollers, or UI.

## Notes

<!-- Screenshots, breaking changes, docs updates -->
