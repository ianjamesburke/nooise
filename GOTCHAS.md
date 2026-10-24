# GOTCHAS

## Windows reports key releases without keyboard enhancement

`crossterm::terminal::sys::windows::supports_keyboard_enhancement` is hardcoded to `Ok(false)`, so nooise always negotiates reduced capabilities on Windows. The console event source reports `KeyEventKind::Release` anyway, straight off the console `key_down` flag (`crossterm/src/event/sys/windows/parse.rs`), independent of enhancement. A press-only fallback that flattens Release into Press therefore fires every binding twice per keystroke, on cmd, PowerShell and Windows Terminal alike. Reduced capabilities drop releases instead.

Holds stay unavailable there: `TerminalCapabilities::supports_holds` needs `key_event_types`, which crossterm will not report on Windows even though Windows Terminal 1.19+ speaks the Kitty protocol.
