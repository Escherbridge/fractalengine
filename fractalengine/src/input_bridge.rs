//! Synthetic input for agents: drain the API→Bevy input bridge and replay each
//! batch as the same Bevy input messages winit writes, ahead of `bevy_input`
//! and `bevy_egui` — see src/AGENTS.md §agent-input-surface.

use std::time::Duration;

use bevy::ecs::system::SystemParam;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput, NativeKeyCode};
use bevy::input::mouse::{MouseButton, MouseButtonInput, MouseMotion, MouseScrollUnit, MouseWheel};
use bevy::input::{ButtonState, InputSystems};
use bevy::math::DVec2;
use bevy::prelude::*;
use bevy::render::view::screenshot::Screenshot;
use bevy::time::Real;
use bevy::window::{CursorMoved, PrimaryWindow};
use bevy_egui::{EguiContext, EguiFullOutput, EguiPostUpdateSet, EguiPreUpdateSet, PrimaryEguiContext};
use fe_renderer::camera::CameraCaptureLock;
use fe_runtime::input::{
    camera_locked_by_capture, denied, InputCommand, InputFailure, InputFailureClass, InputOp,
    InputOutcome, InputOutput, InputProgress, InputRequest, InputRequestReceiver, PointerButton,
    ValidInputSpec, INPUT_PARTIALLY_APPLIED_CODE, NO_EFFECTIVE_INPUT_CODE,
};
use fe_ui::navigation_manager::NavigationManager;

use crate::screenshot_bridge::{capture_png, capture_refusal, release_unread_screenshot};
use crate::vision_bridge::ViewScope;

/// Bevy-side wall-clock bound on one injection (input frames + settle +
/// capture), kept under fe-api's 5 s `DEFAULT_INPUT_REPLY_TIMEOUT`: past it a
/// batch still injecting is a typed partial failure (held input released),
/// one fully applied is an unsettled success (src/AGENTS.md §agent-input-surface).
pub const INPUT_WALL_CLOCK_CAP: Duration = Duration::from_millis(4_500);

/// Largest scroll delta (lines, either axis) one command may carry.
pub const SCROLL_LINES_MAX: f32 = 1_000.0;

/// Bevy end of the API→Bevy input bridge.
#[derive(Resource)]
pub struct InputBridge(pub InputRequestReceiver);

/// The one injection in flight (single flight; a second is `input_in_progress`).
#[derive(Resource, Default)]
pub struct InputJob(Option<ActiveInjection>);

/// Suppress clipboard writes for every frame touched by synthetic input.
#[derive(Resource, Default)]
pub struct SyntheticClipboardGuard(bool);

/// The user's real cursor, saved in PreUpdate before a synthetic override and
/// put back in PostUpdate — so bevy_winit never warps the OS cursor.
#[derive(Resource, Default)]
pub struct RealCursor(Option<Option<DVec2>>);

/// One planned native-shaped event or protected Join text marker.
#[derive(Clone, PartialEq)]
enum Synthetic {
    /// Pointer to a primary-window physical-pixel point.
    Cursor(Vec2),
    Button(MouseButton, ButtonState),
    Key {
        code: KeyCode,
        logical: Key,
        state: ButtonState,
    },
    Scroll(Vec2),
    GuardedJoinText,
}

impl std::fmt::Debug for Synthetic {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name=match self {Self::Cursor(..)=>"Cursor",Self::Button(..)=>"Button",Self::Key {..}=>"Key",Self::Scroll(..)=>"Scroll",Self::GuardedJoinText=>"GuardedJoinText"};
        f.debug_struct(name).finish_non_exhaustive()
    }
}

/// Everything one frame injects, in order.
type Frame = Vec<Synthetic>;

enum Phase {
    /// Replaying the planned frames, one per `PreUpdate`.
    Injecting,
    /// All frames applied; counting `wait_frames`.
    Settling,
    /// `capture_after`: waiting for the PNG from the compute pool.
    Capturing {
        screenshot: Entity,
        result: crossbeam::channel::Receiver<Result<Vec<u8>, String>>,
    },
}

/// An admitted request and its replay state.
pub struct ActiveInjection {
    request: InputRequest,
    frames: Vec<Frame>,
    next_frame: usize,
    commands: u32,
    wait_frames: u32,
    waited: u32,
    capture_after: bool,
    started: Duration,
    phase: Phase,
    /// The synthetic pointer (physical px) held on the window while the job runs.
    cursor: Option<Vec2>,
    /// Last pointer position egui/picking were told about (logical px).
    last_logical: Option<Vec2>,
    held_buttons: Vec<MouseButton>,
    held_keys: Vec<(KeyCode, Key)>,
    /// Shared with the API so a late reply still says how far the batch got.
    progress: InputProgress,
}

/// The message streams winit itself writes for pointer, raw motion, wheel and keys.
#[derive(SystemParam)]
pub struct SyntheticWriters<'w> {
    cursor: MessageWriter<'w, CursorMoved>,
    /// Raw device motion: the orbit camera reads it, not `CursorMoved` (QA-0104).
    motion: MessageWriter<'w, MouseMotion>,
    buttons: MessageWriter<'w, MouseButtonInput>,
    wheel: MessageWriter<'w, MouseWheel>,
    keys: MessageWriter<'w, KeyboardInput>,
}

#[derive(SystemParam)]
pub struct GuardedJoinContext<'w, 's> {
    ui: Option<ResMut<'w, fe_ui::actions::UiManager>>,
    identity: Option<Res<'w, fe_identity::NodeIdentity>>,
    contexts: Query<'w, 's, &'static mut EguiContext, With<PrimaryEguiContext>>,
}
impl GuardedJoinContext<'_, '_> {
    fn admit(
        &mut self,
        id: &str,
        nav: Option<&NavigationManager>,
        entity: Entity,
        window: &Window,
        spec: &ValidInputSpec,
    ) -> bool {
        let (Some(ui), Some(identity), Some(nav)) =
            (self.ui.as_deref_mut(), self.identity.as_deref(), nav)
        else {
            return false;
        };
        ui.admit_qa_join_input(id, &identity.did, nav.intent_revision, &join_window(entity, window), spec)
    }
    fn prepare_frame(
        &mut self,
        id: &str,
        index: usize,
        nav: Option<&NavigationManager>,
        entity: Entity,
        window: &Window,
    ) -> bool {
        let (Some(ui), Some(identity), Some(nav), Ok(mut context)) = (
            self.ui.as_deref_mut(), self.identity.as_deref(), nav, self.contexts.single_mut(),
        ) else {
            return false;
        };
        ui.prepare_qa_join_input_frame(
            id, index, &identity.did, nav.intent_revision, &join_window(entity, window), context.get_mut(),
        )
    }
}
fn join_window(entity: Entity, window: &Window) -> fe_runtime::qa_invitation::JoinInputWindow {
    fe_runtime::qa_invitation::JoinInputWindow {
        window_id: entity.to_bits(),
        width: window.physical_width(),
        height: window.physical_height(),
        scale_bits: f64::from(window.scale_factor()).to_bits(),
        focused: window.focused,
    }
}
fn join_guard_refusal() -> InputFailure {
    unresolvable(
        "guarded_join_changed",
        "The guarded masked Join context changed; input stopped without replay".into(),
    )
}

/// Register the input bridge resources and systems (the one wiring `main.rs`
/// and the tests share — src/AGENTS.md §agent-input-surface "Schedule").
pub fn install(app: &mut App, receiver: InputRequestReceiver) {
    app.insert_resource(InputBridge(receiver))
        .init_resource::<InputJob>()
        .init_resource::<SyntheticClipboardGuard>()
        .init_resource::<RealCursor>()
        .add_systems(
            PreUpdate,
            drive_synthetic_input
                .before(InputSystems)
                .before(EguiPreUpdateSet::ProcessInput),
        )
        .add_systems(PostUpdate, restore_real_cursor)
        .add_systems(
            PostUpdate,
            suppress_synthetic_clipboard
                .after(EguiPostUpdateSet::EndPass)
                .before(EguiPostUpdateSet::ProcessOutput),
        );
}

/// Remove synthetic clipboard writes before bevy_egui's platform sink.
fn suppress_synthetic_clipboard(
    guard: Res<SyntheticClipboardGuard>,
    mut outputs: Query<&mut EguiFullOutput>,
) {
    if !guard.0 {
        return;
    }
    let mut dropped = 0;
    for mut output in &mut outputs {
        if let Some(output) = output.0.as_mut() {
            let count = output.platform_output.commands.len();
            output.platform_output.commands.retain(|command| {
                !matches!(
                    command,
                    bevy_egui::egui::OutputCommand::CopyText(_)
                        | bevy_egui::egui::OutputCommand::CopyImage(_)
                )
            });
            dropped += count - output.platform_output.commands.len();
        }
    }
    if dropped > 0 {
        tracing::warn!(
            commands_dropped = dropped,
            "synthetic clipboard writes suppressed"
        );
    }
}

// ---------------------------------------------------------------------------
// Failures
// ---------------------------------------------------------------------------

fn unresolvable(code: &'static str, message: String) -> InputFailure {
    InputFailure::new(InputFailureClass::Unresolvable, code, message)
}

fn window_failure(window: Option<&Window>) -> Option<InputFailure> {
    let reason = capture_refusal(window)?;
    Some(match window {
        None => InputFailure::new(InputFailureClass::NoWindow, "no_window", reason),
        Some(_) => unresolvable("window_minimized", reason.to_string()),
    })
}

// ---------------------------------------------------------------------------
// Planning (pure: ValidInputSpec + window size -> frames, or a typed refusal)
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const LETTER_CODES: [KeyCode; 26] = [
    KeyCode::KeyA, KeyCode::KeyB, KeyCode::KeyC, KeyCode::KeyD, KeyCode::KeyE, KeyCode::KeyF,
    KeyCode::KeyG, KeyCode::KeyH, KeyCode::KeyI, KeyCode::KeyJ, KeyCode::KeyK, KeyCode::KeyL,
    KeyCode::KeyM, KeyCode::KeyN, KeyCode::KeyO, KeyCode::KeyP, KeyCode::KeyQ, KeyCode::KeyR,
    KeyCode::KeyS, KeyCode::KeyT, KeyCode::KeyU, KeyCode::KeyV, KeyCode::KeyW, KeyCode::KeyX,
    KeyCode::KeyY, KeyCode::KeyZ,
];
#[rustfmt::skip]
const DIGIT_CODES: [KeyCode; 10] = [
    KeyCode::Digit0, KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4,
    KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9,
];
#[rustfmt::skip]
const NUMPAD_CODES: [KeyCode; 10] = [
    KeyCode::Numpad0, KeyCode::Numpad1, KeyCode::Numpad2, KeyCode::Numpad3, KeyCode::Numpad4,
    KeyCode::Numpad5, KeyCode::Numpad6, KeyCode::Numpad7, KeyCode::Numpad8, KeyCode::Numpad9,
];

fn character(c: char) -> Key {
    Key::Character(c.to_string().into())
}

/// Map a `KeyCode` variant name to the physical code plus the logical key a
/// US layout produces (letters upper-cased while Shift is held).
fn resolve_key(name: &str, shift: bool) -> Option<(KeyCode, Key)> {
    let single_suffix = |prefix: &str| {
        name.strip_prefix(prefix)
            .filter(|rest| rest.len() == 1)
            .and_then(|rest| rest.chars().next())
    };
    if let Some(letter) = single_suffix("Key").filter(char::is_ascii_uppercase) {
        let code = LETTER_CODES[(letter as u8 - b'A') as usize];
        let produced = if shift {
            letter
        } else {
            letter.to_ascii_lowercase()
        };
        return Some((code, character(produced)));
    }
    if let Some(digit) = single_suffix("Digit").filter(char::is_ascii_digit) {
        return Some((DIGIT_CODES[(digit as u8 - b'0') as usize], character(digit)));
    }
    if let Some(digit) = single_suffix("Numpad").filter(char::is_ascii_digit) {
        return Some((
            NUMPAD_CODES[(digit as u8 - b'0') as usize],
            character(digit),
        ));
    }
    let (code, logical) = match name {
        "Enter" => (KeyCode::Enter, Key::Enter),
        "NumpadEnter" => (KeyCode::NumpadEnter, Key::Enter),
        "Escape" => (KeyCode::Escape, Key::Escape),
        "Tab" => (KeyCode::Tab, Key::Tab),
        "Space" => (KeyCode::Space, Key::Space),
        "Backspace" => (KeyCode::Backspace, Key::Backspace),
        "Delete" => (KeyCode::Delete, Key::Delete),
        "Insert" => (KeyCode::Insert, Key::Insert),
        "Home" => (KeyCode::Home, Key::Home),
        "End" => (KeyCode::End, Key::End),
        "PageUp" => (KeyCode::PageUp, Key::PageUp),
        "PageDown" => (KeyCode::PageDown, Key::PageDown),
        "ArrowUp" => (KeyCode::ArrowUp, Key::ArrowUp),
        "ArrowDown" => (KeyCode::ArrowDown, Key::ArrowDown),
        "ArrowLeft" => (KeyCode::ArrowLeft, Key::ArrowLeft),
        "ArrowRight" => (KeyCode::ArrowRight, Key::ArrowRight),
        "CapsLock" => (KeyCode::CapsLock, Key::CapsLock),
        "ShiftLeft" => (KeyCode::ShiftLeft, Key::Shift),
        "ShiftRight" => (KeyCode::ShiftRight, Key::Shift),
        "ControlLeft" => (KeyCode::ControlLeft, Key::Control),
        "ControlRight" => (KeyCode::ControlRight, Key::Control),
        "AltLeft" => (KeyCode::AltLeft, Key::Alt),
        "AltRight" => (KeyCode::AltRight, Key::Alt),
        "SuperLeft" => (KeyCode::SuperLeft, Key::Super),
        "SuperRight" => (KeyCode::SuperRight, Key::Super),
        "F1" => (KeyCode::F1, Key::F1),
        "F2" => (KeyCode::F2, Key::F2),
        "F3" => (KeyCode::F3, Key::F3),
        "F4" => (KeyCode::F4, Key::F4),
        "F5" => (KeyCode::F5, Key::F5),
        "F6" => (KeyCode::F6, Key::F6),
        "F7" => (KeyCode::F7, Key::F7),
        "F8" => (KeyCode::F8, Key::F8),
        "F9" => (KeyCode::F9, Key::F9),
        "F10" => (KeyCode::F10, Key::F10),
        "F11" => (KeyCode::F11, Key::F11),
        "F12" => (KeyCode::F12, Key::F12),
        "Minus" => (KeyCode::Minus, character('-')),
        "Equal" => (KeyCode::Equal, character('=')),
        "BracketLeft" => (KeyCode::BracketLeft, character('[')),
        "BracketRight" => (KeyCode::BracketRight, character(']')),
        "Backslash" => (KeyCode::Backslash, character('\\')),
        "Semicolon" => (KeyCode::Semicolon, character(';')),
        "Quote" => (KeyCode::Quote, character('\'')),
        "Comma" => (KeyCode::Comma, character(',')),
        "Period" => (KeyCode::Period, character('.')),
        "Slash" => (KeyCode::Slash, character('/')),
        "Backquote" => (KeyCode::Backquote, character('`')),
        "NumpadAdd" => (KeyCode::NumpadAdd, character('+')),
        "NumpadSubtract" => (KeyCode::NumpadSubtract, character('-')),
        "NumpadMultiply" => (KeyCode::NumpadMultiply, character('*')),
        "NumpadDivide" => (KeyCode::NumpadDivide, character('/')),
        "NumpadDecimal" => (KeyCode::NumpadDecimal, character('.')),
        _ => return None,
    };
    Some((code, logical))
}

fn is_modifier(logical: &Key) -> bool {
    matches!(logical, Key::Shift | Key::Control | Key::Alt | Key::Super)
}

/// Keys bevy_egui's `manage_clipboard` turns into a host OS clipboard
/// read/write when chorded with Control/Super (src/AGENTS.md §agent-input-surface
/// "Clipboard chords").
const CLIPBOARD_CODES: [KeyCode; 3] = [KeyCode::KeyC, KeyCode::KeyV, KeyCode::KeyX];

fn holds_command_modifier(held: &[(KeyCode, Key)]) -> bool {
    held.iter()
        .any(|(_, logical)| matches!(logical, Key::Control | Key::Super))
}

fn clipboard_chord(index: usize, field: &str) -> InputFailure {
    unresolvable(
        "clipboard_chord_refused",
        format!(
            "`commands[{index}].{field}` chords with a held Control/Super key, which would \
             read or write the host's OS clipboard; synthetic input never touches the clipboard"
        ),
    )
}

fn key_event(code: KeyCode, logical: Key, state: ButtonState) -> Synthetic {
    Synthetic::Key {
        code,
        logical,
        state,
    }
}

fn mouse_button(button: PointerButton) -> MouseButton {
    match button {
        PointerButton::Left => MouseButton::Left,
        PointerButton::Right => MouseButton::Right,
        PointerButton::Middle => MouseButton::Middle,
    }
}

/// A pointer point inside the primary window's physical pixels (AIS OD-4:
/// the space a screenshot's own PNG reports).
fn in_window(index: usize, field: &str, x: f32, y: f32, size: UVec2) -> Result<Vec2, InputFailure> {
    if x >= 0.0 && y >= 0.0 && x < size.x as f32 && y < size.y as f32 {
        return Ok(Vec2::new(x, y));
    }
    Err(unresolvable(
        "pointer_out_of_window",
        format!(
            "`commands[{index}].{field}` ({x}, {y}) is outside the {}x{} window \
             (primary-window physical pixels, the size a screenshot reports)",
            size.x, size.y
        ),
    ))
}

/// One frame typing `text`: each control-free run arrives as one character
/// key (egui reads it as `Event::Text`), `\n`/`\r\n` as Enter, `\t` as Tab.
fn text_frame(index: usize, text: &str) -> Result<Frame, InputFailure> {
    let mut frame = Frame::new();
    let mut run = String::new();
    let flush = |run: &mut String, frame: &mut Frame| {
        if !run.is_empty() {
            let logical = Key::Character(std::mem::take(run).into());
            let code = KeyCode::Unidentified(NativeKeyCode::Unidentified);
            frame.push(key_event(code, logical.clone(), ButtonState::Pressed));
            frame.push(key_event(code, logical, ButtonState::Released));
        }
    };
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let named = match c {
            '\r' if chars.peek() == Some(&'\n') => continue,
            '\n' | '\r' => Some((KeyCode::Enter, Key::Enter)),
            '\t' => Some((KeyCode::Tab, Key::Tab)),
            c if c.is_control() => {
                return Err(unresolvable(
                    "unsupported_text",
                    format!(
                        "`commands[{index}].text` contains control character U+{:04X}; only \
                         \\n, \\r\\n and \\t are typed (as Enter/Tab)",
                        c as u32
                    ),
                ))
            }
            c => {
                run.push(c);
                None
            }
        };
        if let Some((code, logical)) = named {
            flush(&mut run, &mut frame);
            frame.push(key_event(code, logical.clone(), ButtonState::Pressed));
            frame.push(key_event(code, logical, ButtonState::Released));
        }
    }
    flush(&mut run, &mut frame);
    Ok(frame)
}

/// Settle the pointer before pressing; see AGENTS.md §agent-input-surface.
fn plan_pointer_press(frames: &mut Vec<Frame>, at: Vec2, button: MouseButton) {
    frames.push(vec![Synthetic::Cursor(at)]);
    frames.push(vec![Synthetic::Button(button, ButtonState::Pressed)]);
}

/// Turn a validated batch into per-frame synthetic events, refusing (before
/// anything is injected) whatever only the Bevy side can judge: pointer
/// points outside the window, unmapped key names, unsupported control
/// characters, oversized scrolls. Modifier key presses are sticky: they stay
/// down through the next non-modifier command (so `[ControlLeft, KeyS]` is
/// Ctrl+S) and are released in the following frame, or at the end of the batch.
fn plan_batch(spec: &ValidInputSpec, window_size: UVec2) -> Result<Vec<Frame>, InputFailure> {
    let mut frames: Vec<Frame> = Vec::new();
    let mut held_modifiers: Vec<(KeyCode, Key)> = Vec::new();
    for (index, command) in spec.commands().iter().enumerate() {
        match command {
            InputCommand::PointerMove { x, y } => {
                frames.push(vec![Synthetic::Cursor(in_window(
                    index,
                    "x/y",
                    *x,
                    *y,
                    window_size,
                )?)]);
            }
            InputCommand::PointerClick { x, y, button } => {
                let at = in_window(index, "x/y", *x, *y, window_size)?;
                let button = mouse_button(*button);
                if spec.join_lease().is_some() {
                    // Preserve the separately guarded Join capsule's exact frame indexes.
                    frames.push(vec![
                        Synthetic::Cursor(at),
                        Synthetic::Button(button, ButtonState::Pressed),
                    ]);
                } else {
                    plan_pointer_press(&mut frames, at, button);
                }
                frames.push(vec![Synthetic::Button(button, ButtonState::Released)]);
            }
            InputCommand::PointerDrag { from, to, button } => {
                let start = in_window(index, "from", from.x, from.y, window_size)?;
                let end = in_window(index, "to", to.x, to.y, window_size)?;
                let button = mouse_button(*button);
                plan_pointer_press(&mut frames, start, button);
                frames.push(vec![Synthetic::Cursor(end)]);
                frames.push(vec![Synthetic::Button(button, ButtonState::Released)]);
            }
            InputCommand::KeyPress { key } => {
                let shift = held_modifiers
                    .iter()
                    .any(|(_, logical)| *logical == Key::Shift);
                let (code, logical) = resolve_key(&key.0, shift).ok_or_else(|| {
                    unresolvable(
                        "invalid_key",
                        format!(
                            "`commands[{index}].key` {:?} is not a supported KeyCode name \
                             (e.g. \"Enter\", \"KeyA\", \"Digit1\", \"ArrowLeft\", \"ShiftLeft\", \"F5\")",
                            key.0
                        ),
                    )
                })?;
                if holds_command_modifier(&held_modifiers) && CLIPBOARD_CODES.contains(&code) {
                    return Err(clipboard_chord(index, "key"));
                }
                if is_modifier(&logical) {
                    if !held_modifiers.iter().any(|(held, _)| *held == code) {
                        frames.push(vec![key_event(code, logical.clone(), ButtonState::Pressed)]);
                        held_modifiers.push((code, logical));
                    }
                    continue;
                }
                frames.push(vec![key_event(code, logical.clone(), ButtonState::Pressed)]);
                frames.push(vec![key_event(code, logical, ButtonState::Released)]);
            }
            InputCommand::TextEntry { text } => {
                if spec.join_lease().is_some() {
                    frames.push(vec![Synthetic::GuardedJoinText]);
                    continue;
                }
                // A single-character run under Control/Super is a key chord to
                // bevy_egui (`Key::from_name("v")`), not text.
                if holds_command_modifier(&held_modifiers) {
                    return Err(clipboard_chord(index, "text"));
                }
                let frame = text_frame(index, text)?;
                if !frame.is_empty() {
                    frames.push(frame);
                }
            }
            InputCommand::Scroll { dx, dy } => {
                if dx.abs() > SCROLL_LINES_MAX || dy.abs() > SCROLL_LINES_MAX {
                    return Err(unresolvable(
                        "scroll_out_of_range",
                        format!(
                            "`commands[{index}]` scroll ({dx}, {dy}) exceeds ±{SCROLL_LINES_MAX} lines"
                        ),
                    ));
                }
                frames.push(vec![Synthetic::Scroll(Vec2::new(*dx, *dy))]);
            }
        }
        if !held_modifiers.is_empty() {
            // Keep modifiers through the command's final egui input pass; see AGENTS.md.
            frames.push(
                held_modifiers
                    .drain(..)
                    .map(|(code, logical)| key_event(code, logical, ButtonState::Released))
                    .collect(),
            );
        }
    }
    if !held_modifiers.is_empty() {
        frames.push(
            held_modifiers
                .drain(..)
                .map(|(code, logical)| key_event(code, logical, ButtonState::Released))
                .collect(),
        );
    }
    // INPUT-ADDENDUM item 7 backstop: `validate()` already refuses empty text.
    if frames.is_empty() {
        return Err(unresolvable(
            NO_EFFECTIVE_INPUT_CODE,
            "the batch plans to zero input frames; nothing would be injected".to_string(),
        ));
    }
    Ok(frames)
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

impl ActiveInjection {
    /// Write one planned frame's events and track what is held down.
    fn apply(
        &mut self,
        frame: &[Synthetic],
        window: Entity,
        scale: f32,
        writers: &mut SyntheticWriters,
    ) {
        for event in frame {
            match event {
                Synthetic::Cursor(physical) => {
                    if let Some(previous) = self.cursor {
                        writers.motion.write(MouseMotion {
                            delta: *physical - previous,
                        });
                    }
                    let position = *physical / scale;
                    writers.cursor.write(CursorMoved {
                        window,
                        position,
                        delta: self.last_logical.map(|last| position - last),
                    });
                    self.last_logical = Some(position);
                    self.cursor = Some(*physical);
                }
                Synthetic::Button(button, state) => {
                    writers.buttons.write(MouseButtonInput {
                        button: *button,
                        state: *state,
                        window,
                    });
                    self.held_buttons.retain(|held| held != button);
                    if *state == ButtonState::Pressed {
                        self.held_buttons.push(*button);
                    }
                }
                Synthetic::Key {
                    code,
                    logical,
                    state,
                } => {
                    write_key(writers, window, *code, logical.clone(), *state);
                    self.held_keys.retain(|(held, _)| held != code);
                    if *state == ButtonState::Pressed {
                        self.held_keys.push((*code, logical.clone()));
                    }
                }
                Synthetic::GuardedJoinText => {} // Applied only by the protected TextEdit event capsule.
                Synthetic::Scroll(delta) => {
                    writers.wheel.write(MouseWheel {
                        unit: MouseScrollUnit::Line,
                        x: delta.x,
                        y: delta.y,
                        window,
                    });
                }
            }
        }
    }

    /// Release every button and key this job still holds down, so an aborted
    /// batch never leaves a stuck press (gimbal drag, a modifier) behind.
    fn release_held(&mut self, window: Entity, writers: &mut SyntheticWriters) {
        for button in self.held_buttons.drain(..) {
            writers.buttons.write(MouseButtonInput {
                button,
                state: ButtonState::Released,
                window,
            });
        }
        for (code, logical) in self.held_keys.drain(..) {
            write_key(writers, window, code, logical, ButtonState::Released);
        }
    }

    fn frames_note(&self) -> String {
        format!(
            "{} of {} input frames had been applied",
            self.next_frame,
            self.frames.len()
        )
    }
}

fn write_key(
    writers: &mut SyntheticWriters,
    window: Entity,
    code: KeyCode,
    logical: Key,
    state: ButtonState,
) {
    let text = match (&logical, state) {
        (Key::Character(text), ButtonState::Pressed) => Some(text.clone()),
        _ => None,
    };
    writers.keys.write(KeyboardInput {
        key_code: code,
        logical_key: logical,
        state,
        text,
        repeat: false,
        window,
    });
}

/// Arrival checks for one request; `Ok` carries the planned frames.
fn admit(
    request: &InputRequest,
    busy: bool,
    locked: bool,
    covered: bool,
    window: Option<&Window>,
) -> Result<Vec<Frame>, InputFailure> {
    if locked {
        return Err(camera_locked_by_capture());
    }
    if !covered {
        return Err(denied());
    }
    if busy {
        return Err(InputFailure::new(
            InputFailureClass::Busy,
            "input_in_progress",
            "another input batch is still being applied; retry when it replies",
        ));
    }
    if let Some(refusal) = window_failure(window) {
        return Err(refusal);
    }
    let size = window.map(Window::physical_size).unwrap_or_default();
    let InputOp::Inject(spec) = &request.op;
    plan_batch(spec, size)
}

/// PreUpdate, before `InputSystems` and bevy_egui's input collection: admit
/// new requests (scope, lock, single flight, window, plan), then replay the
/// active batch one frame at a time, settle, optionally capture, and reply.
/// The idle path is one empty `try_recv`.
#[allow(clippy::too_many_arguments)]
pub fn drive_synthetic_input(
    bridge: Res<InputBridge>,
    mut job: ResMut<InputJob>,
    mut clipboard_guard: ResMut<SyntheticClipboardGuard>,
    mut real_cursor: ResMut<RealCursor>,
    time: Res<Time<Real>>,
    capture_lock: Option<Res<CameraCaptureLock>>,
    nav: Option<Res<NavigationManager>>,
    view: ViewScope,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    mut writers: SyntheticWriters,
    mut commands: Commands,
    mut guarded: GuardedJoinContext,
) {
    clipboard_guard.0 = job.0.is_some();
    let locked = capture_lock.is_some();
    while let Ok(request) = bridge.0.try_recv() {
        // The caller already timed out or hung up: never touch input for it.
        if request.reply_tx.is_closed() {
            continue;
        }
        let window = windows.single().ok().map(|(_, window)| window);
        let covered = view.covers(&request.caller_scope, nav.as_deref());
        match admit(&request, job.0.is_some(), locked, covered, window) {
            Ok(frames) => {
                let InputOp::Inject(spec) = &request.op;
                if let Some(id) = spec.join_lease() {
                    let accepted = windows.single().ok().is_some_and(|(entity, window)| {
                        guarded.admit(id, nav.as_deref(), entity, window, spec)
                    });
                    if !accepted {
                        request.reply(Err(join_guard_refusal()));
                        continue;
                    }
                }
                tracing::info!(
                    correlation_id = %request.correlation_id,
                    commands = spec.commands().len(),
                    frames = frames.len(),
                    wait_frames = spec.wait_frames(),
                    "agent input injection started"
                );
                let (commands_count, wait_frames, capture_after) = (
                    spec.commands().len() as u32,
                    spec.wait_frames(),
                    spec.capture_after(),
                );
                let progress = spec.progress().clone();
                progress.admit(frames.len());
                job.0 = Some(ActiveInjection {
                    request,
                    frames,
                    next_frame: 0,
                    commands: commands_count,
                    wait_frames,
                    waited: 0,
                    capture_after,
                    started: time.elapsed(),
                    phase: Phase::Injecting,
                    cursor: None,
                    last_logical: window.and_then(Window::cursor_position),
                    held_buttons: Vec::new(),
                    held_keys: Vec::new(),
                    progress,
                });
            }
            Err(refusal) => {
                tracing::info!(code = refusal.code, "agent input injection refused");
                request.reply(Err(refusal));
            }
        }
    }

    let Some(active) = job.0.as_mut() else {
        return;
    };
    clipboard_guard.0 = true;
    let Ok((window_entity, mut window)) = windows.single_mut() else {
        let active = job.0.take().expect("checked above");
        let failure = window_failure(None).expect("no window is always a refusal");
        finish(active, Err(failure), None, &mut writers, &mut commands,guarded.ui.as_deref_mut());
        return;
    };
    let elapsed = time.elapsed().saturating_sub(active.started);
    // Check at use (INPUT-ADDENDUM item 2): every frame the job touches input.
    let abort = if active.request.reply_tx.is_closed() {
        Some(InputFailure::new(
            InputFailureClass::Failed,
            "caller_gone",
            format!(
                "the caller stopped waiting; injection abandoned ({})",
                active.frames_note()
            ),
        ))
    } else if locked {
        let mut refusal = camera_locked_by_capture();
        refusal.message = format!("{} ({})", refusal.message, active.frames_note());
        Some(refusal)
    } else if !view.covers(&active.request.caller_scope, nav.as_deref()) {
        Some(denied())
    } else if elapsed >= INPUT_WALL_CLOCK_CAP && matches!(active.phase, Phase::Injecting) {
        Some(InputFailure::new(
            InputFailureClass::Failed,
            INPUT_PARTIALLY_APPLIED_CODE,
            format!(
                "the batch did not finish injecting within {} ms ({}); the rest was abandoned \
                 and held input released — re-read state before retrying",
                INPUT_WALL_CLOCK_CAP.as_millis(),
                active.frames_note()
            ),
        ))
    } else {
        None
    };
    if let Some(failure) = abort {
        let active = job.0.take().expect("checked above");
        let real_pointer = window.cursor_position();
        finish(
            active,
            Err(failure),
            Some((window_entity, real_pointer)),
            &mut writers,
            &mut commands,
            guarded.ui.as_deref_mut(),
        );
        return;
    }

    let InputOp::Inject(spec) = &active.request.op;
    if let Some(id) = spec.join_lease() {
        let ready = if matches!(active.phase, Phase::Injecting) {
            guarded.prepare_frame(id, active.next_frame, nav.as_deref(), window_entity, &window)
        } else {
            guarded.ui.as_deref().is_some_and(|ui| ui.qa_join_input_completed(id))
        };
        if !ready {
            let active=job.0.take().expect("checked active input");
            finish(active,Err(join_guard_refusal()),Some((window_entity,window.cursor_position())),&mut writers,&mut commands,guarded.ui.as_deref_mut());
            return;
        }
    }

    // Every frame applied, only settle/capture outstanding: reply truthfully unsettled.
    let out_of_time = elapsed >= INPUT_WALL_CLOCK_CAP;
    // `Some(png?)` once the job is done; `None` while it still runs.
    let done: Option<Result<Option<Vec<u8>>, InputFailure>> = match active.phase {
        Phase::Settling | Phase::Capturing { .. } if out_of_time => Some(Ok(None)),
        Phase::Injecting => {
            // Checked access (INPUT-ADDENDUM item 7): no per-frame input
            // system indexes unchecked — the app is panic=abort.
            if let Some(frame) = active.frames.get_mut(active.next_frame).map(std::mem::take) {
                active.apply(&frame, window_entity, window.scale_factor(), &mut writers);
                active.next_frame += 1;
                active.progress.frame_applied();
            }
            if active.next_frame >= active.frames.len() {
                active.phase = Phase::Settling;
            }
            None
        }
        Phase::Settling if active.waited < active.wait_frames => {
            active.waited += 1;
            active.progress.frame_settled();
            None
        }
        Phase::Settling if active.capture_after => match window_failure(Some(&*window)) {
            Some(refusal) => Some(Err(refusal)),
            None => {
                let (result_tx, result) = crossbeam::channel::bounded(1);
                let on_encoded = move |png| {
                    if result_tx.try_send(png).is_err() {
                        tracing::warn!(
                            "input capture_after result dropped — the job already finished"
                        );
                    }
                };
                let screenshot =
                    capture_png(&mut commands, Screenshot::primary_window(), on_encoded);
                active.phase = Phase::Capturing { screenshot, result };
                None
            }
        },
        Phase::Settling => Some(Ok(None)),
        Phase::Capturing { ref result, .. } => match result.try_recv() {
            Ok(Ok(png)) => Some(Ok(Some(png))),
            Ok(Err(error)) => Some(Err(InputFailure::new(
                InputFailureClass::Failed,
                "capture_failed",
                format!("input applied; the capture_after frame failed: {error}"),
            ))),
            Err(crossbeam::channel::TryRecvError::Empty) => None,
            Err(crossbeam::channel::TryRecvError::Disconnected) => Some(Err(InputFailure::new(
                InputFailureClass::Failed,
                "capture_dropped",
                "input applied; the renderer dropped the capture_after request",
            ))),
        },
    };

    if let Some(done) = done {
        let output = done.map(|png| InputOutput {
            commands_applied: active.commands,
            wait_frames_used: active.waited,
            settled: !out_of_time,
            png,
        });
        let active = job.0.take().expect("checked above");
        let real_pointer = window.cursor_position();
        finish(
            active,
            output,
            Some((window_entity, real_pointer)),
            &mut writers,
            &mut commands,
            guarded.ui.as_deref_mut(),
        );
        return;
    }
    // Hold the synthetic pointer on the window for this frame's readers
    // (picking, gimbal, viewport click), without flagging the `Window`
    // changed: bevy_winit would otherwise warp the real OS cursor.
    if let Some(cursor) = active.cursor {
        if real_cursor.0.is_none() {
            real_cursor.0 = Some(window.physical_cursor_position().map(|p| p.as_dvec2()));
        }
        window
            .bypass_change_detection()
            .set_physical_cursor_position(Some(cursor.as_dvec2()));
    }
}

/// THE single job exit: release anything still held, hand egui/picking the
/// user's real pointer back, despawn a capture that will never be read, then
/// reply. `window` is the primary window and its real (logical) cursor.
fn finish(
    mut active: ActiveInjection,
    outcome: InputOutcome,
    window: Option<(Entity, Option<Vec2>)>,
    writers: &mut SyntheticWriters,
    commands: &mut Commands,
    ui: Option<&mut fe_ui::actions::UiManager>,
) {
    let InputOp::Inject(spec) = &active.request.op;
    if let Some(id) = spec.join_lease() {
        if let Some(ui) = ui {
            ui.finish_qa_join_input(id);
        }
    }
    if let Some((window, real_pointer)) = window {
        active.release_held(window, writers);
        if let (Some(_), Some(position)) = (active.cursor, real_pointer) {
            writers.cursor.write(CursorMoved {
                window,
                position,
                delta: active.last_logical.map(|last| position - last),
            });
        }
    }
    if let Phase::Capturing { screenshot, .. } = active.phase {
        // A failed or unsettled job never reads its pending capture.
        if !matches!(&outcome, Ok(output) if output.png.is_some()) {
            release_unread_screenshot(commands, screenshot);
        }
    }
    if let Err(failure) = &outcome {
        tracing::info!(code = failure.code, "agent input injection refused/failed");
    }
    active.request.reply(outcome);
}

/// PostUpdate: put the user's real cursor back on the window (bypassing
/// change detection, like the override) before bevy_winit syncs windows in
/// `Last`, so a synthetic pointer never becomes an OS cursor warp.
pub fn restore_real_cursor(
    mut saved: ResMut<RealCursor>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    let Some(real) = saved.0.take() else {
        return;
    };
    if let Ok(mut window) = windows.single_mut() {
        window
            .bypass_change_detection()
            .set_physical_cursor_position(real);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::mouse::AccumulatedMouseScroll;
    use bevy::input::InputPlugin;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    use bevy::render::view::screenshot::{Capturing, ScreenshotCaptured};
    use bevy::tasks::{AsyncComputeTaskPool, TaskPool};
    use bevy_egui::input::{write_modifiers_keys_state_system, ModifierKeysState};
    use fe_runtime::input::{
        input_bridge, InputKey, InputReply, InputRequestSender, InputSpec, PointerPoint,
        CAMERA_LOCKED_BY_CAPTURE_CODE, DENIED_CODE,
    };
    use fe_ui::verse_manager::VerseManager;
    use tokio::sync::oneshot::Receiver;

    use crate::screenshot_bridge::encode_png;

    /// The primary window's cursor as an `Update` reader (gimbal, viewport
    /// click, picking) saw it each frame.
    #[derive(Resource, Default)]
    struct SeenInUpdate(Vec<Option<Vec2>>);

    #[derive(Default)]
    struct ScrollControl {
        seed_offset: Option<f32>,
        samples: Vec<(f32, bevy_egui::egui::Vec2)>,
        target: Option<bevy_egui::egui::Rect>,
        viewport: Option<bevy_egui::egui::Rect>,
        selected: Vec<(usize, bevy_egui::egui::Modifiers)>,
    }

    #[derive(Resource, Default)]
    struct CopyControl {
        context: bevy_egui::egui::Context,
        requested: usize,
        sink_text: usize,
        sink_images: usize,
        late_copy: bool,
        modifier_frames: Vec<ModifierKeysState>,
        modifier_clicks: Vec<bevy_egui::egui::Modifiers>,
        scroll: Option<ScrollControl>,
    }

    fn render_copy_control(
        mut control: ResMut<CopyControl>,
        buttons: Res<ButtonInput<MouseButton>>,
        modifiers: Res<ModifierKeysState>,
        windows: Query<&Window, With<PrimaryWindow>>,
        mut outputs: Query<&mut EguiFullOutput>,
    ) {
        use bevy_egui::egui;
        let cursor = windows.single().unwrap().cursor_position();
        control.modifier_frames.push(*modifiers);
        let egui_modifiers = modifiers.to_egui_modifiers();
        let mut input = egui::RawInput {
            modifiers: egui_modifiers,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        if let Some(cursor) = cursor {
            let pos = egui::pos2(cursor.x, cursor.y);
            input.events.push(egui::Event::PointerMoved(pos));
            for (pressed, happened) in [
                (true, buttons.just_pressed(MouseButton::Left)),
                (false, buttons.just_released(MouseButton::Left)),
            ] {
                if happened {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui_modifiers,
                    });
                }
            }
        } else {
            input.events.push(egui::Event::PointerGone);
        }
        let late_copy = std::mem::take(&mut control.late_copy);
        let mut clicked_modifiers = None;
        let mut scroll = control.scroll.take();
        let mut scroll_sample = None;
        let mut selected_row = None;
        let output = control.context.run(input, |ctx| {
            let clicked = egui::Area::new(egui::Id::new("copy-control"))
                .fixed_pos(egui::pos2(80.0, 35.0))
                .show(ctx, |ui| {
                    ui.add_sized([150.0, 30.0], egui::Button::new("Copy value"))
                        .clicked()
                })
                .inner;
            if clicked {
                clicked_modifiers = Some(ctx.input(|input| input.modifiers));
            }
            if clicked || late_copy {
                ctx.copy_text("clipboard sentinel".into());
                ctx.copy_image(egui::ColorImage::new([1, 1], vec![egui::Color32::RED]));
            }
            if let Some(scroll) = scroll.as_mut() {
                let output = egui::Area::new(egui::Id::new("scroll-control"))
                    .fixed_pos(egui::pos2(300.0, 80.0))
                    .show(ctx, |ui| {
                        ui.set_width(220.0);
                        let mut area = egui::ScrollArea::vertical()
                            .id_salt("populated-rows")
                            .max_height(240.0)
                            .auto_shrink([false, false]);
                        if let Some(offset) = scroll.seed_offset.take() {
                            area = area.vertical_scroll_offset(offset);
                        }
                        area.show(ui, |ui| {
                            for row in 0..40 {
                                let response = ui.add_sized(
                                    [200.0, 24.0],
                                    egui::Button::new(format!("Select row {row}"))
                                        .sense(egui::Sense::click()),
                                );
                                if row == 8 {
                                    scroll.target = Some(response.rect);
                                }
                                if response.clicked() {
                                    selected_row = Some((row, ctx.input(|i| i.modifiers)));
                                }
                            }
                        })
                    })
                    .inner;
                scroll.viewport = Some(output.inner_rect);
                scroll_sample = Some((output.state.offset.y, ctx.input(|i| i.pointer.delta())));
            }
        });
        if let Some(scroll) = scroll.as_mut() {
            if let Some(sample) = scroll_sample {
                scroll.samples.push(sample);
            }
            if let Some(row) = selected_row {
                scroll.selected.push(row);
            }
        }
        control.scroll = scroll;
        if let Some(modifiers) = clicked_modifiers {
            control.modifier_clicks.push(modifiers);
        }
        control.requested += output
            .platform_output
            .commands
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    egui::OutputCommand::CopyText(_) | egui::OutputCommand::CopyImage(_)
                )
            })
            .count();
        outputs.single_mut().unwrap().0 = Some(output);
    }

    fn record_clipboard_sink(
        mut control: ResMut<CopyControl>,
        mut outputs: Query<&mut EguiFullOutput>,
    ) {
        for mut output in &mut outputs {
            if let Some(output) = output.0.take() {
                for command in output.platform_output.commands {
                    match command {
                        bevy_egui::egui::OutputCommand::CopyText(_) => control.sink_text += 1,
                        bevy_egui::egui::OutputCommand::CopyImage(_) => control.sink_images += 1,
                        _ => {}
                    }
                }
            }
        }
    }

    fn record_update_cursor(
        windows: Query<&Window, With<PrimaryWindow>>,
        mut seen: ResMut<SeenInUpdate>,
    ) {
        seen.0.push(
            windows
                .single()
                .ok()
                .and_then(Window::physical_cursor_position),
        );
    }

    struct Harness {
        app: App,
        tx: InputRequestSender,
        window: Entity,
    }

    fn harness() -> Harness {
        AsyncComputeTaskPool::get_or_init(TaskPool::new);
        let mut app = App::new();
        app.add_plugins(InputPlugin)
            .add_message::<CursorMoved>()
            .init_resource::<ModifierKeysState>()
            .add_systems(
                PreUpdate,
                write_modifiers_keys_state_system
                    .after(InputSystems)
                    .after(drive_synthetic_input),
            );
        let (tx, rx) = input_bridge();
        install(&mut app, rx);
        let mut time = Time::<Real>::default();
        time.update_with_duration(Duration::ZERO);
        app.insert_resource(time);
        let mut nav = NavigationManager::default();
        nav.navigate_to_verse("v", "Verse");
        app.insert_resource(nav);
        app.insert_resource(VerseManager::default());
        let mut window = Window::default();
        window.resolution.set_physical_resolution(800, 600);
        let window = app
            .world_mut()
            .spawn((window, PrimaryWindow, EguiFullOutput::default()))
            .id();
        app.init_resource::<SeenInUpdate>()
            .init_resource::<CopyControl>()
            .add_systems(Update, (record_update_cursor, render_copy_control))
            .add_systems(
                PostUpdate,
                record_clipboard_sink.in_set(EguiPostUpdateSet::ProcessOutput),
            );
        Harness { app, tx, window }
    }

    fn click(x: f32, y: f32) -> InputCommand {
        InputCommand::PointerClick {
            x,
            y,
            button: PointerButton::Left,
        }
    }

    fn key(name: &str) -> InputCommand {
        InputCommand::KeyPress {
            key: InputKey(name.to_string()),
        }
    }

    fn batch(commands: Vec<InputCommand>, wait_frames: u32, capture_after: bool) -> InputSpec {
        InputSpec {
            commands,
            wait_frames: Some(wait_frames),
            capture_after: Some(capture_after),
        }
    }

    impl Harness {
        fn send_as(&self, scope: &str, spec: InputSpec) -> Receiver<InputReply> {
            let valid = spec.validate().expect("statically valid batch");
            let (request, reply) = InputRequest::new("c1", scope, InputOp::Inject(valid));
            self.tx.try_send(request).expect("bridge has room");
            reply
        }

        fn send(&self, spec: InputSpec) -> Receiver<InputReply> {
            self.send_as("VERSE#v", spec)
        }

        fn mouse(&self) -> &ButtonInput<MouseButton> {
            self.app.world().resource::<ButtonInput<MouseButton>>()
        }

        fn keys(&self) -> &ButtonInput<KeyCode> {
            self.app.world().resource::<ButtonInput<KeyCode>>()
        }

        fn logical(&self) -> &ButtonInput<Key> {
            self.app.world().resource::<ButtonInput<Key>>()
        }

        fn last_update_cursor(&self) -> Option<Vec2> {
            *self
                .app
                .world()
                .resource::<SeenInUpdate>()
                .0
                .last()
                .expect("a frame ran")
        }

        fn window_cursor_after_frame(&mut self) -> Option<Vec2> {
            let world = self.app.world_mut();
            let mut windows = world.query_filtered::<&Window, With<PrimaryWindow>>();
            windows
                .single(world)
                .expect("one window")
                .physical_cursor_position()
        }

        fn idle(&self) -> bool {
            self.app.world().resource::<InputJob>().0.is_none()
        }

        fn set_verse(&mut self, verse: &str) {
            self.app
                .world_mut()
                .resource_mut::<NavigationManager>()
                .navigate_to_verse(verse, "Verse");
        }
    }

    fn refusal_code(reply: &mut Receiver<InputReply>) -> &'static str {
        reply
            .try_recv()
            .expect("answered this frame")
            .result
            .expect_err("refused")
            .code
    }

    /// One journey over the Bevy-side input translation (TEST POLICY
    /// 2026-11-01: a brand-new region gets one long journey): the idle frame,
    /// a click becoming real `ButtonInput` presses with the synthetic pointer
    /// visible to `Update` readers and the real cursor put back after the
    /// frame, the `wait_frames` hold, every arrival refusal (with nothing
    /// injected), sticky-modifier chords + text, a drag + scroll, single
    /// flight, check-at-use scope loss and a capture lock mid-batch (both
    /// releasing the held button), the wall-clock cap, `capture_after`
    /// returning the same PNG a direct capture encodes, and an abandoned
    /// caller. Every assertion goes red if its branch inverts.
    #[test]
    fn synthetic_input_replays_as_real_bevy_input_and_refuses_at_arrival_and_at_use() {
        crate::qa_debug_bridge::check_asset_audit_ownership();
        let mut protected = batch(vec![click(10.0, 10.0),InputCommand::TextEntry {text:"fractalengine://join/v1/protected-fixture".into()},click(20.0,20.0)],0,false).validate().unwrap();
        protected.require_join_lease("join-input-fixture").unwrap();
        let frames=plan_batch(&protected,UVec2::splat(100)).unwrap();
        assert_eq!(frames.len(),5);
        for (index, at) in [(0, Vec2::splat(10.0)), (3, Vec2::splat(20.0))] {
            assert_eq!(
                frames[index],
                vec![
                    Synthetic::Cursor(at),
                    Synthetic::Button(MouseButton::Left, ButtonState::Pressed)
                ],
                "only the fixed-index guarded Join capsule retains same-frame positioning/press"
            );
            assert_eq!(
                frames[index + 1],
                vec![Synthetic::Button(MouseButton::Left, ButtonState::Released)]
            );
        }
        assert_eq!(
            frames[2],
            vec![Synthetic::GuardedJoinText],
            "Guarded text never becomes global KeyboardInput with a logical secret"
        );
        let bounded = batch(
            vec![
                InputCommand::PointerDrag {
                    from: PointerPoint { x: 10.0, y: 10.0 },
                    to: PointerPoint { x: 20.0, y: 20.0 },
                    button: PointerButton::Left,
                };
                fe_runtime::input::MAX_COMMANDS_PER_REQUEST
            ],
            0,
            false,
        )
        .validate()
        .unwrap();
        assert_eq!(plan_batch(&bounded, UVec2::splat(100)).unwrap().len(), 256);
        assert!(!format!("{frames:?}").contains("protected-fixture"));
        {
            use bevy_egui::egui::accesskit as ak;
            let mut root = ak::Node::new(ak::Role::Window);
            root.set_children([ak::NodeId(2), ak::NodeId(3), ak::NodeId(5)]);
            root.set_transform(ak::Affine::scale(2.0));
            root.set_disabled();
            let mut button = ak::Node::new(ak::Role::Button);
            button.set_label("Maps");
            button.add_action(ak::Action::Click);
            button.set_transform(ak::Affine::translate((10.0, 20.0)));
            button.set_bounds(ak::Rect::new(1.0, 2.0, 11.0, 12.0));
            let mut password = ak::Node::new(ak::Role::PasswordInput);
            password.set_value("secret-value");
            password.set_children([ak::NodeId(4)]);
            let mut text = ak::Node::new(ak::Role::TextRun);
            text.set_value("secret-value");
            let mut linked = ak::Node::new(ak::Role::Button);
            linked.set_labelled_by([ak::NodeId(3), ak::NodeId(4), ak::NodeId(6)]);
            let mut unvisited = ak::Node::new(ak::Role::PasswordInput);
            unvisited.set_value("unvisited-secret");
            let update = ak::TreeUpdate {
                nodes: vec![
                    (ak::NodeId(1), root),
                    (ak::NodeId(2), button),
                    (ak::NodeId(3), password),
                    (ak::NodeId(4), text),
                    (ak::NodeId(5), linked),
                    (ak::NodeId(6), unvisited),
                ],
                tree: Some(ak::Tree::new(ak::NodeId(1))),
                focus: ak::NodeId(2),
            };
            let flat = crate::qa_debug_bridge::flatten(&update, &Default::default());
            let button = flat.iter().find(|n| n["name"] == "Maps").unwrap();
            assert_eq!(
                button["bounds"],
                serde_json::json!({"x":22.0,"y":44.0,"w":20.0,"h":20.0})
            );
            assert_eq!(button["enabled"], false);
            assert_eq!(button["parent"], "1");
            assert!(!serde_json::to_string(&flat)
                .unwrap()
                .contains("secret-value"));
            assert!(!serde_json::to_string(&flat)
                .unwrap()
                .contains("unvisited-secret"));
        }
        let mut h = harness();

        // Idle: an empty channel injects nothing and leaves no job.
        h.app.update();
        assert!(h.idle());
        assert_eq!(h.last_update_cursor(), None);
        assert!(h.mouse().get_just_pressed().next().is_none());

        // A click positions first, then presses/releases before its two settle frames.
        let mut reply = h.send(batch(vec![click(100.0, 50.0)], 2, false));
        h.app.update();
        assert!(
            !h.mouse().pressed(MouseButton::Left),
            "positioning cannot press"
        );
        assert_eq!(h.last_update_cursor(), Some(Vec2::new(100.0, 50.0)));
        h.app.update();
        assert!(
            h.mouse().just_pressed(MouseButton::Left),
            "the press reaches ButtonInput the frame it arrives"
        );
        assert_eq!(
            h.last_update_cursor(),
            Some(Vec2::new(100.0, 50.0)),
            "Update readers see the synthetic pointer"
        );
        assert_eq!(
            h.window_cursor_after_frame(),
            None,
            "the real cursor is put back after the frame (never an OS warp)"
        );
        h.app.update();
        assert!(h.mouse().just_released(MouseButton::Left));
        assert!(!h.mouse().pressed(MouseButton::Left));
        assert_eq!(
            h.app.world().resource::<CopyControl>().requested,
            2,
            "the injected click reaches a real egui copying control"
        );
        assert_eq!(h.app.world().resource::<CopyControl>().sink_text, 0);
        assert_eq!(h.app.world().resource::<CopyControl>().sink_images, 0);
        for _ in 0..2 {
            h.app.world_mut().resource_mut::<CopyControl>().late_copy = true;
            h.app.update();
            assert!(reply.try_recv().is_err(), "held while settling");
            assert_eq!(
                h.last_update_cursor(),
                Some(Vec2::new(100.0, 50.0)),
                "the pointer is held through the settle"
            );
        }
        h.app.world_mut().resource_mut::<CopyControl>().late_copy = true;
        h.app.update();
        let output = reply
            .try_recv()
            .expect("replied after the settle")
            .result
            .expect("applied");
        assert_eq!(
            (output.commands_applied, output.wait_frames_used, output.png),
            (1, 2, None)
        );
        assert!(h.idle());
        assert_eq!(h.app.world().resource::<CopyControl>().requested, 8);
        assert_eq!(
            h.app.world().resource::<CopyControl>().sink_text,
            0,
            "settle and finish frames retain clipboard isolation"
        );
        assert_eq!(h.app.world().resource::<CopyControl>().sink_images, 0);
        assert_eq!(
            h.last_update_cursor(),
            None,
            "the finish frame hands the real pointer back"
        );

        h.app
            .world_mut()
            .get_mut::<Window>(h.window)
            .unwrap()
            .set_physical_cursor_position(Some(DVec2::new(100.0, 50.0)));
        for state in [ButtonState::Pressed, ButtonState::Released] {
            h.app.world_mut().write_message(MouseButtonInput {
                button: MouseButton::Left,
                state,
                window: h.window,
            });
            h.app.update();
        }
        assert_eq!(
            h.app.world().resource::<CopyControl>().sink_text,
            1,
            "ordinary user copying remains enabled outside injection"
        );
        assert_eq!(h.app.world().resource::<CopyControl>().sink_images, 1);
        h.app
            .world_mut()
            .get_mut::<Window>(h.window)
            .unwrap()
            .set_physical_cursor_position(None);
        h.app.update();

        // Arrival refusals: typed, and nothing is injected.
        let mut unknown_key = h.send(batch(vec![key("Foo")], 0, false));
        let mut off_window = h.send(batch(vec![click(800.0, 10.0)], 0, false));
        let mut bell = h.send(batch(
            vec![InputCommand::TextEntry {
                text: "a\u{7}".into(),
            }],
            0,
            false,
        ));
        let mut huge_scroll = h.send(batch(
            vec![InputCommand::Scroll {
                dx: 0.0,
                dy: 5_000.0,
            }],
            0,
            false,
        ));
        h.app.update();
        assert_eq!(refusal_code(&mut unknown_key), "invalid_key");
        assert_eq!(refusal_code(&mut off_window), "pointer_out_of_window");
        assert_eq!(refusal_code(&mut bell), "unsupported_text");
        assert_eq!(refusal_code(&mut huge_scroll), "scroll_out_of_range");
        // Control/Super + C/V/X (as a key or a one-character text run) would
        // read or write the host OS clipboard through bevy_egui: refused.
        // Ctrl+A stays a legal chord.
        let mut paste = h.send(batch(vec![key("ControlLeft"), key("KeyV")], 0, false));
        let mut copy = h.send(batch(vec![key("SuperLeft"), key("KeyC")], 0, false));
        let mut text_paste = h.send(batch(
            vec![
                key("ControlRight"),
                InputCommand::TextEntry { text: "v".into() },
            ],
            0,
            false,
        ));
        h.app.update();
        assert_eq!(refusal_code(&mut paste), "clipboard_chord_refused");
        assert_eq!(refusal_code(&mut copy), "clipboard_chord_refused");
        assert_eq!(refusal_code(&mut text_paste), "clipboard_chord_refused");
        assert!(h.keys().get_pressed().next().is_none());
        let mut select_all = h.send(batch(vec![key("ControlLeft"), key("KeyA")], 0, false));
        let select_all = (0..10)
            .find_map(|_| {
                h.app.update();
                select_all.try_recv().ok()
            })
            .expect("a Ctrl+A chord completes");
        assert!(select_all.result.is_ok(), "Ctrl+A is not a clipboard chord");
        assert!(h.idle());
        let mut foreign = h.send_as("VERSE#other", batch(vec![click(1.0, 1.0)], 0, false));
        h.app.world_mut().insert_resource(CameraCaptureLock);
        let mut while_locked = h.send(batch(vec![click(1.0, 1.0)], 0, false));
        h.app.update();
        assert_eq!(
            refusal_code(&mut foreign),
            CAMERA_LOCKED_BY_CAPTURE_CODE,
            "the capture lock is checked first"
        );
        assert_eq!(
            refusal_code(&mut while_locked),
            CAMERA_LOCKED_BY_CAPTURE_CODE
        );
        h.app.world_mut().remove_resource::<CameraCaptureLock>();
        let mut foreign = h.send_as("VERSE#other", batch(vec![click(1.0, 1.0)], 0, false));
        h.app.update();
        assert_eq!(
            refusal_code(&mut foreign),
            DENIED_CODE,
            "an uncovered verse is the uniform denial"
        );
        assert!(h.idle());
        assert!(h.mouse().get_pressed().next().is_none());
        assert!(h.keys().get_pressed().next().is_none());

        // A sticky modifier chord (Shift+A), then text with a newline.
        let mut reply = h.send(batch(
            vec![
                key("ShiftLeft"),
                key("KeyA"),
                InputCommand::TextEntry {
                    text: "hi\n".into(),
                },
            ],
            0,
            false,
        ));
        h.app.update();
        assert!(h.keys().pressed(KeyCode::ShiftLeft));
        h.app.update();
        assert!(h.keys().just_pressed(KeyCode::KeyA));
        assert!(
            h.logical().just_pressed(Key::Character("A".into())),
            "Shift upper-cases the letter"
        );
        assert!(
            h.keys().pressed(KeyCode::ShiftLeft),
            "the modifier is held through the chord"
        );
        h.app.update();
        assert!(h.keys().just_released(KeyCode::KeyA));
        assert!(h.keys().pressed(KeyCode::ShiftLeft));
        h.app.update();
        assert!(h.keys().just_released(KeyCode::ShiftLeft));
        h.app.update();
        assert!(
            h.logical().just_pressed(Key::Character("hi".into())),
            "a text run is one character key (egui reads it as Event::Text)"
        );
        assert!(h.keys().just_pressed(KeyCode::Enter), "\\n types Enter");
        assert!(
            h.keys().get_pressed().next().is_none(),
            "text leaves nothing held"
        );
        h.app.update();
        let output = reply.try_recv().expect("done").result.expect("applied");
        assert_eq!(output.commands_applied, 3);

        // Use bevy_egui's production collector for every consuming command type.
        for (command, command_frames) in [
            (click(100.0, 50.0), 3),
            (InputCommand::PointerMove { x: 100.0, y: 50.0 }, 1),
            (
                InputCommand::PointerDrag {
                    from: PointerPoint { x: 10.0, y: 10.0 },
                    to: PointerPoint { x: 100.0, y: 50.0 },
                    button: PointerButton::Left,
                },
                4,
            ),
            (key("KeyA"), 2),
            (
                InputCommand::TextEntry {
                    text: "shifted".into(),
                },
                1,
            ),
            (InputCommand::Scroll { dx: 0.0, dy: 1.0 }, 1),
        ] {
            let start = h
                .app
                .world()
                .resource::<CopyControl>()
                .modifier_frames
                .len();
            let mut reply = h.send(batch(vec![key("ShiftLeft"), command], 0, false));
            for _ in 0..=command_frames {
                h.app.update();
                assert!(
                    h.app
                        .world()
                        .resource::<ModifierKeysState>()
                        .shift,
                    "the production collector retains Shift through the final command frame"
                );
                assert!(h.app.world().resource::<SyntheticClipboardGuard>().0);
                assert!(reply.try_recv().is_err());
            }
            h.app.update();
            assert!(h.keys().just_released(KeyCode::ShiftLeft));
            assert!(
                !h.app
                    .world()
                    .resource::<ModifierKeysState>()
                    .shift
            );
            assert!(
                h.app.world().resource::<SyntheticClipboardGuard>().0,
                "the additional release frame keeps clipboard suppression"
            );
            let frames = &h.app.world().resource::<CopyControl>().modifier_frames[start..];
            assert_eq!(frames.len(), command_frames + 2);
            assert!(frames[..command_frames + 1].iter().all(|frame| frame.shift));
            assert!(!frames[command_frames + 1].shift);
            h.app.update();
            assert!(reply
                .try_recv()
                .expect("reply after modifier release")
                .result
                .is_ok());
            assert!(h.app.world().resource::<SyntheticClipboardGuard>().0);
            assert!(h.keys().get_pressed().next().is_none());
        }

        // egui's actual release-frame clicked response must still see the whole chord.
        let clicks_before = h
            .app
            .world()
            .resource::<CopyControl>()
            .modifier_clicks
            .len();
        let mut reply = h.send(batch(
            vec![
                key("ControlLeft"),
                key("ShiftLeft"),
                key("AltLeft"),
                key("SuperLeft"),
                click(100.0, 50.0),
                key("ControlLeft"),
                click(100.0, 50.0),
                click(100.0, 50.0),
            ],
            0,
            false,
        ));
        let answer = (0..20)
            .find_map(|_| {
                h.app.update();
                reply.try_recv().ok()
            })
            .expect("three separately consumed pointer commands complete");
        assert!(answer.result.is_ok());
        let control = h.app.world().resource::<CopyControl>();
        let clicks = &control.modifier_clicks[clicks_before..];
        assert_eq!(clicks.len(), 3);
        assert!(clicks[0].ctrl && clicks[0].shift && clicks[0].alt);
        assert!(clicks[1].ctrl && !clicks[1].shift && !clicks[1].alt);
        assert_eq!(clicks[2], bevy_egui::egui::Modifiers::default());
        assert!(
            control.modifier_frames.iter().any(|frame| frame.win),
            "the production collector also sees Super while the first click is consumed"
        );
        assert!(h.keys().get_pressed().next().is_none());

        // The real planner/Bevy collector feeds a populated egui area with drag scrolling enabled.
        {
            let mut scrolled = harness();
            let scroll = ScrollControl {
                seed_offset: Some(120.0),
                ..Default::default()
            };
            scrolled.app.world_mut().resource_mut::<CopyControl>().scroll = Some(scroll);
            scrolled
                .app
                .world_mut()
                .get_mut::<Window>(scrolled.window)
                .unwrap()
                .set_physical_cursor_position(Some(DVec2::new(720.0, 0.0)));
            for _ in 0..3 {
                scrolled.app.update();
            }
            let (target, baseline) = {
                let control = scrolled.app.world().resource::<CopyControl>();
                let scroll = control.scroll.as_ref().unwrap();
                let target = scroll.target.unwrap().center();
                assert!(scroll.viewport.unwrap().contains(target));
                let baseline = scroll.samples.last().unwrap().0;
                assert_eq!(baseline, 120.0, "seeded once, never forced after input");
                assert!(
                    target.y > baseline,
                    "the old press-frame teleport would clamp the offset to zero"
                );
                (target, baseline)
            };
            for modified in [false, true] {
                let mut commands = Vec::new();
                if modified {
                    commands.push(key("ControlLeft"));
                }
                commands.push(click(target.x, target.y));
                let mut reply = scrolled.send(batch(commands, 2, false));
                let mut completed = false;
                for _ in 0..10 {
                    scrolled.app.update();
                    let control = scrolled.app.world().resource::<CopyControl>();
                    let (offset, delta) = *control.scroll.as_ref().unwrap().samples.last().unwrap();
                    assert_eq!(
                        offset, baseline,
                        "position/press/release/settle cannot move the scroll offset"
                    );
                    if scrolled.mouse().just_pressed(MouseButton::Left) {
                        assert_eq!(
                            delta,
                            bevy_egui::egui::Vec2::ZERO,
                            "press sees no unrelated teleport delta"
                        );
                    }
                    if let Ok(answer) = reply.try_recv() {
                        assert!(answer.result.is_ok());
                        completed = true;
                        break;
                    }
                }
                assert!(completed);
                let control = scrolled.app.world().resource::<CopyControl>();
                let selected = &control.scroll.as_ref().unwrap().selected;
                assert_eq!(
                    selected.len(),
                    if modified { 2 } else { 1 },
                    "each click selects exactly one row"
                );
                assert_eq!(selected.last().unwrap().0, 8);
                assert_eq!(selected.last().unwrap().1.ctrl, modified);
                assert!(!scrolled.mouse().pressed(MouseButton::Left));
                assert!(scrolled.keys().get_pressed().next().is_none());
            }
            // Drag start also settles first; only the declared ten-pixel motion scrolls.
            let mut reply = scrolled.send(batch(
                vec![
                    key("ShiftLeft"),
                    InputCommand::PointerDrag {
                        from: PointerPoint {
                            x: target.x,
                            y: target.y,
                        },
                        to: PointerPoint {
                            x: target.x,
                            y: target.y - 10.0,
                        },
                        button: PointerButton::Left,
                    },
                ],
                0,
                false,
            ));
            for frame in 0..6 {
                scrolled.app.update();
                let control = scrolled.app.world().resource::<CopyControl>();
                let (offset, delta) = *control.scroll.as_ref().unwrap().samples.last().unwrap();
                if frame <= 2 {
                    assert_eq!(offset, baseline);
                }
                if frame == 2 {
                    assert!(scrolled.mouse().just_pressed(MouseButton::Left));
                    assert_eq!(delta, bevy_egui::egui::Vec2::ZERO);
                }
                if frame == 3 {
                    assert_eq!(
                        offset,
                        baseline + 10.0,
                        "only declared drag displacement applies"
                    );
                    assert!(control.modifier_frames.last().unwrap().shift);
                }
                if frame == 4 {
                    assert!(control.modifier_frames.last().unwrap().shift);
                }
                if frame == 5 {
                    assert!(!control.modifier_frames.last().unwrap().shift);
                }
            }
            scrolled.app.update();
            assert!(reply.try_recv().unwrap().result.is_ok());
            assert!(!scrolled.mouse().pressed(MouseButton::Left));
        }

        // Whole-batch preflight still refuses a later clipboard chord before an earlier click.
        let clicks_before = h
            .app
            .world()
            .resource::<CopyControl>()
            .modifier_clicks
            .len();
        let mut reply = h.send(batch(
            vec![click(100.0, 50.0), key("ControlLeft"), key("KeyV")],
            0,
            false,
        ));
        h.app.update();
        assert_eq!(refusal_code(&mut reply), "clipboard_chord_refused");
        assert_eq!(
            h.app
                .world()
                .resource::<CopyControl>()
                .modifier_clicks
                .len(),
            clicks_before
        );
        assert!(h.keys().get_pressed().next().is_none());
        assert!(h.mouse().get_pressed().next().is_none());

        // A drag, then a scroll.
        let drag = InputCommand::PointerDrag {
            from: PointerPoint { x: 10.0, y: 10.0 },
            to: PointerPoint { x: 300.0, y: 200.0 },
            button: PointerButton::Left,
        };
        let scroll = InputCommand::Scroll { dx: 0.0, dy: -3.0 };
        let mut reply = h.send(batch(vec![drag, scroll], 0, false));
        h.app.update();
        assert!(!h.mouse().pressed(MouseButton::Left));
        assert_eq!(h.last_update_cursor(), Some(Vec2::new(10.0, 10.0)));
        h.app.update();
        assert!(h.mouse().just_pressed(MouseButton::Left));
        assert_eq!(h.last_update_cursor(), Some(Vec2::new(10.0, 10.0)));
        h.app.update();
        assert!(h.mouse().pressed(MouseButton::Left));
        assert_eq!(h.last_update_cursor(), Some(Vec2::new(300.0, 200.0)));
        h.app.update();
        assert!(h.mouse().just_released(MouseButton::Left));
        h.app.update();
        assert_eq!(
            h.app.world().resource::<AccumulatedMouseScroll>().delta,
            Vec2::new(0.0, -3.0)
        );
        h.app.update();
        assert!(reply.try_recv().expect("done").result.is_ok());

        // Single flight, then check at use: the user leaves for an uncovered
        // verse mid-batch — denied, and the held button is released.
        let mut first = h.send(batch(vec![click(20.0, 20.0)], 10, false));
        let mut second = h.send(batch(vec![click(30.0, 30.0)], 0, false));
        h.app.update();
        assert_eq!(refusal_code(&mut second), "input_in_progress");
        assert!(!h.mouse().pressed(MouseButton::Left));
        h.app.update();
        assert!(h.mouse().pressed(MouseButton::Left));
        h.set_verse("w");
        let copied_before_abort = h.app.world().resource::<CopyControl>().sink_text;
        h.app.world_mut().resource_mut::<CopyControl>().late_copy = true;
        h.app.update();
        assert_eq!(refusal_code(&mut first), DENIED_CODE);
        assert!(h.mouse().just_released(MouseButton::Left));
        assert!(!h.mouse().pressed(MouseButton::Left));
        assert!(h.idle());
        assert_eq!(
            h.app.world().resource::<CopyControl>().sink_text,
            copied_before_abort,
            "the abort frame cannot leak a late clipboard write"
        );
        assert_eq!(
            h.app.world().resource::<CopyControl>().sink_images,
            copied_before_abort
        );
        h.set_verse("v");

        // A posed capture takes the camera mid-batch: refused, button released.
        let mut reply = h.send(batch(vec![click(40.0, 40.0)], 0, false));
        h.app.update();
        h.app.update();
        assert!(h.mouse().pressed(MouseButton::Left));
        h.app.world_mut().insert_resource(CameraCaptureLock);
        h.app.update();
        assert_eq!(refusal_code(&mut reply), CAMERA_LOCKED_BY_CAPTURE_CODE);
        assert!(!h.mouse().pressed(MouseButton::Left));
        h.app.world_mut().remove_resource::<CameraCaptureLock>();

        // Wall-clock cap mid-injection: a typed partial failure, held input released.
        let mut reply = h.send(batch(vec![click(50.0, 50.0)], 100, false));
        h.app.update();
        h.app.update();
        assert!(h.mouse().pressed(MouseButton::Left));
        h.app
            .world_mut()
            .resource_mut::<Time<Real>>()
            .update_with_duration(INPUT_WALL_CLOCK_CAP);
        h.app.update();
        assert_eq!(refusal_code(&mut reply), INPUT_PARTIALLY_APPLIED_CODE);
        assert!(!h.mouse().pressed(MouseButton::Left));

        // Wall-clock cap after every frame landed (a stalled settle, QA-0034):
        // the click happened, so the reply is an unsettled success, not a failure.
        let mut reply = h.send(batch(vec![click(55.0, 55.0)], 100, false));
        for _ in 0..5 {
            h.app.update();
        }
        assert!(!h.mouse().pressed(MouseButton::Left), "press and release both landed");
        h.app
            .world_mut()
            .resource_mut::<Time<Real>>()
            .update_with_duration(INPUT_WALL_CLOCK_CAP);
        h.app.update();
        let output = reply
            .try_recv()
            .expect("the capped job replied")
            .result
            .expect("fully applied input is a success");
        assert!(!output.settled, "the settle outlived the cap");
        assert_eq!((output.commands_applied, output.png), (1, None));
        assert!(output.wait_frames_used < 100);
        assert!(h.idle());

        // capture_after: the reply carries exactly the PNG a direct capture encodes.
        let mut reply = h.send(batch(vec![click(60.0, 60.0)], 0, true));
        let shot = (0..10)
            .find_map(|_| {
                h.app.update();
                let world = h.app.world_mut();
                world
                    .query_filtered::<Entity, With<Screenshot>>()
                    .iter(world)
                    .next()
            })
            .expect("capture_after queues one primary-window Screenshot");
        let image = Image::new_fill(
            Extent3d {
                width: 4,
                height: 2,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[10, 20, 30, 255],
            TextureFormat::Rgba8UnormSrgb,
            bevy::asset::RenderAssetUsages::default(),
        );
        let direct = encode_png(image.clone()).expect("encodes");
        h.app.world_mut().trigger(ScreenshotCaptured {
            entity: shot,
            image,
        });
        let answer = (0..500)
            .find_map(|_| {
                h.app.update();
                std::thread::sleep(Duration::from_millis(2));
                reply.try_recv().ok()
            })
            .expect("replied once the PNG encoded");
        let output = answer.result.expect("applied and captured");
        assert_eq!(output.png, Some(direct));

        // QA-0080: abandoning a capture the renderer already holds
        // (`Capturing`) leaves its entity to Bevy — despawning it made
        // `trigger_screenshots` panic and abort the app. An unextracted one
        // is still despawned.
        let screenshots = |h: &mut Harness| -> Vec<Entity> {
            let world = h.app.world_mut();
            world
                .query_filtered::<Entity, With<Screenshot>>()
                .iter(world)
                .collect()
        };
        for in_flight in [true, false] {
            let before = screenshots(&mut h);
            let reply = h.send(batch(vec![click(65.0, 65.0)], 0, true));
            let shot = (0..10)
                .find_map(|_| {
                    h.app.update();
                    screenshots(&mut h).into_iter().find(|e| !before.contains(e))
                })
                .expect("capture_after queues one Screenshot");
            if in_flight {
                h.app.world_mut().entity_mut(shot).insert(Capturing);
            }
            drop(reply);
            h.app.update();
            assert!(h.idle());
            assert_eq!(
                h.app.world().get_entity(shot).is_ok(),
                in_flight,
                "only an in-flight capture outlives the abandoned job"
            );
        }

        // Abort during a modifier prefix releases it through the same production collector.
        let mut reply = h.send(batch(
            vec![key("ControlLeft"), click(100.0, 50.0)],
            0,
            false,
        ));
        h.app.update();
        assert!(
            h.app
                .world()
                .resource::<ModifierKeysState>()
                .ctrl
        );
        h.set_verse("w");
        h.app.update();
        assert_eq!(refusal_code(&mut reply), DENIED_CODE);
        assert!(h.keys().just_released(KeyCode::ControlLeft));
        assert!(
            !h.app
                .world()
                .resource::<ModifierKeysState>()
                .ctrl
        );
        assert!(h.app.world().resource::<SyntheticClipboardGuard>().0);
        assert!(h.idle());
        h.set_verse("v");

        // An abandoned caller: the batch stops and nothing stays pressed.
        let reply = h.send(batch(vec![click(70.0, 70.0)], 0, false));
        h.app.update();
        assert!(!h.mouse().pressed(MouseButton::Left));
        h.app.update();
        assert!(h.mouse().pressed(MouseButton::Left));
        drop(reply);
        h.app.update();
        assert!(h.idle());
        assert!(!h.mouse().pressed(MouseButton::Left));
        crate::native_accessibility::journey::extend_owned_input_journey();
    }
}
