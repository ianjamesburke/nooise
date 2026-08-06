//! Phase-aware terminal adapter and fair input scheduler.
//!
//! The runtime preserves transport facts without assigning musical meaning.
//! It is the production boundary for terminal lifecycle and Crossterm events.

use std::collections::VecDeque;
#[cfg(test)]
use std::fmt::Write as _;
use std::io;
use std::time::Duration;
use std::time::Instant;

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    MediaKeyCode, ModifierKeyCode, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::interaction::{
    ChordDrill, InputPhase, Intent, InteractionMode, Navigation, PageDirection, PerformanceAction,
    PerformanceInstrument, PerformanceKind, PerformanceMode, SemanticAction, SequenceStage,
};

pub(crate) const FRAME_INTERVAL: Duration = Duration::from_millis(33);
pub(crate) const MAX_FRAME_GAP: Duration = Duration::from_millis(50);
pub(crate) const TICK_INTERVAL: Duration = Duration::from_millis(33);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TerminalCapabilities {
    pub(crate) key_event_types: bool,
    pub(crate) plain_key_releases: bool,
}

impl TerminalCapabilities {
    pub(crate) fn supports_holds(self) -> bool {
        self.key_event_types && self.plain_key_releases
    }
}

trait TerminalControl {
    fn enable_raw(&mut self) -> io::Result<()>;
    fn enter_alternate_screen(&mut self) -> io::Result<()>;
    fn keyboard_enhancement_supported(&mut self) -> io::Result<bool>;
    fn push_keyboard_enhancement(&mut self) -> io::Result<()>;
    fn pop_keyboard_enhancement(&mut self) -> io::Result<()>;
    fn leave_alternate_screen(&mut self) -> io::Result<()>;
    fn disable_raw(&mut self) -> io::Result<()>;
}

#[derive(Default)]
struct CrosstermControl;

impl TerminalControl for CrosstermControl {
    fn enable_raw(&mut self) -> io::Result<()> {
        enable_raw_mode()
    }

    fn enter_alternate_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), EnterAlternateScreen)
    }

    fn keyboard_enhancement_supported(&mut self) -> io::Result<bool> {
        supports_keyboard_enhancement()
    }

    fn push_keyboard_enhancement(&mut self) -> io::Result<()> {
        execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
            )
        )
    }

    fn pop_keyboard_enhancement(&mut self) -> io::Result<()> {
        execute!(io::stdout(), PopKeyboardEnhancementFlags)
    }

    fn leave_alternate_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), LeaveAlternateScreen)
    }

    fn disable_raw(&mut self) -> io::Result<()> {
        disable_raw_mode()
    }
}

struct TerminalLifecycle<C: TerminalControl> {
    control: C,
    raw_enabled: bool,
    alternate_screen: bool,
    keyboard_flags_pushed: bool,
    capabilities: TerminalCapabilities,
}

impl<C: TerminalControl> TerminalLifecycle<C> {
    fn enter(control: C) -> io::Result<Self> {
        let mut lifecycle = Self {
            control,
            raw_enabled: false,
            alternate_screen: false,
            keyboard_flags_pushed: false,
            capabilities: TerminalCapabilities::default(),
        };

        lifecycle.control.enable_raw()?;
        lifecycle.raw_enabled = true;
        // Mark first so a partially written command is paired with a
        // best-effort leave during error cleanup.
        lifecycle.alternate_screen = true;
        lifecycle.control.enter_alternate_screen()?;

        if lifecycle
            .control
            .keyboard_enhancement_supported()
            .unwrap_or(false)
        {
            // Mark first so a partially written command is paired with a best-effort pop.
            lifecycle.keyboard_flags_pushed = true;
            if lifecycle.control.push_keyboard_enhancement().is_ok() {
                lifecycle.capabilities = TerminalCapabilities {
                    key_event_types: true,
                    plain_key_releases: true,
                };
            } else {
                if lifecycle.control.pop_keyboard_enhancement().is_ok() {
                    lifecycle.keyboard_flags_pushed = false;
                }
            }
        }

        Ok(lifecycle)
    }

    fn restore(&mut self) -> io::Result<()> {
        let mut first_error = None;
        if self.keyboard_flags_pushed {
            let result = self.control.pop_keyboard_enhancement();
            if result.is_ok() {
                self.keyboard_flags_pushed = false;
            }
            record_first_error(&mut first_error, result);
        }
        if self.alternate_screen {
            let result = self.control.leave_alternate_screen();
            if result.is_ok() {
                self.alternate_screen = false;
            }
            record_first_error(&mut first_error, result);
        }
        if self.raw_enabled {
            let result = self.control.disable_raw();
            if result.is_ok() {
                self.raw_enabled = false;
            }
            record_first_error(&mut first_error, result);
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl<C: TerminalControl> Drop for TerminalLifecycle<C> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn record_first_error(first: &mut Option<io::Error>, result: io::Result<()>) {
    if let Err(error) = result
        && first.is_none()
    {
        *first = Some(error);
    }
}

pub(crate) struct TerminalSession {
    // Drop the Ratatui terminal before restoring process-wide terminal modes.
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    lifecycle: TerminalLifecycle<CrosstermControl>,
}

impl TerminalSession {
    pub(crate) fn enter() -> io::Result<Self> {
        let lifecycle = TerminalLifecycle::enter(CrosstermControl)?;
        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        Ok(Self {
            terminal,
            lifecycle,
        })
    }

    pub(crate) fn capabilities(&self) -> TerminalCapabilities {
        self.lifecycle.capabilities
    }

    pub(crate) fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<io::Stdout>> {
        &mut self.terminal
    }

    pub(crate) fn restore(self) -> io::Result<()> {
        let Self {
            terminal,
            mut lifecycle,
        } = self;
        drop(terminal);
        lifecycle.restore()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Modifiers(u8);

impl Modifiers {
    pub(crate) const SHIFT: Self = Self(1 << 0);
    pub(crate) const CONTROL: Self = Self(1 << 1);
    pub(crate) const ALT: Self = Self(1 << 2);
    pub(crate) const SUPER: Self = Self(1 << 3);
    pub(crate) const HYPER: Self = Self(1 << 4);
    pub(crate) const META: Self = Self(1 << 5);

    pub(crate) fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[cfg(test)]
    pub(crate) fn from_bits(bits: u8) -> Self {
        Self(bits & 0b11_1111)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MediaKey {
    Play,
    Pause,
    PlayPause,
    Reverse,
    Stop,
    FastForward,
    Rewind,
    TrackNext,
    TrackPrevious,
    Record,
    LowerVolume,
    RaiseVolume,
    MuteVolume,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ModifierKey {
    LeftShift,
    LeftControl,
    LeftAlt,
    LeftSuper,
    LeftHyper,
    LeftMeta,
    RightShift,
    RightControl,
    RightAlt,
    RightSuper,
    RightHyper,
    RightMeta,
    IsoLevel3Shift,
    IsoLevel5Shift,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PhysicalKey {
    Backspace,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    Delete,
    Insert,
    Function(u8),
    Character(char),
    Null,
    Escape,
    CapsLock,
    ScrollLock,
    NumLock,
    PrintScreen,
    Pause,
    Menu,
    KeypadBegin,
    Media(MediaKey),
    Modifier(ModifierKey),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TransportKey {
    pub(crate) code: PhysicalKey,
    pub(crate) modifiers: Modifiers,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TransportEvent {
    Key {
        key: TransportKey,
        phase: InputPhase,
        /// Consecutive identical Repeat events are run-length encoded.
        repeat_count: u64,
    },
    Resize {
        width: u16,
        height: u16,
    },
    FocusGained,
    FocusLost,
    Paste(String),
    Mouse(String),
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "reserved for an external shutdown source")
    )]
    Shutdown,
}

/// In-memory sanitized recorder for capturing a live normalized input stream.
/// Call `record` at the scheduler seam and persist `finish()` only when the
/// developer explicitly chooses to retain a regression fixture.
#[cfg(test)]
pub(crate) struct SanitizedTraceRecorder {
    previous_at: Duration,
    fixture: String,
}

#[cfg(test)]
impl SanitizedTraceRecorder {
    pub(crate) fn new(started_at: Duration) -> Self {
        Self {
            previous_at: started_at,
            fixture: "nooise-replay-v1\n".into(),
        }
    }

    pub(crate) fn record(&mut self, now: Duration, event: &TransportEvent) {
        let after_ms = self.advance(now);
        match event {
            TransportEvent::Key {
                key,
                phase,
                repeat_count,
            } => {
                let token = encode_physical_key(&key.code);
                writeln!(
                    self.fixture,
                    "+{after_ms} key {token} {} mods:{} repeats:{repeat_count}",
                    recorder_phase_token(*phase),
                    key.modifiers.0
                )
                .expect("writing to String cannot fail");
            }
            TransportEvent::Resize { width, height } => {
                writeln!(self.fixture, "+{after_ms} resize {width}x{height}")
                    .expect("writing to String cannot fail");
            }
            TransportEvent::Paste(_) => {
                writeln!(self.fixture, "+{after_ms} redacted paste")
                    .expect("writing to String cannot fail");
            }
            TransportEvent::Mouse(_) => {
                writeln!(self.fixture, "+{after_ms} redacted mouse")
                    .expect("writing to String cannot fail");
            }
            TransportEvent::FocusGained => {
                writeln!(self.fixture, "+{after_ms} focus-gained")
                    .expect("writing to String cannot fail");
            }
            TransportEvent::FocusLost => {
                writeln!(self.fixture, "+{after_ms} focus-lost")
                    .expect("writing to String cannot fail");
            }
            TransportEvent::Shutdown => {
                writeln!(self.fixture, "+{after_ms} shutdown")
                    .expect("writing to String cannot fail");
            }
        }
    }

    pub(crate) fn record_tick(&mut self, now: Duration) {
        let after_ms = self.advance(now);
        writeln!(self.fixture, "+{after_ms} tick").expect("writing to String cannot fail");
    }

    pub(crate) fn record_idle(&mut self, now: Duration) {
        let after_ms = self.advance(now);
        writeln!(self.fixture, "+{after_ms} idle").expect("writing to String cannot fail");
    }

    pub(crate) fn finish(self) -> String {
        self.fixture
    }

    fn advance(&mut self, now: Duration) -> u64 {
        let after_ms = now
            .saturating_sub(self.previous_at)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        self.previous_at = now;
        after_ms
    }
}

#[cfg(test)]
fn recorder_phase_token(phase: InputPhase) -> &'static str {
    match phase {
        InputPhase::Press => "press",
        InputPhase::Repeat => "repeat",
        InputPhase::Release => "release",
    }
}

#[cfg(test)]
pub(crate) fn encode_physical_key(code: &PhysicalKey) -> String {
    match code {
        PhysicalKey::Character(character) => format!("char:{:06x}", u32::from(*character)),
        PhysicalKey::Escape => "escape".into(),
        PhysicalKey::Home => "home".into(),
        PhysicalKey::End => "end".into(),
        PhysicalKey::PageUp => "page-up".into(),
        PhysicalKey::PageDown => "page-down".into(),
        PhysicalKey::Enter => "enter".into(),
        PhysicalKey::Backspace => "backspace".into(),
        PhysicalKey::Left => "left".into(),
        PhysicalKey::Right => "right".into(),
        PhysicalKey::Up => "up".into(),
        PhysicalKey::Down => "down".into(),
        PhysicalKey::Tab => "tab".into(),
        PhysicalKey::BackTab => "backtab".into(),
        PhysicalKey::Delete => "delete".into(),
        PhysicalKey::Insert => "insert".into(),
        PhysicalKey::Function(number) => format!("f:{number}"),
        PhysicalKey::Null => "null".into(),
        PhysicalKey::CapsLock => "caps-lock".into(),
        PhysicalKey::ScrollLock => "scroll-lock".into(),
        PhysicalKey::NumLock => "num-lock".into(),
        PhysicalKey::PrintScreen => "print-screen".into(),
        PhysicalKey::Pause => "pause".into(),
        PhysicalKey::Menu => "menu".into(),
        PhysicalKey::KeypadBegin => "keypad-begin".into(),
        PhysicalKey::Media(key) => format!("media:{}", encode_media_key(*key)),
        PhysicalKey::Modifier(key) => format!("modifier:{}", encode_modifier_key(*key)),
    }
}

#[cfg(test)]
pub(crate) fn decode_physical_key(token: &str) -> Option<PhysicalKey> {
    Some(match token {
        "escape" => PhysicalKey::Escape,
        "home" => PhysicalKey::Home,
        "end" => PhysicalKey::End,
        "page-up" => PhysicalKey::PageUp,
        "page-down" => PhysicalKey::PageDown,
        "enter" => PhysicalKey::Enter,
        "backspace" => PhysicalKey::Backspace,
        "left" => PhysicalKey::Left,
        "right" => PhysicalKey::Right,
        "up" => PhysicalKey::Up,
        "down" => PhysicalKey::Down,
        "tab" => PhysicalKey::Tab,
        "backtab" => PhysicalKey::BackTab,
        "delete" => PhysicalKey::Delete,
        "insert" => PhysicalKey::Insert,
        "null" => PhysicalKey::Null,
        "caps-lock" => PhysicalKey::CapsLock,
        "scroll-lock" => PhysicalKey::ScrollLock,
        "num-lock" => PhysicalKey::NumLock,
        "print-screen" => PhysicalKey::PrintScreen,
        "pause" => PhysicalKey::Pause,
        "menu" => PhysicalKey::Menu,
        "keypad-begin" => PhysicalKey::KeypadBegin,
        _ if token.starts_with("char:") => {
            let scalar = u32::from_str_radix(token.strip_prefix("char:")?, 16).ok()?;
            PhysicalKey::Character(char::from_u32(scalar)?)
        }
        _ if token.starts_with("f:") => {
            PhysicalKey::Function(token.strip_prefix("f:")?.parse().ok()?)
        }
        _ if token.starts_with("media:") => {
            PhysicalKey::Media(decode_media_key(token.strip_prefix("media:")?)?)
        }
        _ if token.starts_with("modifier:") => {
            PhysicalKey::Modifier(decode_modifier_key(token.strip_prefix("modifier:")?)?)
        }
        _ => return None,
    })
}

#[cfg(test)]
fn encode_media_key(key: MediaKey) -> &'static str {
    match key {
        MediaKey::Play => "play",
        MediaKey::Pause => "pause",
        MediaKey::PlayPause => "play-pause",
        MediaKey::Reverse => "reverse",
        MediaKey::Stop => "stop",
        MediaKey::FastForward => "fast-forward",
        MediaKey::Rewind => "rewind",
        MediaKey::TrackNext => "track-next",
        MediaKey::TrackPrevious => "track-previous",
        MediaKey::Record => "record",
        MediaKey::LowerVolume => "lower-volume",
        MediaKey::RaiseVolume => "raise-volume",
        MediaKey::MuteVolume => "mute-volume",
    }
}

#[cfg(test)]
fn decode_media_key(token: &str) -> Option<MediaKey> {
    Some(match token {
        "play" => MediaKey::Play,
        "pause" => MediaKey::Pause,
        "play-pause" => MediaKey::PlayPause,
        "reverse" => MediaKey::Reverse,
        "stop" => MediaKey::Stop,
        "fast-forward" => MediaKey::FastForward,
        "rewind" => MediaKey::Rewind,
        "track-next" => MediaKey::TrackNext,
        "track-previous" => MediaKey::TrackPrevious,
        "record" => MediaKey::Record,
        "lower-volume" => MediaKey::LowerVolume,
        "raise-volume" => MediaKey::RaiseVolume,
        "mute-volume" => MediaKey::MuteVolume,
        _ => return None,
    })
}

#[cfg(test)]
fn encode_modifier_key(key: ModifierKey) -> &'static str {
    match key {
        ModifierKey::LeftShift => "left-shift",
        ModifierKey::LeftControl => "left-control",
        ModifierKey::LeftAlt => "left-alt",
        ModifierKey::LeftSuper => "left-super",
        ModifierKey::LeftHyper => "left-hyper",
        ModifierKey::LeftMeta => "left-meta",
        ModifierKey::RightShift => "right-shift",
        ModifierKey::RightControl => "right-control",
        ModifierKey::RightAlt => "right-alt",
        ModifierKey::RightSuper => "right-super",
        ModifierKey::RightHyper => "right-hyper",
        ModifierKey::RightMeta => "right-meta",
        ModifierKey::IsoLevel3Shift => "iso-level3-shift",
        ModifierKey::IsoLevel5Shift => "iso-level5-shift",
    }
}

#[cfg(test)]
fn decode_modifier_key(token: &str) -> Option<ModifierKey> {
    Some(match token {
        "left-shift" => ModifierKey::LeftShift,
        "left-control" => ModifierKey::LeftControl,
        "left-alt" => ModifierKey::LeftAlt,
        "left-super" => ModifierKey::LeftSuper,
        "left-hyper" => ModifierKey::LeftHyper,
        "left-meta" => ModifierKey::LeftMeta,
        "right-shift" => ModifierKey::RightShift,
        "right-control" => ModifierKey::RightControl,
        "right-alt" => ModifierKey::RightAlt,
        "right-super" => ModifierKey::RightSuper,
        "right-hyper" => ModifierKey::RightHyper,
        "right-meta" => ModifierKey::RightMeta,
        "iso-level3-shift" => ModifierKey::IsoLevel3Shift,
        "iso-level5-shift" => ModifierKey::IsoLevel5Shift,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeferredInput {
    RuntimeContext(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputMapping {
    Action(SemanticAction),
    Ignored,
    Deferred(DeferredInput),
}

/// Canonical transport-to-domain input boundary. Decided bindings map to
/// actions; staged behavior names its missing owner instead of disappearing.
pub(crate) fn map_input(
    mode: &InteractionMode,
    navigation: Navigation,
    event: &TransportEvent,
    capabilities: TerminalCapabilities,
) -> InputMapping {
    let TransportEvent::Key {
        key,
        phase,
        repeat_count: _,
    } = event
    else {
        return InputMapping::Ignored;
    };
    let has_control = key.modifiers.contains(Modifiers::CONTROL);
    if has_control
        && matches!(
            mode,
            InteractionMode::Browsing | InteractionMode::Automation(_)
        )
        && matches!(key.code, PhysicalKey::Character('s' | 'S'))
    {
        return InputMapping::Action(SemanticAction {
            phase: *phase,
            intent: Intent::Save,
        });
    }
    if has_control
        && matches!(
            mode,
            InteractionMode::Browsing | InteractionMode::Automation(_)
        )
        && matches!(key.code, PhysicalKey::Character('c' | 'C'))
    {
        return InputMapping::Action(SemanticAction {
            phase: *phase,
            intent: Intent::Quit,
        });
    }
    let intent = if matches!(key.code, PhysicalKey::Escape) {
        Intent::Cancel
    } else if matches!(mode, InteractionMode::Browsing)
        && matches!(key.code, PhysicalKey::BackTab)
        && (key.modifiers == Modifiers::default() || key.modifiers == Modifiers::SHIFT)
    {
        Intent::ChangePage(PageDirection::Previous)
    } else {
        match mode {
            InteractionMode::Browsing if key.modifiers == Modifiers::default() => match key.code {
                PhysicalKey::Up | PhysicalKey::Character('k') => Intent::MoveSelection(-1),
                PhysicalKey::Down | PhysicalKey::Character('j') => Intent::MoveSelection(1),
                PhysicalKey::Left | PhysicalKey::Character('h') => Intent::AdjustSelected(-1),
                PhysicalKey::Right | PhysicalKey::Character('l') => Intent::AdjustSelected(1),
                PhysicalKey::Tab => Intent::ChangePage(PageDirection::Next),
                PhysicalKey::Character('/') => Intent::OpenPalette,
                PhysicalKey::Character('f') => {
                    Intent::OpenAutomation(super::interaction::AutomationKind::Lfo)
                }
                PhysicalKey::Character('e') => {
                    Intent::OpenAutomation(super::interaction::AutomationKind::Envelope)
                }
                PhysicalKey::Character('v') => Intent::ToggleMacro,
                PhysicalKey::Character('p') => Intent::ActivatePerformance(PerformanceKind::Deck),
                PhysicalKey::Character(' ') => {
                    Intent::ActivatePerformance(PerformanceKind::Sequence)
                }
                PhysicalKey::Character(character)
                    if character.is_ascii_digit() || character == '.' || character == '-' =>
                {
                    Intent::BeginNumeric(character)
                }
                PhysicalKey::Character('q') => Intent::Quit,
                PhysicalKey::Enter => match navigation {
                    Navigation::Chords {
                        selected,
                        drill: ChordDrill::Progression { .. },
                    } => Intent::EnterChordSlot(selected),
                    Navigation::Chords {
                        drill: ChordDrill::None,
                        ..
                    } => Intent::TouchSelected,
                    _ => Intent::TouchSelected,
                },
                PhysicalKey::Character('a') => Intent::ToggleAuto,
                PhysicalKey::Character('m') => Intent::ToggleMute { master: false },
                PhysicalKey::Character('t') => Intent::ToggleUnits,
                PhysicalKey::Character('x') => Intent::RemoveAutomation,
                PhysicalKey::Character('r') => Intent::ReseedAutomation,
                _ => return InputMapping::Ignored,
            },
            InteractionMode::Numeric(_) => match key.code {
                PhysicalKey::Enter => Intent::Confirm,
                PhysicalKey::Backspace => Intent::Backspace,
                PhysicalKey::Character(character)
                    if character.is_ascii_digit() || character == '.' || character == '-' =>
                {
                    Intent::TypeCharacter(character)
                }
                _ => return InputMapping::Ignored,
            },
            InteractionMode::Palette(_)
                if key.modifiers == Modifiers::default() || key.modifiers == Modifiers::SHIFT =>
            {
                match key.code {
                    PhysicalKey::Enter => Intent::Confirm,
                    PhysicalKey::Backspace => Intent::Backspace,
                    PhysicalKey::Tab => Intent::PaletteAutocomplete,
                    PhysicalKey::Up => Intent::MoveSelection(-1),
                    PhysicalKey::Down => Intent::MoveSelection(1),
                    PhysicalKey::Character(character) => Intent::TypeCharacter(character),
                    _ => return InputMapping::Ignored,
                }
            }
            InteractionMode::Palette(_) if has_control => match key.code {
                PhysicalKey::Character('p' | 'P') => Intent::MoveSelection(-1),
                PhysicalKey::Character('n' | 'N') => Intent::MoveSelection(1),
                PhysicalKey::Character('b' | 'B') => Intent::CommitPaletteAtBar,
                _ => return InputMapping::Ignored,
            },
            InteractionMode::Automation(automation) if key.modifiers == Modifiers::default() => {
                match key.code {
                    PhysicalKey::Up | PhysicalKey::Character('k') => Intent::MoveSelection(-1),
                    PhysicalKey::Down | PhysicalKey::Character('j') => Intent::MoveSelection(1),
                    PhysicalKey::Left | PhysicalKey::Character('h') => Intent::AdjustSelected(-1),
                    PhysicalKey::Right | PhysicalKey::Character('l') => Intent::AdjustSelected(1),
                    PhysicalKey::Enter => return InputMapping::Ignored,
                    PhysicalKey::Character('v') => Intent::ToggleMacro,
                    PhysicalKey::Character('x') => Intent::RemoveAutomation,
                    PhysicalKey::Character('r') => Intent::ReseedAutomation,
                    PhysicalKey::Character('t') => Intent::ToggleUnits,
                    PhysicalKey::Tab => Intent::ChangePage(PageDirection::Next),
                    PhysicalKey::Character('/') => Intent::OpenPalette,
                    PhysicalKey::Character('q') => Intent::Quit,
                    PhysicalKey::Character('a') => Intent::ToggleAuto,
                    PhysicalKey::Character('f') => {
                        Intent::OpenAutomation(super::interaction::AutomationKind::Lfo)
                    }
                    PhysicalKey::Character('e') => {
                        Intent::OpenAutomation(super::interaction::AutomationKind::Envelope)
                    }
                    PhysicalKey::Character('m') => Intent::ToggleMute { master: false },
                    PhysicalKey::Character(character)
                        if character.is_ascii_digit() || character == '.' || character == '-' =>
                    {
                        Intent::BeginNumeric(character)
                    }
                    _ => return InputMapping::Ignored,
                }
            }
            InteractionMode::Browsing | InteractionMode::Automation(_)
                if key.modifiers == Modifiers::SHIFT =>
            {
                match key.code {
                    PhysicalKey::Left | PhysicalKey::Character('H' | 'h') => Intent::ResetSelected,
                    PhysicalKey::Character('M' | 'm') => Intent::ToggleMute { master: true },
                    PhysicalKey::Character('T' | 't') => Intent::ToggleUnits,
                    PhysicalKey::Character('X' | 'x') => Intent::RemoveAutomation,
                    PhysicalKey::Character('R' | 'r') => Intent::ReseedAutomation,
                    PhysicalKey::BackTab => Intent::ChangePage(PageDirection::Previous),
                    _ => return InputMapping::Ignored,
                }
            }
            InteractionMode::Performance(performance) if key.modifiers == Modifiers::default() => {
                if matches!(key.code, PhysicalKey::Character('p')) {
                    Intent::ActivatePerformance(PerformanceKind::Deck)
                } else if matches!(key.code, PhysicalKey::Character(' ')) {
                    Intent::ActivatePerformance(PerformanceKind::Sequence)
                } else if let Some(instrument) = performance_instrument(&key.code) {
                    match phase {
                        InputPhase::Release => Intent::ReleaseHeldSelector(instrument),
                        InputPhase::Press
                            if matches!(
                                performance,
                                PerformanceMode::Deck { .. }
                                    | PerformanceMode::Sequence {
                                        stage: SequenceStage::ChooseInstrument
                                            | SequenceStage::Perform { .. },
                                        ..
                                    }
                            ) =>
                        {
                            Intent::SelectPerformanceInstrument {
                                instrument,
                                hold: capabilities.supports_holds(),
                            }
                        }
                        InputPhase::Repeat => return InputMapping::Ignored,
                        _ => return InputMapping::Ignored,
                    }
                } else if let Some(action) = performance_action(&key.code) {
                    match (performance, phase) {
                        (
                            PerformanceMode::Sequence {
                                stage: SequenceStage::AwaitActionRelease { action: armed, .. },
                                ..
                            },
                            InputPhase::Release,
                        ) if action == *armed => Intent::FinishPerformanceSequence(action),
                        (PerformanceMode::Deck { .. }, InputPhase::Press | InputPhase::Repeat)
                        | (
                            PerformanceMode::Sequence {
                                stage: SequenceStage::Perform { .. },
                                ..
                            },
                            InputPhase::Press,
                        ) => Intent::ApplyPerformanceAction {
                            action,
                            release_available: capabilities.supports_holds(),
                        },
                        _ => return InputMapping::Ignored,
                    }
                } else {
                    return InputMapping::Ignored;
                }
            }
            _ if key.modifiers != Modifiers::default() => {
                return deferred_runtime("modified binding is not in the current kernel");
            }
            _ => return InputMapping::Ignored,
        }
    };
    InputMapping::Action(SemanticAction {
        phase: *phase,
        intent,
    })
}

fn performance_instrument(key: &PhysicalKey) -> Option<PerformanceInstrument> {
    match key {
        PhysicalKey::Character('a') => Some(PerformanceInstrument::Pads),
        PhysicalKey::Character('s') => Some(PerformanceInstrument::Bass),
        PhysicalKey::Character('d') => Some(PerformanceInstrument::Kick),
        PhysicalKey::Character('f') => Some(PerformanceInstrument::Perc),
        _ => None,
    }
}

fn performance_action(key: &PhysicalKey) -> Option<PerformanceAction> {
    match key {
        PhysicalKey::Character('h') => Some(PerformanceAction::Shorter),
        PhysicalKey::Character('l') => Some(PerformanceAction::Longer),
        PhysicalKey::Character('j') => Some(PerformanceAction::Quieter),
        PhysicalKey::Character('k') => Some(PerformanceAction::Louder),
        PhysicalKey::Character('u') => Some(PerformanceAction::Sparser),
        PhysicalKey::Character('i') => Some(PerformanceAction::Denser),
        _ => None,
    }
}

fn deferred_runtime(reason: &'static str) -> InputMapping {
    InputMapping::Deferred(DeferredInput::RuntimeContext(reason))
}

impl TransportEvent {
    fn coalesce_repeat(&mut self, incoming: &Self) -> bool {
        match (self, incoming) {
            (
                Self::Key {
                    key: current,
                    phase: InputPhase::Repeat,
                    repeat_count,
                },
                Self::Key {
                    key: next,
                    phase: InputPhase::Repeat,
                    repeat_count: incoming_count,
                },
            ) if current == next => {
                *repeat_count = repeat_count.saturating_add(*incoming_count);
                true
            }
            _ => false,
        }
    }
}

fn normalize_event(event: Event, capabilities: TerminalCapabilities) -> TransportEvent {
    match event {
        Event::Key(key) => normalize_key_event(key, capabilities),
        Event::Resize(width, height) => TransportEvent::Resize { width, height },
        Event::FocusGained => TransportEvent::FocusGained,
        Event::FocusLost => TransportEvent::FocusLost,
        Event::Paste(text) => TransportEvent::Paste(text),
        Event::Mouse(mouse) => TransportEvent::Mouse(format!("{mouse:?}")),
    }
}

pub(crate) fn normalize_key_event(
    event: KeyEvent,
    capabilities: TerminalCapabilities,
) -> TransportEvent {
    let phase = if capabilities.key_event_types {
        match event.kind {
            KeyEventKind::Press => InputPhase::Press,
            KeyEventKind::Repeat => InputPhase::Repeat,
            KeyEventKind::Release if capabilities.plain_key_releases => InputPhase::Release,
            KeyEventKind::Release => InputPhase::Press,
        }
    } else {
        // Legacy terminals report no trustworthy phase distinction. Preserve
        // ordinary commands while ensuring ambiguous input cannot become a hold.
        InputPhase::Press
    };
    TransportEvent::Key {
        key: TransportKey {
            code: normalize_key_code(event.code),
            modifiers: normalize_modifiers(event.modifiers),
        },
        phase,
        repeat_count: 1,
    }
}

fn normalize_key_code(code: KeyCode) -> PhysicalKey {
    match code {
        KeyCode::Backspace => PhysicalKey::Backspace,
        KeyCode::Enter => PhysicalKey::Enter,
        KeyCode::Left => PhysicalKey::Left,
        KeyCode::Right => PhysicalKey::Right,
        KeyCode::Up => PhysicalKey::Up,
        KeyCode::Down => PhysicalKey::Down,
        KeyCode::Home => PhysicalKey::Home,
        KeyCode::End => PhysicalKey::End,
        KeyCode::PageUp => PhysicalKey::PageUp,
        KeyCode::PageDown => PhysicalKey::PageDown,
        KeyCode::Tab => PhysicalKey::Tab,
        KeyCode::BackTab => PhysicalKey::BackTab,
        KeyCode::Delete => PhysicalKey::Delete,
        KeyCode::Insert => PhysicalKey::Insert,
        KeyCode::F(number) => PhysicalKey::Function(number),
        KeyCode::Char(character) => PhysicalKey::Character(character),
        KeyCode::Null => PhysicalKey::Null,
        KeyCode::Esc => PhysicalKey::Escape,
        KeyCode::CapsLock => PhysicalKey::CapsLock,
        KeyCode::ScrollLock => PhysicalKey::ScrollLock,
        KeyCode::NumLock => PhysicalKey::NumLock,
        KeyCode::PrintScreen => PhysicalKey::PrintScreen,
        KeyCode::Pause => PhysicalKey::Pause,
        KeyCode::Menu => PhysicalKey::Menu,
        KeyCode::KeypadBegin => PhysicalKey::KeypadBegin,
        KeyCode::Media(media) => PhysicalKey::Media(match media {
            MediaKeyCode::Play => MediaKey::Play,
            MediaKeyCode::Pause => MediaKey::Pause,
            MediaKeyCode::PlayPause => MediaKey::PlayPause,
            MediaKeyCode::Reverse => MediaKey::Reverse,
            MediaKeyCode::Stop => MediaKey::Stop,
            MediaKeyCode::FastForward => MediaKey::FastForward,
            MediaKeyCode::Rewind => MediaKey::Rewind,
            MediaKeyCode::TrackNext => MediaKey::TrackNext,
            MediaKeyCode::TrackPrevious => MediaKey::TrackPrevious,
            MediaKeyCode::Record => MediaKey::Record,
            MediaKeyCode::LowerVolume => MediaKey::LowerVolume,
            MediaKeyCode::RaiseVolume => MediaKey::RaiseVolume,
            MediaKeyCode::MuteVolume => MediaKey::MuteVolume,
        }),
        KeyCode::Modifier(modifier) => PhysicalKey::Modifier(match modifier {
            ModifierKeyCode::LeftShift => ModifierKey::LeftShift,
            ModifierKeyCode::LeftControl => ModifierKey::LeftControl,
            ModifierKeyCode::LeftAlt => ModifierKey::LeftAlt,
            ModifierKeyCode::LeftSuper => ModifierKey::LeftSuper,
            ModifierKeyCode::LeftHyper => ModifierKey::LeftHyper,
            ModifierKeyCode::LeftMeta => ModifierKey::LeftMeta,
            ModifierKeyCode::RightShift => ModifierKey::RightShift,
            ModifierKeyCode::RightControl => ModifierKey::RightControl,
            ModifierKeyCode::RightAlt => ModifierKey::RightAlt,
            ModifierKeyCode::RightSuper => ModifierKey::RightSuper,
            ModifierKeyCode::RightHyper => ModifierKey::RightHyper,
            ModifierKeyCode::RightMeta => ModifierKey::RightMeta,
            ModifierKeyCode::IsoLevel3Shift => ModifierKey::IsoLevel3Shift,
            ModifierKeyCode::IsoLevel5Shift => ModifierKey::IsoLevel5Shift,
        }),
    }
}

fn normalize_modifiers(modifiers: KeyModifiers) -> Modifiers {
    let mut normalized = Modifiers::default();
    for (source, target) in [
        (KeyModifiers::SHIFT, Modifiers::SHIFT),
        (KeyModifiers::CONTROL, Modifiers::CONTROL),
        (KeyModifiers::ALT, Modifiers::ALT),
        (KeyModifiers::SUPER, Modifiers::SUPER),
        (KeyModifiers::HYPER, Modifiers::HYPER),
        (KeyModifiers::META, Modifiers::META),
    ] {
        if modifiers.contains(source) {
            normalized.0 |= target.0;
        }
    }
    normalized
}

pub(crate) trait EventSource {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool>;
    fn read(&mut self) -> io::Result<TransportEvent>;
}

pub(crate) struct CrosstermEventSource {
    capabilities: TerminalCapabilities,
}

impl CrosstermEventSource {
    pub(crate) fn new(capabilities: TerminalCapabilities) -> Self {
        Self { capabilities }
    }
}

impl EventSource for CrosstermEventSource {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        event::poll(timeout)
    }

    fn read(&mut self) -> io::Result<TransportEvent> {
        event::read().map(|event| normalize_event(event, self.capabilities))
    }
}

pub(crate) trait Clock {
    fn now(&self) -> Duration;
}

pub(crate) struct MonotonicClock {
    started: Instant,
}

impl MonotonicClock {
    pub(crate) fn start() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.started.elapsed()
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SchedulerConfig {
    pub(crate) queue_capacity: usize,
    pub(crate) max_reads_per_turn: usize,
    pub(crate) input_time_budget: Duration,
    pub(crate) tick_interval: Duration,
    pub(crate) frame_interval: Duration,
    pub(crate) max_frame_gap: Duration,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 256,
            max_reads_per_turn: 64,
            input_time_budget: Duration::from_millis(4),
            tick_interval: TICK_INTERVAL,
            frame_interval: FRAME_INTERVAL,
            max_frame_gap: MAX_FRAME_GAP,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RuntimeTurn {
    pub(crate) events: Vec<TransportEvent>,
    pub(crate) tick_due: bool,
    pub(crate) render_due: bool,
    pub(crate) shutdown_seen: bool,
}

pub(crate) struct Scheduler {
    config: SchedulerConfig,
    queue: VecDeque<TransportEvent>,
    last_tick_at: Duration,
    last_frame_at: Duration,
    frame_requested: bool,
    shutdown_seen: bool,
}

impl Scheduler {
    pub(crate) fn new(config: SchedulerConfig, started_at: Duration) -> Self {
        assert!(config.queue_capacity > 0, "queue capacity must be nonzero");
        assert!(config.max_reads_per_turn > 0, "read budget must be nonzero");
        assert!(
            config.frame_interval <= config.max_frame_gap,
            "normal frame interval must fit inside the hard frame gap"
        );
        Self {
            config,
            queue: VecDeque::with_capacity(config.queue_capacity),
            last_tick_at: started_at,
            last_frame_at: started_at,
            frame_requested: true,
            shutdown_seen: false,
        }
    }

    pub(crate) fn collect_turn<S: EventSource, C: Clock>(
        &mut self,
        source: &mut S,
        clock: &C,
    ) -> io::Result<RuntimeTurn> {
        let started = clock.now();
        if self.frame_requested || self.hard_frame_due(started) {
            return Ok(self.turn(Vec::new(), self.tick_due(started), true));
        }

        let mut reads = 0;
        while reads < self.config.max_reads_per_turn
            && clock.now().saturating_sub(started) < self.config.input_time_budget
            && self.queue.len() < self.config.queue_capacity
            && !self.shutdown_seen
        {
            let timeout = if reads == 0 && self.queue.is_empty() {
                self.time_until_next_wakeup(clock.now())
            } else {
                Duration::ZERO
            };
            if !source.poll(timeout)? {
                break;
            }

            let event = source.read()?;
            reads += 1;
            if matches!(event, TransportEvent::Shutdown) {
                self.shutdown_seen = true;
            }
            self.admit(event);

            if self.hard_frame_due(clock.now()) {
                break;
            }
        }

        let now = clock.now();
        if self.hard_frame_due(now) {
            return Ok(self.turn(Vec::new(), self.tick_due(now), true));
        }

        let events = self.queue.drain(..).collect();
        let tick_due = self.tick_due(now);
        let render_due = self.frame_requested
            || now.saturating_sub(self.last_frame_at) >= self.config.frame_interval;
        Ok(self.turn(events, tick_due, render_due))
    }

    fn admit(&mut self, event: TransportEvent) {
        if self
            .queue
            .back_mut()
            .is_some_and(|queued| queued.coalesce_repeat(&event))
        {
            return;
        }
        // collect_turn checks capacity before reading, so reliable events are
        // backpressured at the source rather than dropped here.
        debug_assert!(self.queue.len() < self.config.queue_capacity);
        self.queue.push_back(event);
    }

    fn turn(&self, events: Vec<TransportEvent>, tick_due: bool, render_due: bool) -> RuntimeTurn {
        let shutdown_seen = events
            .iter()
            .any(|event| matches!(event, TransportEvent::Shutdown));
        RuntimeTurn {
            events,
            tick_due,
            render_due,
            shutdown_seen,
        }
    }

    pub(crate) fn request_frame(&mut self) {
        self.frame_requested = true;
    }

    pub(crate) fn tick_due(&self, now: Duration) -> bool {
        now.saturating_sub(self.last_tick_at) >= self.config.tick_interval
    }

    pub(crate) fn complete_tick(&mut self, now: Duration) {
        self.last_tick_at = now;
    }

    pub(crate) fn render_due(&self, now: Duration) -> bool {
        self.frame_requested || now.saturating_sub(self.last_frame_at) >= self.config.frame_interval
    }

    pub(crate) fn complete_frame(&mut self, now: Duration) {
        self.last_frame_at = now;
        self.frame_requested = false;
    }

    #[cfg(test)]
    pub(crate) fn queue_len(&self) -> usize {
        self.queue.len()
    }

    fn hard_frame_due(&self, now: Duration) -> bool {
        now.saturating_sub(self.last_frame_at) >= self.config.max_frame_gap
    }

    fn time_until_next_wakeup(&self, now: Duration) -> Duration {
        let until_tick = self
            .config
            .tick_interval
            .saturating_sub(now.saturating_sub(self.last_tick_at));
        let until_frame = self
            .config
            .frame_interval
            .saturating_sub(now.saturating_sub(self.last_frame_at));
        until_tick.min(until_frame)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;

    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ControlCall {
        EnableRaw,
        EnterAlternate,
        QueryCapabilities,
        PushKeyboard,
        PopKeyboard,
        LeaveAlternate,
        DisableRaw,
    }

    struct FakeControl {
        calls: Rc<RefCell<Vec<ControlCall>>>,
        keyboard_supported: io::Result<bool>,
        fail_on: Option<ControlCall>,
    }

    impl FakeControl {
        fn call(&mut self, call: ControlCall) -> io::Result<()> {
            self.calls.borrow_mut().push(call);
            if self.fail_on == Some(call) {
                Err(io::Error::other(format!("{call:?} failed")))
            } else {
                Ok(())
            }
        }
    }

    impl TerminalControl for FakeControl {
        fn enable_raw(&mut self) -> io::Result<()> {
            self.call(ControlCall::EnableRaw)
        }

        fn enter_alternate_screen(&mut self) -> io::Result<()> {
            self.call(ControlCall::EnterAlternate)
        }

        fn keyboard_enhancement_supported(&mut self) -> io::Result<bool> {
            self.calls.borrow_mut().push(ControlCall::QueryCapabilities);
            self.keyboard_supported
                .as_ref()
                .map(|supported| *supported)
                .map_err(|error| io::Error::new(error.kind(), error.to_string()))
        }

        fn push_keyboard_enhancement(&mut self) -> io::Result<()> {
            self.call(ControlCall::PushKeyboard)
        }

        fn pop_keyboard_enhancement(&mut self) -> io::Result<()> {
            self.call(ControlCall::PopKeyboard)
        }

        fn leave_alternate_screen(&mut self) -> io::Result<()> {
            self.call(ControlCall::LeaveAlternate)
        }

        fn disable_raw(&mut self) -> io::Result<()> {
            self.call(ControlCall::DisableRaw)
        }
    }

    fn fake_control(
        calls: Rc<RefCell<Vec<ControlCall>>>,
        supported: io::Result<bool>,
        fail_on: Option<ControlCall>,
    ) -> FakeControl {
        FakeControl {
            calls,
            keyboard_supported: supported,
            fail_on,
        }
    }

    #[test]
    fn terminal_lifecycle_negotiates_and_restores_in_reverse_order() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let lifecycle = TerminalLifecycle::enter(fake_control(Rc::clone(&calls), Ok(true), None))
            .expect("enter");
        assert!(lifecycle.capabilities.supports_holds());
        drop(lifecycle);

        assert_eq!(
            *calls.borrow(),
            [
                ControlCall::EnableRaw,
                ControlCall::EnterAlternate,
                ControlCall::QueryCapabilities,
                ControlCall::PushKeyboard,
                ControlCall::PopKeyboard,
                ControlCall::LeaveAlternate,
                ControlCall::DisableRaw,
            ]
        );
    }

    #[test]
    fn terminal_lifecycle_exposes_reduced_capabilities_when_query_fails() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let lifecycle = TerminalLifecycle::enter(fake_control(
            Rc::clone(&calls),
            Err(io::Error::other("unsupported")),
            None,
        ))
        .expect("fallback remains usable");
        assert_eq!(lifecycle.capabilities, TerminalCapabilities::default());
        drop(lifecycle);
        assert!(!calls.borrow().contains(&ControlCall::PushKeyboard));
        assert!(!calls.borrow().contains(&ControlCall::PopKeyboard));
    }

    #[test]
    fn terminal_lifecycle_falls_back_and_pops_after_failed_keyboard_push() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let lifecycle = TerminalLifecycle::enter(fake_control(
            Rc::clone(&calls),
            Ok(true),
            Some(ControlCall::PushKeyboard),
        ))
        .expect("failed enhancement remains a usable reduced terminal");
        assert_eq!(lifecycle.capabilities, TerminalCapabilities::default());
        drop(lifecycle);
        assert_eq!(
            calls
                .borrow()
                .iter()
                .filter(|call| **call == ControlCall::PopKeyboard)
                .count(),
            1
        );
    }

    #[test]
    fn explicit_restore_error_is_retried_during_drop() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut lifecycle = TerminalLifecycle::enter(fake_control(
            Rc::clone(&calls),
            Ok(true),
            Some(ControlCall::PopKeyboard),
        ))
        .expect("enter");
        assert!(lifecycle.restore().is_err());
        drop(lifecycle);
        assert_eq!(
            calls
                .borrow()
                .iter()
                .filter(|call| **call == ControlCall::PopKeyboard)
                .count(),
            2
        );
    }

    #[test]
    fn terminal_lifecycle_restores_partial_setup_after_error() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let result = TerminalLifecycle::enter(fake_control(
            Rc::clone(&calls),
            Ok(true),
            Some(ControlCall::EnterAlternate),
        ));
        assert!(result.is_err());
        assert_eq!(
            *calls.borrow(),
            [
                ControlCall::EnableRaw,
                ControlCall::EnterAlternate,
                ControlCall::LeaveAlternate,
                ControlCall::DisableRaw,
            ]
        );
    }

    #[test]
    fn terminal_lifecycle_restores_during_panic_unwind() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let result = catch_unwind(AssertUnwindSafe({
            let calls = Rc::clone(&calls);
            move || {
                let _lifecycle =
                    TerminalLifecycle::enter(fake_control(calls, Ok(true), None)).expect("enter");
                panic!("test unwind");
            }
        }));
        assert!(result.is_err());
        assert_eq!(
            &calls.borrow()[4..],
            [
                ControlCall::PopKeyboard,
                ControlCall::LeaveAlternate,
                ControlCall::DisableRaw,
            ]
        );
    }

    #[test]
    fn key_normalization_preserves_phase_key_and_modifiers() {
        let event = KeyEvent::new_with_kind(
            KeyCode::Char('p'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            KeyEventKind::Release,
        );
        assert_eq!(
            normalize_key_event(
                event,
                TerminalCapabilities {
                    key_event_types: true,
                    plain_key_releases: true,
                },
            ),
            TransportEvent::Key {
                key: TransportKey {
                    code: PhysicalKey::Character('p'),
                    modifiers: Modifiers(Modifiers::CONTROL.0 | Modifiers::SHIFT.0),
                },
                phase: InputPhase::Release,
                repeat_count: 1,
            }
        );
    }

    #[test]
    fn reduced_capabilities_never_normalize_ambiguous_phases_as_holds() {
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            let event = KeyEvent::new_with_kind(KeyCode::Char('p'), KeyModifiers::NONE, kind);
            assert!(matches!(
                normalize_key_event(event, TerminalCapabilities::default()),
                TransportEvent::Key {
                    phase: InputPhase::Press,
                    ..
                }
            ));
        }
    }

    #[test]
    fn canonical_mapper_preserves_performance_entry_phase_for_kernel_policy() {
        let event = TransportEvent::Key {
            key: TransportKey {
                code: PhysicalKey::Character('p'),
                modifiers: Modifiers::default(),
            },
            phase: InputPhase::Repeat,
            repeat_count: 1,
        };
        assert_eq!(
            map_input(
                &InteractionMode::Browsing,
                Navigation::default(),
                &event,
                TerminalCapabilities {
                    key_event_types: true,
                    plain_key_releases: true,
                }
            ),
            InputMapping::Action(SemanticAction {
                phase: InputPhase::Repeat,
                intent: Intent::ActivatePerformance(PerformanceKind::Deck),
            })
        );
    }

    #[test]
    fn canonical_mapper_maps_selector_press_repeat_and_matching_release() {
        let model = InteractionMode::Performance(PerformanceMode::Sequence {
            stage: SequenceStage::Perform {
                instrument: PerformanceInstrument::Pads,
            },
            held_selector: None,
        });
        let expected = [
            InputMapping::Action(SemanticAction {
                phase: InputPhase::Press,
                intent: Intent::SelectPerformanceInstrument {
                    instrument: PerformanceInstrument::Pads,
                    hold: true,
                },
            }),
            InputMapping::Ignored,
            InputMapping::Action(SemanticAction {
                phase: InputPhase::Release,
                intent: Intent::ReleaseHeldSelector(PerformanceInstrument::Pads),
            }),
        ];
        for (phase, expected) in [InputPhase::Press, InputPhase::Repeat, InputPhase::Release]
            .into_iter()
            .zip(expected)
        {
            let event = TransportEvent::Key {
                key: TransportKey {
                    code: PhysicalKey::Character('a'),
                    modifiers: Modifiers::default(),
                },
                phase,
                repeat_count: 1,
            };
            assert_eq!(
                map_input(
                    &model,
                    Navigation::default(),
                    &event,
                    TerminalCapabilities {
                        key_event_types: true,
                        plain_key_releases: true,
                    }
                ),
                expected
            );
        }
    }

    #[test]
    fn mapper_preserves_modal_ownership_and_names_context_gaps() {
        let ctrl_s = TransportEvent::Key {
            key: TransportKey {
                code: PhysicalKey::Character('s'),
                modifiers: Modifiers::CONTROL,
            },
            phase: InputPhase::Press,
            repeat_count: 1,
        };
        assert!(matches!(
            map_input(
                &InteractionMode::Browsing,
                Navigation::default(),
                &ctrl_s,
                TerminalCapabilities::default()
            ),
            InputMapping::Action(SemanticAction {
                intent: Intent::Save,
                ..
            })
        ));
        assert!(matches!(
            map_input(
                &InteractionMode::Numeric(Default::default()),
                Navigation::default(),
                &ctrl_s,
                TerminalCapabilities::default()
            ),
            InputMapping::Ignored
        ));

        let opener = TransportEvent::Key {
            key: TransportKey {
                code: PhysicalKey::Character('f'),
                modifiers: Modifiers::default(),
            },
            phase: InputPhase::Press,
            repeat_count: 1,
        };
        assert!(matches!(
            map_input(
                &InteractionMode::Browsing,
                Navigation::default(),
                &opener,
                TerminalCapabilities::default()
            ),
            InputMapping::Action(SemanticAction {
                intent: Intent::OpenAutomation(super::super::interaction::AutomationKind::Lfo),
                ..
            })
        ));

        let back_tab = TransportEvent::Key {
            key: TransportKey {
                code: PhysicalKey::BackTab,
                modifiers: Modifiers::SHIFT,
            },
            phase: InputPhase::Press,
            repeat_count: 1,
        };
        assert!(matches!(
            map_input(
                &InteractionMode::Browsing,
                Navigation::default(),
                &back_tab,
                TerminalCapabilities::default()
            ),
            InputMapping::Action(SemanticAction {
                intent: Intent::ChangePage(PageDirection::Previous),
                ..
            })
        ));
    }

    #[test]
    fn mapper_covers_decided_bindings_and_classifies_staged_ones() {
        let event = |code, modifiers| TransportEvent::Key {
            key: TransportKey { code, modifiers },
            phase: InputPhase::Press,
            repeat_count: 1,
        };
        let browsing = InteractionMode::Browsing;
        for (input, intent) in [
            (
                event(PhysicalKey::Up, Modifiers::default()),
                Intent::MoveSelection(-1),
            ),
            (
                event(PhysicalKey::Character('j'), Modifiers::default()),
                Intent::MoveSelection(1),
            ),
            (
                event(PhysicalKey::Left, Modifiers::default()),
                Intent::AdjustSelected(-1),
            ),
            (
                event(PhysicalKey::Character('l'), Modifiers::default()),
                Intent::AdjustSelected(1),
            ),
            (
                event(PhysicalKey::Tab, Modifiers::default()),
                Intent::ChangePage(PageDirection::Next),
            ),
            (
                event(PhysicalKey::BackTab, Modifiers::SHIFT),
                Intent::ChangePage(PageDirection::Previous),
            ),
            (
                event(PhysicalKey::Character('/'), Modifiers::default()),
                Intent::OpenPalette,
            ),
            (
                event(PhysicalKey::Character('1'), Modifiers::default()),
                Intent::BeginNumeric('1'),
            ),
            (
                event(PhysicalKey::Character('q'), Modifiers::default()),
                Intent::Quit,
            ),
            (
                event(PhysicalKey::Character('c'), Modifiers::CONTROL),
                Intent::Quit,
            ),
            (
                event(PhysicalKey::Character('s'), Modifiers::CONTROL),
                Intent::Save,
            ),
        ] {
            assert!(matches!(
                map_input(
                    &browsing,
                    Navigation::default(),
                    &input,
                    TerminalCapabilities::default()
                ),
                InputMapping::Action(SemanticAction {
                    intent: actual,
                    ..
                }) if actual == intent
            ));
        }
        for (code, kind) in [
            (PhysicalKey::Character('p'), PerformanceKind::Deck),
            (PhysicalKey::Character(' '), PerformanceKind::Sequence),
        ] {
            assert_eq!(
                map_input(
                    &browsing,
                    Navigation::default(),
                    &event(code, Modifiers::default()),
                    TerminalCapabilities::default()
                ),
                InputMapping::Action(SemanticAction::press(Intent::ActivatePerformance(kind)))
            );
        }

        let automation =
            InteractionMode::Automation(super::super::interaction::AutomationMode::Lfo {
                depth: super::super::interaction::LfoDepth::Editor,
                selected: 0,
            });
        for (code, intent) in [
            (PhysicalKey::Character('k'), Intent::MoveSelection(-1)),
            (PhysicalKey::Character('j'), Intent::MoveSelection(1)),
            (PhysicalKey::Character('h'), Intent::AdjustSelected(-1)),
            (PhysicalKey::Character('l'), Intent::AdjustSelected(1)),
        ] {
            assert!(matches!(
                map_input(
                    &automation,
                    Navigation::default(),
                    &event(code, Modifiers::default()),
                    TerminalCapabilities::default()
                ),
                InputMapping::Action(SemanticAction {
                    intent: actual,
                    ..
                }) if actual == intent
            ));
        }

        for (code, expected) in [
            (
                'f',
                Intent::OpenAutomation(super::super::interaction::AutomationKind::Lfo),
            ),
            (
                'e',
                Intent::OpenAutomation(super::super::interaction::AutomationKind::Envelope),
            ),
            ('v', Intent::ToggleMacro),
            ('a', Intent::ToggleAuto),
            ('m', Intent::ToggleMute { master: false }),
            ('t', Intent::ToggleUnits),
            ('x', Intent::RemoveAutomation),
            ('r', Intent::ReseedAutomation),
        ] {
            assert!(matches!(
                map_input(
                    &browsing,
                    Navigation::default(),
                    &event(PhysicalKey::Character(code), Modifiers::default()),
                    TerminalCapabilities::default()
                ),
                InputMapping::Action(SemanticAction {
                    intent: actual,
                    ..
                }) if actual == expected
            ));
        }
    }

    #[derive(Clone)]
    struct FakeClock(Rc<Cell<Duration>>);

    impl FakeClock {
        fn new() -> Self {
            Self(Rc::new(Cell::new(Duration::ZERO)))
        }

        fn advance(&self, duration: Duration) {
            self.0.set(self.0.get().saturating_add(duration));
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            self.0.get()
        }
    }

    struct FakeSource {
        clock: FakeClock,
        events: VecDeque<TransportEvent>,
        infinite: Option<TransportEvent>,
        read_cost: Duration,
    }

    impl FakeSource {
        fn finite(clock: FakeClock, events: impl IntoIterator<Item = TransportEvent>) -> Self {
            Self {
                clock,
                events: events.into_iter().collect(),
                infinite: None,
                read_cost: Duration::ZERO,
            }
        }

        fn infinite(clock: FakeClock, event: TransportEvent, read_cost: Duration) -> Self {
            Self {
                clock,
                events: VecDeque::new(),
                infinite: Some(event),
                read_cost,
            }
        }
    }

    impl EventSource for FakeSource {
        fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
            if self.events.is_empty() && self.infinite.is_none() {
                self.clock.advance(timeout);
                Ok(false)
            } else {
                Ok(true)
            }
        }

        fn read(&mut self) -> io::Result<TransportEvent> {
            self.clock.advance(self.read_cost);
            self.events
                .pop_front()
                .or_else(|| self.infinite.clone())
                .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "no event"))
        }
    }

    fn key(character: char, phase: InputPhase) -> TransportEvent {
        TransportEvent::Key {
            key: TransportKey {
                code: PhysicalKey::Character(character),
                modifiers: Modifiers::default(),
            },
            phase,
            repeat_count: 1,
        }
    }

    fn config() -> SchedulerConfig {
        SchedulerConfig {
            queue_capacity: 8,
            max_reads_per_turn: 4,
            input_time_budget: Duration::from_millis(4),
            tick_interval: TICK_INTERVAL,
            frame_interval: FRAME_INTERVAL,
            max_frame_gap: MAX_FRAME_GAP,
        }
    }

    #[test]
    fn scheduler_preserves_input_resize_and_shutdown_order() {
        let clock = FakeClock::new();
        let mut scheduler = Scheduler::new(config(), clock.now());
        scheduler.complete_frame(clock.now());
        let expected = vec![
            key('a', InputPhase::Press),
            TransportEvent::Resize {
                width: 100,
                height: 30,
            },
            key('a', InputPhase::Release),
            TransportEvent::Shutdown,
        ];
        let mut source = FakeSource::finite(clock.clone(), expected.clone());

        let turn = scheduler.collect_turn(&mut source, &clock).expect("turn");
        assert_eq!(turn.events, expected);
        assert!(turn.shutdown_seen);
    }

    #[test]
    fn scheduler_coalesces_only_consecutive_identical_repeats_without_losing_count() {
        let clock = FakeClock::new();
        let mut scheduler = Scheduler::new(
            SchedulerConfig {
                max_reads_per_turn: 8,
                ..config()
            },
            clock.now(),
        );
        scheduler.complete_frame(clock.now());
        let mut source = FakeSource::finite(
            clock.clone(),
            [
                key('j', InputPhase::Repeat),
                key('j', InputPhase::Repeat),
                key('x', InputPhase::Press),
                key('j', InputPhase::Repeat),
            ],
        );

        let turn = scheduler.collect_turn(&mut source, &clock).expect("turn");
        assert_eq!(turn.events.len(), 3);
        assert!(matches!(
            turn.events[0],
            TransportEvent::Key {
                phase: InputPhase::Repeat,
                repeat_count: 2,
                ..
            }
        ));
        assert!(matches!(
            turn.events[2],
            TransportEvent::Key {
                phase: InputPhase::Repeat,
                repeat_count: 1,
                ..
            }
        ));
    }

    #[test]
    fn scheduler_bounds_work_and_forces_frame_under_infinite_repeat_flood() {
        let clock = FakeClock::new();
        let mut scheduler = Scheduler::new(config(), clock.now());
        scheduler.complete_frame(clock.now());
        let mut source = FakeSource::infinite(
            clock.clone(),
            key('j', InputPhase::Repeat),
            Duration::from_millis(1),
        );

        for _ in 0..20 {
            let turn = scheduler.collect_turn(&mut source, &clock).expect("turn");
            assert!(turn.events.len() <= config().queue_capacity);
            assert!(scheduler.queue_len() <= config().queue_capacity);
            if turn.render_due {
                assert!(clock.now() <= MAX_FRAME_GAP);
                return;
            }
        }
        panic!("continuous input never yielded a frame");
    }

    #[test]
    fn hard_frame_deadline_runs_before_queued_input() {
        let clock = FakeClock::new();
        let mut scheduler = Scheduler::new(config(), clock.now());
        scheduler.complete_frame(clock.now());
        clock.advance(MAX_FRAME_GAP);
        let mut source = FakeSource::finite(clock.clone(), [key('a', InputPhase::Press)]);

        let turn = scheduler.collect_turn(&mut source, &clock).expect("turn");
        assert!(turn.render_due);
        assert!(turn.events.is_empty());
    }

    #[test]
    fn state_change_requests_an_immediate_frame() {
        let clock = FakeClock::new();
        let mut scheduler = Scheduler::new(config(), clock.now());
        scheduler.complete_frame(clock.now());
        assert!(!scheduler.render_due(clock.now()));
        scheduler.request_frame();
        assert!(scheduler.render_due(clock.now()));

        let mut source = FakeSource::finite(clock.clone(), [key('a', InputPhase::Repeat)]);
        let turn = scheduler.collect_turn(&mut source, &clock).expect("turn");
        assert!(turn.render_due);
        assert!(turn.events.is_empty());
        assert_eq!(source.events.len(), 1);
    }

    #[test]
    fn scheduler_owns_tick_deadlines_and_waits_for_the_earliest_deadline() {
        let clock = FakeClock::new();
        let mut scheduler = Scheduler::new(
            SchedulerConfig {
                tick_interval: Duration::from_millis(10),
                frame_interval: Duration::from_millis(30),
                ..config()
            },
            clock.now(),
        );
        scheduler.complete_frame(clock.now());
        let mut source = FakeSource::finite(clock.clone(), []);

        let turn = scheduler.collect_turn(&mut source, &clock).expect("turn");
        assert_eq!(clock.now(), Duration::from_millis(10));
        assert!(turn.tick_due);
        assert!(!turn.render_due);

        scheduler.complete_tick(clock.now());
        let next = scheduler.collect_turn(&mut source, &clock).expect("next");
        assert_eq!(clock.now(), Duration::from_millis(20));
        assert!(next.tick_due);
        assert!(!next.render_due);
    }

    #[test]
    fn queue_capacity_backpressures_reliable_events_without_dropping() {
        let clock = FakeClock::new();
        let mut scheduler = Scheduler::new(
            SchedulerConfig {
                queue_capacity: 2,
                max_reads_per_turn: 8,
                ..config()
            },
            clock.now(),
        );
        scheduler.complete_frame(clock.now());
        let events = [
            key('a', InputPhase::Press),
            key('a', InputPhase::Release),
            key('b', InputPhase::Press),
        ];
        let mut source = FakeSource::finite(clock.clone(), events.clone());

        let first = scheduler.collect_turn(&mut source, &clock).expect("first");
        assert_eq!(first.events, events[..2]);
        let second = scheduler.collect_turn(&mut source, &clock).expect("second");
        assert_eq!(second.events, events[2..]);
    }
}
