use qwe_core::{LayoutRect, PaneId, PaneKind, PaneSpec, tile_pane_specs};
use qwe_protocol::{
    ATTACH_PROTOCOL_VERSION, ProtocolErrorBody,
    attach::{self, FrameType},
    verify_same_uid,
};
use serde_json::{Value, json};
use std::io::{IsTerminal, Read, Write};
use std::num::NonZeroU16;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;
use vt100::{Color, Parser};

use crate::{RawTerminal, attach_error, attach_socket, call, terminal_size};

const STATUS_ROWS: u16 = 1;
const COMPOSITOR_SCROLLBACK: usize = 2_000;
const RESIZE_POLL_INTERVAL: Duration = Duration::from_millis(200);

struct PaneView {
    id: PaneId,
    spec: PaneSpec,
    parser: Parser,
    rect: LayoutRect,
    active: bool,
    detail: String,
}

struct InputTarget {
    writer: Arc<Mutex<UnixStream>>,
    active: bool,
}

struct SharedInput {
    targets: Vec<InputTarget>,
    focused: usize,
    ui_mode: UiMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiMode {
    Normal,
    Menu,
    Copy,
    MoveFloating,
}

enum Event {
    Frame(PaneId, attach::ReceivedFrame),
    Focus(usize),
    Action(ComposeAction),
    CopyStart,
    Copy(CopyCommand),
    FloatingMoveStart,
    FloatingMove(FloatingMoveCommand),
    Mouse(MouseEvent),
    UiChanged,
    Viewport(u16, u16),
    ReaderClosed(PaneId),
    Detach,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposeAction {
    Split,
    Float,
    Close,
    MoveLeft,
    MoveRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyCommand {
    Up,
    Down,
    Left,
    Right,
    Start,
    SelectLine,
    SelectAll,
    ScrollUp,
    ScrollDown,
    Finish,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloatingMoveCommand {
    Left,
    Down,
    Up,
    Right,
    ShrinkWidth,
    GrowWidth,
    ShrinkHeight,
    GrowHeight,
    Finish,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CopyPoint {
    row: u16,
    column: u16,
}

struct CopyState {
    cursor: CopyPoint,
    anchor: Option<CopyPoint>,
    scrollback: usize,
}

struct FloatingMoveState {
    pane_id: PaneId,
    rect: LayoutRect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MouseEvent {
    kind: MouseEventKind,
    button: u8,
    x: u16,
    y: u16,
    raw: Vec<u8>,
}

enum MouseParse {
    Complete(MouseEvent, usize),
    Incomplete,
    NotMouse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MouseEventKind {
    Press,
    Motion,
    Release,
}

struct RenderUi<'a> {
    focused: usize,
    copy_state: Option<&'a CopyState>,
    menu_open: bool,
    moving: bool,
    notice: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellStyle {
    foreground: Color,
    background: Color,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl Default for CellStyle {
    fn default() -> Self {
        Self {
            foreground: Color::Default,
            background: Color::Default,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            inverse: false,
        }
    }
}

pub fn run(
    control_socket: &str,
    session_id: &str,
    owner: &str,
) -> Result<Value, ProtocolErrorBody> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(attach_error(
            "unsupported_terminal",
            "session compose requires an interactive terminal",
        ));
    }

    let (terminal_rows, terminal_columns) = terminal_size();
    let (layout_rows, pane_specs, initial_focus) =
        query_panes(control_socket, session_id, terminal_rows, terminal_columns)?;
    let layouts = tile_layout(&pane_specs, layout_rows, terminal_columns)?;
    let (events_tx, events_rx) = mpsc::channel();
    let mut views = Vec::with_capacity(pane_specs.len());
    let mut input_targets = Vec::with_capacity(pane_specs.len());

    for (spec, rect) in pane_specs.into_iter().zip(layouts.iter()) {
        let pane_id = spec.pane_id.clone();
        let (rows, columns) = pane_content_size(rect.rect);
        let (writer, snapshot) = attach_to_pane(
            control_socket,
            session_id,
            owner,
            &pane_id,
            rows,
            columns,
            events_tx.clone(),
        )?;
        let mut parser = Parser::new(
            NonZeroU16::new(rows).expect("Pane content rows are positive"),
            NonZeroU16::new(columns).expect("Pane content columns are positive"),
            COMPOSITOR_SCROLLBACK,
        );
        seed_parser(&mut parser, &snapshot);
        views.push(PaneView {
            id: pane_id,
            spec,
            parser,
            rect: rect.rect,
            active: true,
            detail: "attached".into(),
        });
        input_targets.push(InputTarget {
            writer: Arc::new(Mutex::new(writer)),
            active: true,
        });
    }

    let shared = Arc::new(Mutex::new(SharedInput {
        targets: input_targets,
        focused: initial_focus,
        ui_mode: UiMode::Normal,
    }));
    let stop = Arc::new(AtomicBool::new(false));
    let terminal = RawTerminal::enter(true)?;
    {
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(b"\x1b[?1002h\x1b[?1006h")
            .and_then(|()| stdout.flush())
            .map_err(|failure| attach_error("transport", failure.to_string()))?;
    }
    let mut copy_state = None;
    let mut floating_move_state = None;
    let mut mouse_drag_origin: Option<(CopyPoint, LayoutRect)> = None;
    let mut mouse_resize_origin: Option<(CopyPoint, LayoutRect)> = None;
    let mut mouse_forward_target: Option<(PaneId, bool)> = None;
    let mut notice = None;
    render(
        session_id,
        terminal_rows,
        terminal_columns,
        &views,
        &layouts,
        RenderUi {
            focused: initial_focus,
            copy_state: None,
            menu_open: false,
            moving: false,
            notice: None,
        },
    )
    .map_err(|failure| attach_error("transport", failure.to_string()))?;

    spawn_input(Arc::clone(&shared), Arc::clone(&stop), events_tx.clone());
    spawn_resize(
        (terminal_rows, terminal_columns),
        Arc::clone(&stop),
        events_tx.clone(),
    );

    let mut focused = initial_focus;
    let mut viewport = (terminal_rows, terminal_columns);
    let result = 'events: loop {
        let event = match events_rx.recv() {
            Ok(event) => event,
            Err(_) => break 'events Ok(Value::Null),
        };
        let mut should_render = false;
        let mut layout_changed = false;
        let mut current_event = Some(event);
        while let Some(event) = current_event.take() {
            match event {
                Event::Frame(pane_id, frame) => {
                    let Some(index) = views.iter().position(|view| view.id == pane_id) else {
                        continue;
                    };
                    match frame.frame_type {
                        FrameType::Output => {
                            let Ok((_, data)) = attach::decode_output(&frame.payload) else {
                                break 'events Err(attach_error(
                                    "worker_protocol_error",
                                    "OUTPUT frame has no stream offset",
                                ));
                            };
                            views[index].parser.process(data);
                            should_render = true;
                        }
                        FrameType::Snapshot => {
                            let Ok(snapshot) =
                                attach::decode_json::<attach::TerminalSnapshot>(&frame.payload)
                            else {
                                break 'events Err(attach_error(
                                    "worker_protocol_error",
                                    "invalid SNAPSHOT payload",
                                ));
                            };
                            seed_parser(&mut views[index].parser, &snapshot);
                            if copy_state.is_some() {
                                cancel_copy(
                                    &shared,
                                    &mut copy_state,
                                    &mut notice,
                                    "terminal state was replaced",
                                );
                            }
                            should_render = true;
                        }
                        FrameType::ProcessExited => {
                            let exited =
                                attach::decode_json::<attach::ProcessExited>(&frame.payload)
                                    .map_err(|failure| {
                                        attach_error("worker_protocol_error", failure.to_string())
                                    })?;
                            views[index].active = false;
                            views[index].detail = format!(
                                "exited code={} signal={}",
                                exited
                                    .exit_code
                                    .map_or_else(|| "none".to_string(), |code| code.to_string()),
                                exited.exit_signal.as_deref().unwrap_or("none")
                            );
                            if let Ok(mut shared) = shared.lock() {
                                shared.targets[index].active = false;
                            }
                            if copy_state.is_some() {
                                cancel_copy(
                                    &shared,
                                    &mut copy_state,
                                    &mut notice,
                                    "Pane process exited",
                                );
                            }
                            should_render = true;
                        }
                        FrameType::OwnershipRevoked => {
                            let revoked =
                                attach::decode_json::<attach::OwnershipRevoked>(&frame.payload);
                            break 'events Err(attach_error(
                                "not_owner",
                                revoked.map_or_else(
                                    |failure| failure.to_string(),
                                    |value| {
                                        format!("attachment authority revoked: {}", value.reason)
                                    },
                                ),
                            ));
                        }
                        FrameType::Error => {
                            let failure =
                                attach::decode_json::<attach::AttachError>(&frame.payload);
                            break 'events Err(failure.map_or_else(
                                |failure| {
                                    attach_error("worker_protocol_error", failure.to_string())
                                },
                                |failure| attach_error(&failure.code, failure.message),
                            ));
                        }
                        FrameType::AttachAccepted
                        | FrameType::Ack
                        | FrameType::Resized
                        | FrameType::Detach => {}
                        _ => {
                            break 'events Err(attach_error(
                                "worker_protocol_error",
                                format!("unexpected server frame {}", frame.frame_type.name()),
                            ));
                        }
                    }
                }
                Event::Focus(index) => {
                    focused = index;
                    let Some(view) = views.get(index) else {
                        break 'events Err(attach_error(
                            "internal",
                            "focus switched to an unknown Pane",
                        ));
                    };
                    if let Err(failure) = call(
                        control_socket,
                        "PANE_FOCUS",
                        json!({
                            "pane_id": view.id.to_string(),
                            "owner": owner
                        }),
                    ) {
                        break 'events Err(failure);
                    }
                    should_render = true;
                }
                Event::Action(action) => {
                    let result = match action {
                        ComposeAction::Split => split_focused_pane(
                            control_socket,
                            session_id,
                            owner,
                            &events_tx,
                            &mut views,
                            &shared,
                            focused,
                        ),
                        ComposeAction::Float => create_floating_pane(
                            control_socket,
                            session_id,
                            owner,
                            &events_tx,
                            &mut views,
                            &shared,
                            focused,
                        ),
                        ComposeAction::Close => {
                            close_focused_pane(control_socket, owner, &mut views, &shared, focused)
                        }
                        ComposeAction::MoveLeft => reorder_focused_pane(
                            control_socket,
                            owner,
                            &mut views,
                            &shared,
                            focused,
                            false,
                        ),
                        ComposeAction::MoveRight => reorder_focused_pane(
                            control_socket,
                            owner,
                            &mut views,
                            &shared,
                            focused,
                            true,
                        ),
                    };
                    match result {
                        Ok((next_focus, structural)) => {
                            focused = next_focus;
                            should_render = true;
                            layout_changed |= structural;
                        }
                        Err(failure) => break 'events Err(failure),
                    }
                }
                Event::CopyStart => {
                    let Some(view) = views.get_mut(focused) else {
                        break 'events Err(attach_error("internal", "focused Pane does not exist"));
                    };
                    let (row, column) = view.parser.screen().cursor_position();
                    copy_state = Some(CopyState {
                        cursor: CopyPoint { row, column },
                        anchor: None,
                        scrollback: 0,
                    });
                    notice = None;
                    should_render = true;
                }
                Event::Copy(command) => {
                    let Some(mut state) = copy_state.take() else {
                        continue;
                    };
                    let Some(view) = views.get_mut(focused) else {
                        break 'events Err(attach_error("internal", "focused Pane does not exist"));
                    };
                    let (rows, columns) = view.parser.screen().size();
                    let max_row = rows.get().saturating_sub(1);
                    let max_column = columns.get().saturating_sub(1);
                    match command {
                        CopyCommand::Up => state.cursor.row = state.cursor.row.saturating_sub(1),
                        CopyCommand::Down => {
                            state.cursor.row = state.cursor.row.saturating_add(1).min(max_row)
                        }
                        CopyCommand::Left => {
                            state.cursor.column = state.cursor.column.saturating_sub(1)
                        }
                        CopyCommand::Right => {
                            state.cursor.column =
                                state.cursor.column.saturating_add(1).min(max_column)
                        }
                        CopyCommand::Start => state.anchor = Some(state.cursor),
                        CopyCommand::SelectLine => {
                            state.anchor = Some(CopyPoint {
                                row: state.cursor.row,
                                column: 0,
                            });
                            state.cursor.column = max_column;
                        }
                        CopyCommand::SelectAll => {
                            state.anchor = Some(CopyPoint { row: 0, column: 0 });
                            state.cursor = CopyPoint {
                                row: max_row,
                                column: max_column,
                            };
                        }
                        CopyCommand::ScrollUp => {
                            let scrollback_len = view.parser.screen().scrollback_len();
                            state.scrollback =
                                state.scrollback.saturating_add(1).min(scrollback_len);
                            state.anchor = None;
                        }
                        CopyCommand::ScrollDown => {
                            state.scrollback = state.scrollback.saturating_sub(1);
                            state.anchor = None;
                        }
                        CopyCommand::Finish => {
                            let Some(anchor) = state.anchor else {
                                notice = Some("copy: select text before copying".into());
                                copy_state = Some(state);
                                should_render = true;
                                continue;
                            };
                            view.parser.screen_mut().set_scrollback(state.scrollback);
                            let text = copy_text(view, anchor, state.cursor);
                            if text.is_empty() {
                                notice = Some("copy: selection is empty".into());
                            } else {
                                notice = Some(match write_clipboard(&text) {
                                    Ok(provider) => {
                                        format!("copied {} bytes via {provider}", text.len())
                                    }
                                    Err(failure) => format!("copy failed: {failure}"),
                                });
                            }
                            copy_state = None;
                            should_render = true;
                            continue;
                        }
                        CopyCommand::Cancel => {
                            copy_state = None;
                            notice = Some("copy cancelled".into());
                            should_render = true;
                            continue;
                        }
                    }
                    copy_state = Some(state);
                    should_render = true;
                }
                Event::Mouse(mouse) => {
                    if mouse.x == 0 || mouse.y == 0 {
                        continue;
                    }
                    let left_button = mouse.button & 3 == 0 && mouse.button & 64 == 0;
                    let point = CopyPoint {
                        row: mouse.y - 1,
                        column: mouse.x - 1,
                    };
                    let ui_mode = shared
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .ui_mode;
                    if ui_mode == UiMode::Copy {
                        let Some(view) = views.get_mut(focused) else {
                            continue;
                        };
                        if point.row <= view.rect.y
                            || point.row >= view.rect.y.saturating_add(view.rect.rows)
                            || point.column < view.rect.x
                            || point.column >= view.rect.x.saturating_add(view.rect.columns)
                        {
                            continue;
                        }
                        let local = CopyPoint {
                            row: point.row - view.rect.y - 1,
                            column: point.column - view.rect.x,
                        };
                        let Some(mut state) = copy_state.take() else {
                            continue;
                        };
                        match mouse.kind {
                            MouseEventKind::Press => {
                                state.cursor = local;
                                state.anchor = Some(local);
                                copy_state = Some(state);
                            }
                            MouseEventKind::Motion => {
                                state.cursor = local;
                                copy_state = Some(state);
                            }
                            MouseEventKind::Release => {
                                state.cursor = local;
                                view.parser.screen_mut().set_scrollback(state.scrollback);
                                let text = copy_text(view, state.anchor.unwrap_or(local), local);
                                notice = Some(if text.is_empty() {
                                    "copy: selection is empty".into()
                                } else {
                                    match write_clipboard(&text) {
                                        Ok(provider) => {
                                            format!("copied {} bytes via {provider}", text.len())
                                        }
                                        Err(failure) => format!("copy failed: {failure}"),
                                    }
                                });
                                shared
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .ui_mode = UiMode::Normal;
                                let _ = events_tx.send(Event::UiChanged);
                                copy_state = None;
                            }
                        }
                        should_render = true;
                        continue;
                    }
                    let Some(mut index) = pane_at_point(&views, point) else {
                        continue;
                    };
                    if matches!(mouse.kind, MouseEventKind::Press)
                        && left_button
                        && views[index].spec.kind == PaneKind::Floating
                    {
                        index =
                            raise_floating_view(control_socket, owner, &mut views, &shared, index)?;
                        notice = Some("floating Pane raised".into());
                    }
                    let view = &views[index];
                    let on_floating_title =
                        view.spec.kind == PaneKind::Floating && point.row == view.rect.y;
                    let on_resize_handle = view.spec.kind == PaneKind::Floating
                        && point.row
                            == view.rect.y.saturating_add(view.rect.rows.saturating_sub(1))
                        && point.column
                            == view
                                .rect
                                .x
                                .saturating_add(view.rect.columns.saturating_sub(1));
                    match mouse.kind {
                        MouseEventKind::Press => {
                            if left_button {
                                focus_view(control_socket, owner, &views, &shared, index)?;
                                focused = index;
                            }
                            mouse_forward_target = None;
                            if left_button && on_floating_title {
                                floating_move_state = Some(FloatingMoveState {
                                    pane_id: view.id.clone(),
                                    rect: view.spec.floating_rect.unwrap_or(view.rect),
                                });
                                mouse_drag_origin = floating_move_state
                                    .as_ref()
                                    .map(|state| (point, state.rect));
                            } else if left_button && on_resize_handle {
                                floating_move_state = Some(FloatingMoveState {
                                    pane_id: view.id.clone(),
                                    rect: view.spec.floating_rect.unwrap_or(view.rect),
                                });
                                mouse_resize_origin = floating_move_state
                                    .as_ref()
                                    .map(|state| (point, state.rect));
                            } else {
                                let mouse_enabled = view.parser.screen().mouse_protocol_mode()
                                    != vt100::MouseProtocolMode::None;
                                mouse_forward_target = Some((view.id.clone(), mouse_enabled));
                                if mouse_enabled {
                                    forward_mouse_event(&shared, index, &mouse.raw)?;
                                }
                            }
                            should_render = true;
                        }
                        MouseEventKind::Motion => {
                            if let Some((origin, start_rect)) = mouse_resize_origin
                                && let Some(state) = floating_move_state.as_mut()
                            {
                                let delta_rows = i32::from(point.row) - i32::from(origin.row);
                                let delta_columns =
                                    i32::from(point.column) - i32::from(origin.column);
                                state.rect.rows = u16::try_from(
                                    i32::from(start_rect.rows).saturating_add(delta_rows).max(1),
                                )
                                .unwrap_or(u16::MAX);
                                state.rect.columns = u16::try_from(
                                    i32::from(start_rect.columns)
                                        .saturating_add(delta_columns)
                                        .max(1),
                                )
                                .unwrap_or(u16::MAX);
                                state.rect = clamp_floating_draft(
                                    state.rect,
                                    viewport.0.saturating_sub(STATUS_ROWS).max(1),
                                    viewport.1,
                                );
                                layout_changed = true;
                                should_render = true;
                            } else if let Some((origin, start_rect)) = mouse_drag_origin
                                && let Some(state) = floating_move_state.as_mut()
                            {
                                let delta_x = i32::from(point.column) - i32::from(origin.column);
                                let delta_y = i32::from(point.row) - i32::from(origin.row);
                                let x =
                                    i32::from(start_rect.x).saturating_add(delta_x).max(0) as u16;
                                let y =
                                    i32::from(start_rect.y).saturating_add(delta_y).max(0) as u16;
                                state.rect.x = x;
                                state.rect.y = y;
                                state.rect = clamp_floating_draft(
                                    state.rect,
                                    viewport.0.saturating_sub(STATUS_ROWS).max(1),
                                    viewport.1,
                                );
                                layout_changed = true;
                                should_render = true;
                            } else if let Some((pane_id, true)) = &mouse_forward_target
                                && let Some(index) =
                                    views.iter().position(|view| view.id == *pane_id)
                            {
                                forward_mouse_event(&shared, index, &mouse.raw)?;
                            }
                        }
                        MouseEventKind::Release => {
                            let resizing = mouse_resize_origin.take().is_some();
                            let dragging = mouse_drag_origin.take().is_some();
                            if dragging || resizing {
                                if let Some(state) = floating_move_state.take() {
                                    let updated = call(
                                        control_socket,
                                        "PANE_FLOATING_UPDATE",
                                        json!({
                                            "pane_id": state.pane_id.to_string(),
                                            "rect": state.rect,
                                            "owner": owner
                                        }),
                                    );
                                    match updated {
                                        Ok(_) => {
                                            if let Some(view) = views
                                                .iter_mut()
                                                .find(|view| view.id == state.pane_id)
                                            {
                                                view.spec.floating_rect = Some(state.rect);
                                                view.rect = state.rect;
                                            }
                                            notice = Some(if resizing {
                                                "floating bounds resized by mouse".into()
                                            } else {
                                                "floating bounds saved by mouse".into()
                                            });
                                            layout_changed = true;
                                        }
                                        Err(failure) => break 'events Err(failure),
                                    }
                                }
                            } else if let Some((pane_id, _)) = mouse_forward_target.take()
                                && let Some(index) =
                                    views.iter().position(|view| view.id == pane_id)
                                && views[index].parser.screen().mouse_protocol_mode()
                                    != vt100::MouseProtocolMode::None
                            {
                                forward_mouse_event(&shared, index, &mouse.raw)?;
                            }
                            should_render = true;
                        }
                    }
                }
                Event::FloatingMoveStart => {
                    let Some(view) = views.get(focused) else {
                        break 'events Err(attach_error("internal", "focused Pane does not exist"));
                    };
                    if view.spec.kind != PaneKind::Floating {
                        notice = Some("floating move requires a floating Pane".into());
                        if let Ok(mut shared) = shared.lock() {
                            shared.ui_mode = UiMode::Normal;
                        }
                        let _ = events_tx.send(Event::UiChanged);
                    } else {
                        floating_move_state = Some(FloatingMoveState {
                            pane_id: view.id.clone(),
                            rect: view.spec.floating_rect.unwrap_or(view.rect),
                        });
                        notice = None;
                    }
                    should_render = true;
                }
                Event::FloatingMove(command) => {
                    let Some(mut state) = floating_move_state.take() else {
                        continue;
                    };
                    match command {
                        FloatingMoveCommand::Left => state.rect.x = state.rect.x.saturating_sub(1),
                        FloatingMoveCommand::Down => state.rect.y = state.rect.y.saturating_add(1),
                        FloatingMoveCommand::Up => state.rect.y = state.rect.y.saturating_sub(1),
                        FloatingMoveCommand::Right => state.rect.x = state.rect.x.saturating_add(1),
                        FloatingMoveCommand::ShrinkWidth => {
                            state.rect.columns = state.rect.columns.saturating_sub(1).max(1)
                        }
                        FloatingMoveCommand::GrowWidth => {
                            state.rect.columns = state.rect.columns.saturating_add(1)
                        }
                        FloatingMoveCommand::ShrinkHeight => {
                            state.rect.rows = state.rect.rows.saturating_sub(1).max(1)
                        }
                        FloatingMoveCommand::GrowHeight => {
                            state.rect.rows = state.rect.rows.saturating_add(1)
                        }
                        FloatingMoveCommand::Finish => {
                            let updated = call(
                                control_socket,
                                "PANE_FLOATING_UPDATE",
                                json!({
                                    "pane_id": state.pane_id.to_string(),
                                    "rect": state.rect,
                                    "owner": owner
                                }),
                            );
                            match updated {
                                Ok(_) => {
                                    if let Some(view) =
                                        views.iter_mut().find(|view| view.id == state.pane_id)
                                    {
                                        view.spec.floating_rect = Some(state.rect);
                                        view.rect = state.rect;
                                    }
                                    notice = Some("floating bounds saved".into());
                                    layout_changed = true;
                                }
                                Err(failure) => break 'events Err(failure),
                            }
                            should_render = true;
                            continue;
                        }
                        FloatingMoveCommand::Cancel => {
                            notice = Some("floating move cancelled".into());
                            should_render = true;
                            continue;
                        }
                    }
                    state.rect = clamp_floating_draft(
                        state.rect,
                        viewport.0.saturating_sub(STATUS_ROWS).max(1),
                        viewport.1,
                    );
                    floating_move_state = Some(state);
                    layout_changed = true;
                    should_render = true;
                }
                Event::UiChanged => {
                    should_render = true;
                }
                Event::Viewport(rows, columns) => {
                    viewport = (rows, columns);
                    if floating_move_state.is_some() {
                        floating_move_state = None;
                        notice = Some("floating move cancelled after resize".into());
                        if let Ok(mut shared) = shared.lock() {
                            shared.ui_mode = UiMode::Normal;
                        }
                    }
                    if copy_state.is_some() {
                        cancel_copy(
                            &shared,
                            &mut copy_state,
                            &mut notice,
                            "copy cancelled after resize",
                        );
                    }
                    should_render = true;
                    layout_changed = true;
                }
                Event::ReaderClosed(pane_id) => {
                    if let Some(view) = views.iter_mut().find(|view| view.id == pane_id) {
                        view.active = false;
                        view.detail = "attach closed".into();
                    }
                    if copy_state.is_some() {
                        cancel_copy(&shared, &mut copy_state, &mut notice, "Pane Attach closed");
                    }
                    should_render = true;
                }
                Event::Detach => break 'events Ok(Value::Null),
            }
            current_event = events_rx.try_recv().ok();
        }
        if should_render {
            let mut layouts = if layout_changed {
                refresh_layout(&mut views, viewport, &shared)?
            } else {
                let pane_rows = viewport.0.saturating_sub(STATUS_ROWS).max(1);
                tile_layout(&pane_specs_from_views(&views), pane_rows, viewport.1)?
            };
            for view in &mut views {
                view.parser.screen_mut().set_scrollback(0);
            }
            if let Some(copy) = copy_state.as_ref()
                && let Some(view) = views.get_mut(focused)
            {
                view.parser.screen_mut().set_scrollback(copy.scrollback);
            }
            if let Some(state) = floating_move_state.as_ref() {
                apply_floating_draft(&mut views, &mut layouts, &shared, state)?;
            }
            let ui_mode = shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .ui_mode;
            if let Err(failure) = render(
                session_id,
                viewport.0,
                viewport.1,
                &views,
                &layouts,
                RenderUi {
                    focused,
                    copy_state: copy_state.as_ref(),
                    menu_open: ui_mode == UiMode::Menu,
                    moving: ui_mode == UiMode::MoveFloating,
                    notice: notice.as_deref(),
                },
            ) {
                break 'events Err(attach_error("transport", failure.to_string()));
            }
        }
    };

    stop.store(true, Ordering::SeqCst);
    detach_all(&shared);
    terminal.reset_modes();
    drop(terminal);
    result
}

fn query_panes(
    control_socket: &str,
    session_id: &str,
    rows: u16,
    columns: u16,
) -> Result<(u16, Vec<PaneSpec>, usize), ProtocolErrorBody> {
    let usable_rows = rows.saturating_sub(STATUS_ROWS).max(1);
    let layout = call(
        control_socket,
        "SESSION_LAYOUT",
        json!({
            "session_id": session_id,
            "rows": usable_rows,
            "columns": columns
        }),
    )?;
    let panes = layout
        .get("panes")
        .and_then(Value::as_array)
        .ok_or_else(|| attach_error("contract", "SESSION_LAYOUT response has no panes"))?;
    let pane_specs = panes
        .iter()
        .map(|pane| {
            let id = pane
                .get("pane_id")
                .and_then(Value::as_str)
                .ok_or_else(|| attach_error("contract", "layout Pane has no pane_id"))?;
            let pane_id = PaneId::from_str(id)
                .map_err(|failure| attach_error("contract", failure.to_string()))?;
            let kind = pane
                .get("kind")
                .and_then(Value::as_str)
                .ok_or_else(|| attach_error("contract", "layout Pane has no kind"))?
                .parse::<PaneKind>()
                .map_err(|failure| attach_error("contract", failure))?;
            let layout_order = pane
                .get("layout_order")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| attach_error("contract", "layout Pane has no layout_order"))?;
            let layout_weight = pane
                .get("layout_weight")
                .and_then(Value::as_u64)
                .and_then(|value| u16::try_from(value).ok())
                .ok_or_else(|| attach_error("contract", "layout Pane has no layout_weight"))?;
            let z_index = pane
                .get("z_index")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| attach_error("contract", "layout Pane has no z_index"))?;
            Ok(PaneSpec {
                pane_id,
                kind,
                layout_order,
                layout_weight,
                z_index,
                floating_rect: pane
                    .get("floating_rect")
                    .filter(|value| !value.is_null())
                    .cloned()
                    .map(|value| parse_layout_rect_value(&value))
                    .transpose()?,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if pane_specs.is_empty() {
        return Err(attach_error("session_lost", "Session has no live Panes"));
    }
    let focused = panes
        .iter()
        .position(|pane| pane.get("focused").and_then(Value::as_bool) == Some(true))
        .unwrap_or_default();
    Ok((usable_rows, pane_specs, focused))
}

fn tile_layout(
    pane_specs: &[PaneSpec],
    rows: u16,
    columns: u16,
) -> Result<Vec<qwe_core::PaneLayout>, ProtocolErrorBody> {
    tile_pane_specs(
        pane_specs,
        rows,
        columns,
        pane_specs.first().map(|pane| &pane.pane_id),
    )
    .map_err(|failure| attach_error("invalid_layout", failure.to_string()))
}

fn pane_spec_from_created(value: &Value, pane_id: PaneId) -> Result<PaneSpec, ProtocolErrorBody> {
    let kind = value
        .get("pane_kind")
        .and_then(Value::as_str)
        .ok_or_else(|| attach_error("contract", "Pane creation response has no pane_kind"))?
        .parse::<PaneKind>()
        .map_err(|failure| attach_error("contract", failure))?;
    let layout_order = value
        .get("layout_order")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| attach_error("contract", "Pane creation response has no layout_order"))?;
    let layout_weight = value
        .get("layout_weight")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| attach_error("contract", "Pane creation response has no layout_weight"))?;
    let z_index = value
        .get("z_index")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| attach_error("contract", "Pane creation response has no z_index"))?;
    Ok(PaneSpec {
        pane_id,
        kind,
        layout_order,
        layout_weight,
        z_index,
        floating_rect: value
            .get("floating_rect")
            .filter(|value| !value.is_null())
            .cloned()
            .map(|value| parse_layout_rect_value(&value))
            .transpose()?,
    })
}

fn parse_layout_rect_value(value: &Value) -> Result<LayoutRect, ProtocolErrorBody> {
    serde_json::from_value(value.clone())
        .map_err(|failure| attach_error("contract", failure.to_string()))
}

fn refresh_layout(
    views: &mut [PaneView],
    viewport: (u16, u16),
    shared: &Arc<Mutex<SharedInput>>,
) -> Result<Vec<qwe_core::PaneLayout>, ProtocolErrorBody> {
    let pane_rows = viewport.0.saturating_sub(STATUS_ROWS).max(1);
    let layouts = tile_layout(&pane_specs_from_views(views), pane_rows, viewport.1)?;
    let shared = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (index, (view, layout)) in views.iter_mut().zip(layouts.iter()).enumerate() {
        view.rect = layout.rect;
        let (rows, columns) = pane_content_size(layout.rect);
        view.parser.set_size(
            NonZeroU16::new(rows).expect("Pane content rows are positive"),
            NonZeroU16::new(columns).expect("Pane content columns are positive"),
        );
        if let Some(target) = shared.targets.get(index)
            && target.active
        {
            send_resize(&target.writer, rows, columns)?;
        }
    }
    Ok(layouts)
}

fn clamp_floating_draft(rect: LayoutRect, rows: u16, columns: u16) -> LayoutRect {
    let rect_rows = rect.rows.clamp(1, rows);
    let rect_columns = rect.columns.clamp(1, columns);
    LayoutRect {
        x: rect.x.min(columns - rect_columns),
        y: rect.y.min(rows - rect_rows),
        rows: rect_rows,
        columns: rect_columns,
    }
}

fn apply_floating_draft(
    views: &mut [PaneView],
    layouts: &mut [qwe_core::PaneLayout],
    shared: &Arc<Mutex<SharedInput>>,
    state: &FloatingMoveState,
) -> Result<(), ProtocolErrorBody> {
    let Some(index) = views.iter().position(|view| view.id == state.pane_id) else {
        return Ok(());
    };
    views[index].rect = state.rect;
    layouts[index].rect = state.rect;
    let (rows, columns) = pane_content_size(state.rect);
    views[index].parser.set_size(
        NonZeroU16::new(rows).expect("Pane content rows are positive"),
        NonZeroU16::new(columns).expect("Pane content columns are positive"),
    );
    if let Some(target) = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .targets
        .get(index)
        && target.active
    {
        send_resize(&target.writer, rows, columns)?;
    }
    Ok(())
}

fn split_focused_pane(
    control_socket: &str,
    session_id: &str,
    owner: &str,
    events: &mpsc::Sender<Event>,
    views: &mut Vec<PaneView>,
    shared: &Arc<Mutex<SharedInput>>,
    focused: usize,
) -> Result<(usize, bool), ProtocolErrorBody> {
    let source = views
        .get(focused)
        .ok_or_else(|| attach_error("internal", "focused Pane does not exist"))?;
    let mut create_payload = json!({
        "session_id": session_id,
        "program": "sh",
        "args": [],
        "pane_kind": "normal",
        "owner": owner
    });
    if source.spec.kind == PaneKind::Normal {
        create_payload["after_pane_id"] = json!(source.id.to_string());
    }
    let created = call(control_socket, "PANE_CREATE", create_payload)?;
    let pane_id = created
        .get("pane_id")
        .and_then(Value::as_str)
        .ok_or_else(|| attach_error("contract", "Pane creation response has no pane_id"))?;
    let pane_id = PaneId::from_str(pane_id)
        .map_err(|failure| attach_error("contract", failure.to_string()))?;
    let pane_key = pane_id.to_string();
    let spec = pane_spec_from_created(&created, pane_id.clone())?;
    let (rows, columns) = pane_content_size(source.rect);
    let attached = attach_to_pane(
        control_socket,
        session_id,
        owner,
        &pane_id,
        rows,
        columns,
        events.clone(),
    );
    let (writer, snapshot) = match attached {
        Ok(attached) => attached,
        Err(failure) => {
            let _ = call(
                control_socket,
                "PANE_CLOSE",
                json!({"pane_id": pane_key.clone(), "owner": owner}),
            );
            return Err(failure);
        }
    };
    let mut parser = Parser::new(
        NonZeroU16::new(rows).expect("Pane content rows are positive"),
        NonZeroU16::new(columns).expect("Pane content columns are positive"),
        COMPOSITOR_SCROLLBACK,
    );
    seed_parser(&mut parser, &snapshot);
    let insertion = if source.spec.kind == PaneKind::Normal {
        focused + 1
    } else {
        views
            .iter()
            .position(|view| view.spec.kind == PaneKind::Floating)
            .unwrap_or(views.len())
    };
    views.insert(
        insertion,
        PaneView {
            id: pane_id,
            spec,
            parser,
            rect: source.rect,
            active: true,
            detail: "attached".into(),
        },
    );
    {
        let mut shared = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        shared.targets.insert(
            insertion,
            InputTarget {
                writer: Arc::new(Mutex::new(writer)),
                active: true,
            },
        );
        shared.focused = insertion;
    }
    call(
        control_socket,
        "PANE_FOCUS",
        json!({"pane_id": pane_key.clone(), "owner": owner}),
    )?;
    Ok((insertion, true))
}

fn create_floating_pane(
    control_socket: &str,
    session_id: &str,
    owner: &str,
    events: &mpsc::Sender<Event>,
    views: &mut Vec<PaneView>,
    shared: &Arc<Mutex<SharedInput>>,
    focused: usize,
) -> Result<(usize, bool), ProtocolErrorBody> {
    let (rows, columns) = views
        .get(focused)
        .map(|view| pane_content_size(view.rect))
        .ok_or_else(|| attach_error("internal", "focused Pane does not exist"))?;
    let created = call(
        control_socket,
        "PANE_CREATE",
        json!({
            "session_id": session_id,
            "program": "sh",
            "args": [],
            "pane_kind": "floating",
            "owner": owner
        }),
    )?;
    let pane_id = created
        .get("pane_id")
        .and_then(Value::as_str)
        .ok_or_else(|| attach_error("contract", "Pane creation response has no pane_id"))?;
    let pane_id = PaneId::from_str(pane_id)
        .map_err(|failure| attach_error("contract", failure.to_string()))?;
    let pane_key = pane_id.to_string();
    let spec = pane_spec_from_created(&created, pane_id.clone())?;
    let attached = attach_to_pane(
        control_socket,
        session_id,
        owner,
        &pane_id,
        rows,
        columns,
        events.clone(),
    );
    let (writer, snapshot) = match attached {
        Ok(attached) => attached,
        Err(failure) => {
            let _ = call(
                control_socket,
                "PANE_CLOSE",
                json!({"pane_id": pane_key.clone(), "owner": owner}),
            );
            return Err(failure);
        }
    };
    let mut parser = Parser::new(
        NonZeroU16::new(rows).expect("Pane content rows are positive"),
        NonZeroU16::new(columns).expect("Pane content columns are positive"),
        COMPOSITOR_SCROLLBACK,
    );
    seed_parser(&mut parser, &snapshot);
    let insertion = views.len();
    views.push(PaneView {
        id: pane_id,
        spec,
        parser,
        rect: LayoutRect {
            x: 0,
            y: 0,
            rows,
            columns,
        },
        active: true,
        detail: "floating".into(),
    });
    {
        let mut shared = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        shared.targets.push(InputTarget {
            writer: Arc::new(Mutex::new(writer)),
            active: true,
        });
        shared.focused = insertion;
    }
    call(
        control_socket,
        "PANE_FOCUS",
        json!({"pane_id": pane_key, "owner": owner}),
    )?;
    Ok((insertion, true))
}

fn close_focused_pane(
    control_socket: &str,
    owner: &str,
    views: &mut Vec<PaneView>,
    shared: &Arc<Mutex<SharedInput>>,
    focused: usize,
) -> Result<(usize, bool), ProtocolErrorBody> {
    if views.len() <= 1 {
        return Err(attach_error(
            "invalid_operation",
            "the last Pane must be closed with the Session",
        ));
    }
    let pane_id = views
        .get(focused)
        .ok_or_else(|| attach_error("internal", "focused Pane does not exist"))?
        .id
        .clone();
    call(
        control_socket,
        "PANE_FOCUS",
        json!({"pane_id": pane_id.to_string(), "owner": owner}),
    )?;
    call(
        control_socket,
        "PANE_CLOSE",
        json!({"pane_id": pane_id.to_string(), "owner": owner}),
    )?;
    remove_input_target(shared, focused);
    views.remove(focused);
    let next_focus = focused.min(views.len() - 1);
    if let Ok(mut shared) = shared.lock() {
        shared.focused = next_focus;
    }
    if let Some(view) = views.get(next_focus) {
        call(
            control_socket,
            "PANE_FOCUS",
            json!({"pane_id": view.id.to_string(), "owner": owner}),
        )?;
    }
    Ok((next_focus, true))
}

fn reorder_focused_pane(
    control_socket: &str,
    owner: &str,
    views: &mut [PaneView],
    shared: &Arc<Mutex<SharedInput>>,
    focused: usize,
    move_right: bool,
) -> Result<(usize, bool), ProtocolErrorBody> {
    if views
        .get(focused)
        .is_none_or(|view| view.spec.kind != PaneKind::Normal)
    {
        return Ok((focused, false));
    }
    let tiled = views
        .iter()
        .enumerate()
        .filter(|(_, view)| view.spec.kind == PaneKind::Normal)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let Some(position) = tiled.iter().position(|index| *index == focused) else {
        return Ok((focused, false));
    };
    let Some(destination_position) = (if move_right {
        position.checked_add(1)
    } else {
        position.checked_sub(1)
    }) else {
        return Ok((focused, false));
    };
    if destination_position >= tiled.len() {
        return Ok((focused, false));
    }
    let mut pane_ids = tiled
        .iter()
        .map(|index| views[*index].id.clone())
        .collect::<Vec<_>>();
    pane_ids.swap(position, destination_position);
    call(
        control_socket,
        "PANE_REORDER",
        json!({
            "pane_ids": pane_ids,
            "owner": owner
        }),
    )?;
    let destination = tiled[destination_position];
    views.swap(focused, destination);
    if let Ok(mut shared) = shared.lock() {
        shared.targets.swap(focused, destination);
        shared.focused = destination;
    }
    Ok((destination, true))
}

fn remove_input_target(shared: &Arc<Mutex<SharedInput>>, index: usize) {
    let Ok(mut shared) = shared.lock() else {
        return;
    };
    if let Some(target) = shared.targets.get(index) {
        if let Ok(writer) = target.writer.lock() {
            let _ = writer.shutdown(std::net::Shutdown::Both);
        }
    }
    if index < shared.targets.len() {
        shared.targets.remove(index);
    }
}

fn pane_specs_from_views(views: &[PaneView]) -> Vec<PaneSpec> {
    views.iter().map(|view| view.spec.clone()).collect()
}

fn pane_at_point(views: &[PaneView], point: CopyPoint) -> Option<usize> {
    views.iter().enumerate().rev().find_map(|(index, view)| {
        let rect = view.rect;
        (point.row >= rect.y
            && point.row < rect.y.saturating_add(rect.rows)
            && point.column >= rect.x
            && point.column < rect.x.saturating_add(rect.columns))
        .then_some(index)
    })
}

fn focus_view(
    control_socket: &str,
    owner: &str,
    views: &[PaneView],
    shared: &Arc<Mutex<SharedInput>>,
    index: usize,
) -> Result<(), ProtocolErrorBody> {
    let view = views
        .get(index)
        .ok_or_else(|| attach_error("internal", "focus Pane does not exist"))?;
    if let Ok(mut shared) = shared.lock() {
        shared.focused = index;
    }
    call(
        control_socket,
        "PANE_FOCUS",
        json!({"pane_id": view.id.to_string(), "owner": owner}),
    )?;
    Ok(())
}

fn raise_floating_view(
    control_socket: &str,
    owner: &str,
    views: &mut Vec<PaneView>,
    shared: &Arc<Mutex<SharedInput>>,
    index: usize,
) -> Result<usize, ProtocolErrorBody> {
    let pane_id = views
        .get(index)
        .ok_or_else(|| attach_error("internal", "raised Pane does not exist"))?
        .id
        .clone();
    let raised = call(
        control_socket,
        "PANE_FLOATING_RAISE",
        json!({"pane_id": pane_id.to_string(), "owner": owner}),
    )?;
    let z_index = raised
        .get("z_index")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| attach_error("contract", "raise response has no z_index"))?;
    let view = views
        .get_mut(index)
        .ok_or_else(|| attach_error("internal", "raised Pane disappeared"))?;
    view.spec.z_index = z_index;
    sort_views_and_targets(views, shared, &pane_id)
}

fn sort_views_and_targets(
    views: &mut Vec<PaneView>,
    shared: &Arc<Mutex<SharedInput>>,
    focus_id: &PaneId,
) -> Result<usize, ProtocolErrorBody> {
    let mut order = (0..views.len()).collect::<Vec<_>>();
    order.sort_by_key(|index| {
        let view = &views[*index];
        match view.spec.kind {
            PaneKind::Normal => (0_u8, 0_u32, view.spec.layout_order, view.id.to_string()),
            PaneKind::Floating => (
                1_u8,
                view.spec.z_index,
                view.spec.layout_order,
                view.id.to_string(),
            ),
        }
    });
    let original_views = std::mem::take(views)
        .into_iter()
        .map(Some)
        .collect::<Vec<_>>();
    let mut mut_shared = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let original_targets = std::mem::take(&mut mut_shared.targets)
        .into_iter()
        .map(Some)
        .collect::<Vec<_>>();
    let mut target_slots = original_targets;
    let mut view_slots = original_views;
    for index in order {
        if let Some(view) = view_slots[index].take() {
            views.push(view);
        }
        if let Some(target) = target_slots[index].take() {
            mut_shared.targets.push(target);
        }
    }
    let focused = views
        .iter()
        .position(|view| view.id == *focus_id)
        .ok_or_else(|| attach_error("internal", "focused Pane disappeared during sort"))?;
    mut_shared.focused = focused;
    Ok(focused)
}

fn forward_mouse_event(
    shared: &Arc<Mutex<SharedInput>>,
    index: usize,
    raw: &[u8],
) -> Result<(), ProtocolErrorBody> {
    let shared = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let target = shared
        .targets
        .get(index)
        .ok_or_else(|| attach_error("internal", "mouse target is unavailable"))?;
    let mut writer = target
        .writer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    attach::write_payload_frame(&mut *writer, FrameType::Input, raw)
        .map_err(|failure| attach_error("transport", failure.to_string()))
}

fn pane_content_size(rect: LayoutRect) -> (u16, u16) {
    (rect.rows.saturating_sub(1).max(1), rect.columns.max(1))
}

fn attach_to_pane(
    control_socket: &str,
    session_id: &str,
    owner: &str,
    pane_id: &PaneId,
    rows: u16,
    columns: u16,
    events: mpsc::Sender<Event>,
) -> Result<(UnixStream, attach::TerminalSnapshot), ProtocolErrorBody> {
    let attach_path = attach_socket(control_socket);
    let reader = UnixStream::connect(&attach_path)
        .map_err(|failure| attach_error("transport", format!("{attach_path}: {failure}")))?;
    verify_same_uid(&reader)
        .map_err(|failure| attach_error("unauthorized_peer", failure.to_string()))?;
    let open = attach::AttachOpen::versioned(session_id, owner, rows, columns)
        .with_pane(pane_id.to_string());
    attach::write_json_frame(
        &mut reader
            .try_clone()
            .map_err(|failure| attach_error("transport", failure.to_string()))?,
        FrameType::AttachOpen,
        &open,
    )
    .map_err(|failure| attach_error("transport", failure.to_string()))?;

    let mut reader = reader;
    loop {
        let frame = attach::read_frame(&mut reader)
            .map_err(|failure| attach_error("transport", failure.to_string()))?;
        match frame.frame_type {
            FrameType::AttachAccepted => {}
            FrameType::Snapshot => {
                let snapshot = attach::decode_json::<attach::TerminalSnapshot>(&frame.payload)
                    .map_err(|failure| {
                        attach_error("worker_protocol_error", failure.to_string())
                    })?;
                let writer = reader
                    .try_clone()
                    .map_err(|failure| attach_error("transport", failure.to_string()))?;
                let pane_id = pane_id.clone();
                thread::spawn(move || {
                    let mut reader = reader;
                    while let Ok(frame) = attach::read_frame(&mut reader) {
                        if events.send(Event::Frame(pane_id.clone(), frame)).is_err() {
                            return;
                        }
                    }
                    let _ = events.send(Event::ReaderClosed(pane_id));
                });
                return Ok((writer, snapshot));
            }
            FrameType::Error => {
                let failure = attach::decode_json::<attach::AttachError>(&frame.payload).map_err(
                    |failure| attach_error("worker_protocol_error", failure.to_string()),
                )?;
                return Err(attach_error(&failure.code, failure.message));
            }
            _ => {
                return Err(attach_error(
                    "worker_protocol_error",
                    format!(
                        "expected ATTACH_ACCEPTED and SNAPSHOT, received {}",
                        frame.frame_type.name()
                    ),
                ));
            }
        }
    }
}

fn seed_parser(parser: &mut Parser, snapshot: &attach::TerminalSnapshot) {
    parser.process(b"\x1b[0m\x1b[2J\x1b[H");
    for (row, line) in snapshot
        .screen_text
        .split('\n')
        .take(usize::from(snapshot.rows))
        .enumerate()
    {
        parser.process(format!("\x1b[{};1H", row + 1).as_bytes());
        parser.process(line.as_bytes());
    }
    parser.process(
        format!(
            "\x1b[{};{}H",
            u32::from(snapshot.cursor_row) + 1,
            u32::from(snapshot.cursor_column) + 1
        )
        .as_bytes(),
    );
    parser.process(if snapshot.cursor_visible {
        b"\x1b[?25h".as_slice()
    } else {
        b"\x1b[?25l".as_slice()
    });
}

fn parse_sgr_mouse(data: &[u8], start: usize) -> MouseParse {
    if !data[start..].starts_with(b"\x1b[<") {
        return MouseParse::NotMouse;
    }
    let Some(end_offset) = data[start + 3..]
        .iter()
        .position(|byte| matches!(byte, b'M' | b'm'))
    else {
        return MouseParse::Incomplete;
    };
    let end = start + 3 + end_offset;
    let body = &data[start + 3..end];
    let parts = body.split(|byte| *byte == b';').collect::<Vec<_>>();
    if parts.len() != 3 {
        return MouseParse::NotMouse;
    }
    let parse_number = |part: &[u8]| {
        std::str::from_utf8(part)
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
    };
    let (Some(button), Some(x), Some(y)) = (
        parse_number(parts[0]),
        parse_number(parts[1]),
        parse_number(parts[2]),
    ) else {
        return MouseParse::NotMouse;
    };
    let kind = if data[end] == b'm' {
        MouseEventKind::Release
    } else if button & 32 != 0 {
        MouseEventKind::Motion
    } else {
        MouseEventKind::Press
    };
    let length = end - start + 1;
    MouseParse::Complete(
        MouseEvent {
            kind,
            button: u8::try_from(button).unwrap_or(u8::MAX),
            x,
            y,
            raw: data[start..start + length].to_vec(),
        },
        length,
    )
}

fn spawn_input(
    shared: Arc<Mutex<SharedInput>>,
    stop: Arc<AtomicBool>,
    events: mpsc::Sender<Event>,
) {
    thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = [0_u8; 8192];
        let mut prefix = false;
        let mut pending = Vec::new();
        while !stop.load(Ordering::SeqCst) {
            let count = match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => count,
            };
            pending.extend_from_slice(&buffer[..count]);
            let data = std::mem::take(&mut pending);
            let mut byte_index = 0;
            let mut payload = Vec::new();
            let mut focus_changed = None;
            let mut action = None;
            let mut detach = false;
            while byte_index < data.len() {
                let byte = data[byte_index];
                if byte == 0x1b {
                    match parse_sgr_mouse(&data, byte_index) {
                        MouseParse::Complete(event, length) => {
                            byte_index += length;
                            let mode = shared
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .ui_mode;
                            if matches!(mode, UiMode::Normal | UiMode::Copy)
                                && events.send(Event::Mouse(event)).is_err()
                            {
                                return;
                            }
                            continue;
                        }
                        MouseParse::Incomplete => {
                            pending.extend_from_slice(&data[byte_index..]);
                            break;
                        }
                        MouseParse::NotMouse => {}
                    }
                }
                byte_index += 1;
                let mode = shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .ui_mode;
                if mode == UiMode::Menu {
                    let mut next_action = None;
                    let mut copy = false;
                    let mut close = false;
                    match byte {
                        b'c' => next_action = Some(ComposeAction::Split),
                        b'f' => next_action = Some(ComposeAction::Float),
                        b'd' => {
                            shared
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .ui_mode = UiMode::MoveFloating;
                            let _ = events.send(Event::UiChanged);
                            let _ = events.send(Event::FloatingMoveStart);
                        }
                        b'x' => next_action = Some(ComposeAction::Close),
                        b'h' => next_action = Some(ComposeAction::MoveLeft),
                        b'l' => next_action = Some(ComposeAction::MoveRight),
                        b'[' => copy = true,
                        b'q' | b'm' | 0x1b => close = true,
                        _ => {}
                    }
                    if next_action.is_some() || copy || close {
                        shared
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .ui_mode = if copy { UiMode::Copy } else { UiMode::Normal };
                        let _ = events.send(Event::UiChanged);
                        if let Some(action) = next_action {
                            let _ = events.send(Event::Action(action));
                        } else if copy {
                            let _ = events.send(Event::CopyStart);
                        }
                    }
                    continue;
                }
                if mode == UiMode::Copy {
                    let command = match byte {
                        b'k' => Some(CopyCommand::Up),
                        b'j' => Some(CopyCommand::Down),
                        b'h' => Some(CopyCommand::Left),
                        b'l' => Some(CopyCommand::Right),
                        b' ' | b'v' => Some(CopyCommand::Start),
                        b'V' => Some(CopyCommand::SelectLine),
                        b'A' => Some(CopyCommand::SelectAll),
                        0x15 => Some(CopyCommand::ScrollUp),
                        0x04 => Some(CopyCommand::ScrollDown),
                        b'y' | b'\r' => Some(CopyCommand::Finish),
                        b'q' | 0x1b => Some(CopyCommand::Cancel),
                        _ => None,
                    };
                    if let Some(command) = command {
                        if matches!(command, CopyCommand::Finish | CopyCommand::Cancel) {
                            shared
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .ui_mode = UiMode::Normal;
                            let _ = events.send(Event::UiChanged);
                        }
                        let _ = events.send(Event::Copy(command));
                    }
                    continue;
                }
                if mode == UiMode::MoveFloating {
                    let command = match byte {
                        b'h' => Some(FloatingMoveCommand::Left),
                        b'j' => Some(FloatingMoveCommand::Down),
                        b'k' => Some(FloatingMoveCommand::Up),
                        b'l' => Some(FloatingMoveCommand::Right),
                        b'H' => Some(FloatingMoveCommand::ShrinkWidth),
                        b'L' => Some(FloatingMoveCommand::GrowWidth),
                        b'K' => Some(FloatingMoveCommand::ShrinkHeight),
                        b'J' => Some(FloatingMoveCommand::GrowHeight),
                        b'y' | b'\r' => Some(FloatingMoveCommand::Finish),
                        b'q' | 0x1b => Some(FloatingMoveCommand::Cancel),
                        _ => None,
                    };
                    if let Some(command) = command {
                        if matches!(
                            command,
                            FloatingMoveCommand::Finish | FloatingMoveCommand::Cancel
                        ) {
                            shared
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .ui_mode = UiMode::Normal;
                            let _ = events.send(Event::UiChanged);
                        }
                        let _ = events.send(Event::FloatingMove(command));
                    }
                    continue;
                }
                if byte == 0x1d {
                    detach = true;
                    break;
                }
                if prefix {
                    prefix = false;
                    match byte {
                        b'a' => payload.push(0x01),
                        b']' | b'q' => detach = true,
                        b'c' => action = Some(ComposeAction::Split),
                        b'f' => action = Some(ComposeAction::Float),
                        b'd' => {
                            shared
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .ui_mode = UiMode::MoveFloating;
                            let _ = events.send(Event::UiChanged);
                            let _ = events.send(Event::FloatingMoveStart);
                        }
                        b'x' => action = Some(ComposeAction::Close),
                        b'h' => action = Some(ComposeAction::MoveLeft),
                        b'l' => action = Some(ComposeAction::MoveRight),
                        b'm' => {
                            shared
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .ui_mode = UiMode::Menu;
                            let _ = events.send(Event::UiChanged);
                        }
                        b'[' => {
                            shared
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .ui_mode = UiMode::Copy;
                            let _ = events.send(Event::UiChanged);
                            let _ = events.send(Event::CopyStart);
                        }
                        b'n' | b'p' => {
                            let Ok(mut shared) = shared.lock() else {
                                return;
                            };
                            let count = shared.targets.len();
                            if count > 0 {
                                shared.focused = if byte == b'n' {
                                    (shared.focused + 1) % count
                                } else {
                                    (shared.focused + count - 1) % count
                                };
                                focus_changed = Some(shared.focused);
                            }
                        }
                        b'1'..=b'9' => {
                            let Ok(mut shared) = shared.lock() else {
                                return;
                            };
                            let index = usize::from(byte - b'1');
                            if index < shared.targets.len() {
                                shared.focused = index;
                                focus_changed = Some(index);
                            }
                        }
                        _ => payload.push(byte),
                    }
                    if detach {
                        break;
                    }
                    continue;
                }
                if byte == 0x01 {
                    prefix = true;
                } else {
                    payload.push(byte);
                }
            }
            if !payload.is_empty() {
                let Ok(shared) = shared.lock() else {
                    return;
                };
                let Some(target) = shared.targets.get(shared.focused) else {
                    return;
                };
                if target.active {
                    let Ok(mut writer) = target.writer.lock() else {
                        return;
                    };
                    if attach::write_payload_frame(&mut *writer, FrameType::Input, &payload)
                        .is_err()
                    {
                        return;
                    }
                }
            }
            if let Some(index) = focus_changed
                && events.send(Event::Focus(index)).is_err()
            {
                return;
            }
            if let Some(action) = action
                && events.send(Event::Action(action)).is_err()
            {
                return;
            }
            if detach {
                let _ = events.send(Event::Detach);
                return;
            }
        }
    });
}

fn spawn_resize(initial: (u16, u16), stop: Arc<AtomicBool>, events: mpsc::Sender<Event>) {
    thread::spawn(move || {
        let mut previous = initial;
        while !stop.load(Ordering::SeqCst) {
            thread::sleep(RESIZE_POLL_INTERVAL);
            let current = terminal_size();
            if current == previous {
                continue;
            }
            previous = current;
            if events.send(Event::Viewport(current.0, current.1)).is_err() {
                return;
            }
        }
    });
}

fn send_resize(
    writer: &Arc<Mutex<UnixStream>>,
    rows: u16,
    columns: u16,
) -> Result<(), ProtocolErrorBody> {
    let resize = attach::Resize {
        protocol_version: ATTACH_PROTOCOL_VERSION,
        rows,
        columns,
    };
    let mut writer = writer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    attach::write_json_frame(&mut *writer, FrameType::Resize, &resize)
        .map_err(|failure| attach_error("transport", failure.to_string()))
}

fn cancel_copy(
    shared: &Arc<Mutex<SharedInput>>,
    copy_state: &mut Option<CopyState>,
    notice: &mut Option<String>,
    reason: &str,
) {
    *copy_state = None;
    *notice = Some(format!("copy cancelled: {reason}"));
    if let Ok(mut shared) = shared.lock() {
        shared.ui_mode = UiMode::Normal;
    }
}

fn detach_all(shared: &Arc<Mutex<SharedInput>>) {
    let Ok(shared) = shared.lock() else {
        return;
    };
    for target in &shared.targets {
        let Ok(mut writer) = target.writer.lock() else {
            continue;
        };
        let detach = attach::Detach {
            protocol_version: ATTACH_PROTOCOL_VERSION,
            hold_ttl_ms: 0,
        };
        let _ = attach::write_json_frame(&mut *writer, FrameType::Detach, &detach);
        let _ = writer.shutdown(std::net::Shutdown::Both);
    }
}

fn selection_contains(copy: &CopyState, point: CopyPoint) -> bool {
    let Some(anchor) = copy.anchor else {
        return point == copy.cursor;
    };
    let (start, end) = ordered_points(anchor, copy.cursor);
    point >= start && point <= end
}

fn ordered_points(left: CopyPoint, right: CopyPoint) -> (CopyPoint, CopyPoint) {
    if (left.row, left.column) <= (right.row, right.column) {
        (left, right)
    } else {
        (right, left)
    }
}

fn copy_text(view: &PaneView, anchor: CopyPoint, cursor: CopyPoint) -> String {
    let (start, end) = ordered_points(anchor, cursor);
    let screen = view.parser.screen();
    let mut lines = Vec::new();
    for row in start.row..=end.row {
        let first_column = if row == start.row { start.column } else { 0 };
        let last_column = if row == end.row {
            end.column
        } else {
            screen.size().1.get().saturating_sub(1)
        };
        let mut line = String::new();
        for column in first_column..=last_column {
            let Some(cell) = screen.cell(row, column) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            if cell.has_contents() {
                line.push_str(cell.contents());
            } else {
                line.push(' ');
            }
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n").trim_end().to_string()
}

fn write_clipboard(text: &str) -> Result<String, String> {
    if let Ok(command) = std::env::var("QWE_CLIPBOARD_COMMAND") {
        let mut process = Command::new("sh");
        process.arg("-c").arg(command);
        return write_clipboard_process(&mut process, "environment override", text);
    }

    #[cfg(target_os = "macos")]
    let candidates = vec![("pbcopy", Vec::<&str>::new())];
    #[cfg(target_os = "linux")]
    let candidates = vec![
        ("wl-copy", vec!["--type", "text/plain"]),
        ("xclip", vec!["-selection", "clipboard"]),
        ("xsel", vec!["--clipboard", "--input"]),
    ];
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let candidates: Vec<(&str, Vec<&str>)> = Vec::new();

    let mut failures = Vec::new();
    for (program, arguments) in candidates {
        let mut process = Command::new(program);
        process.args(arguments);
        match write_clipboard_process(&mut process, program, text) {
            Ok(provider) => return Ok(provider),
            Err(failure) => failures.push(failure),
        }
    }
    if failures.is_empty() {
        Err("no clipboard provider is available on this platform".into())
    } else {
        Err(format!("clipboard unavailable: {}", failures.join("; ")))
    }
}

fn write_clipboard_process(
    process: &mut Command,
    provider: &str,
    text: &str,
) -> Result<String, String> {
    let mut child = process
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|failure| format!("{provider}: {failure}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| format!("{provider}: clipboard stdin is unavailable"))?
        .write_all(text.as_bytes())
        .map_err(|failure| format!("{provider}: {failure}"))?;
    let output = child
        .wait_with_output()
        .map_err(|failure| format!("{provider}: {failure}"))?;
    if output.status.success() {
        Ok(provider.to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "{provider}: exited with {}: {}",
            output.status,
            stderr.trim()
        ))
    }
}

fn render(
    session_id: &str,
    terminal_rows: u16,
    terminal_columns: u16,
    views: &[PaneView],
    layouts: &[qwe_core::PaneLayout],
    ui: RenderUi<'_>,
) -> std::io::Result<()> {
    let RenderUi {
        focused,
        copy_state,
        menu_open,
        moving,
        notice,
    } = ui;
    let mut output = String::new();
    output.push_str("\x1b[?25l\x1b[?2026h\x1b[0m\x1b[2J\x1b[H");
    let mut cursor = None;

    for (index, (view, layout)) in views.iter().zip(layouts.iter()).enumerate() {
        let rect = layout.rect;
        if rect.x >= terminal_columns || rect.y >= terminal_rows {
            continue;
        }
        let visible_columns = rect.columns.min(terminal_columns - rect.x);
        let label = match (view.spec.kind, view.active) {
            (PaneKind::Floating, true) => "[float]",
            (PaneKind::Floating, false) => "[float closed]",
            (PaneKind::Normal, true) => "",
            (PaneKind::Normal, false) => "[closed]",
        };
        let title = truncate_display(
            &format!(
                " {} {} {} ",
                index + 1,
                short_id(&view.id.to_string()),
                label
            ),
            visible_columns,
        );
        output.push_str(&format!("\x1b[{};{}H", rect.y + 1, rect.x + 1));
        output.push_str(if index == focused {
            "\x1b[7m"
        } else {
            "\x1b[0m"
        });
        output.push_str(&title);
        output.push_str(
            &" ".repeat(usize::from(visible_columns).saturating_sub(title.chars().count())),
        );
        output.push_str("\x1b[0m");

        if rect.rows <= 1 {
            continue;
        }
        let screen = view.parser.screen();
        let (screen_rows, screen_columns) = screen.size();
        let content_rows = rect
            .rows
            .saturating_sub(1)
            .min(screen_rows.get())
            .min(terminal_rows.saturating_sub(rect.y + 1));
        let content_columns = visible_columns.min(screen_columns.get());
        for row in 0..content_rows {
            output.push_str(&format!("\x1b[{};{}H", rect.y + row + 2, rect.x + 1));
            output.push_str("\x1b[0m");
            let mut current_style = CellStyle::default();
            let mut column = 0_u16;
            while column < content_columns {
                let Some(cell) = screen.cell(row, column) else {
                    break;
                };
                if cell.is_wide_continuation() {
                    column += 1;
                    continue;
                }
                let style = style_of(cell);
                let style = if index == focused
                    && copy_state
                        .is_some_and(|copy| selection_contains(copy, CopyPoint { row, column }))
                {
                    CellStyle {
                        inverse: !style.inverse,
                        ..style
                    }
                } else {
                    style
                };
                if style != current_style {
                    output.push_str(&style_sequence(style));
                    current_style = style;
                }
                if cell.has_contents() {
                    output.push_str(cell.contents());
                } else {
                    output.push(' ');
                }
                column += 1;
            }
            if column < content_columns {
                output.push_str(&" ".repeat(usize::from(content_columns - column)));
            }
        }
        if view.spec.kind == PaneKind::Floating && rect.rows > 1 {
            output.push_str(&format!(
                "\x1b[{};{}H\x1b[7m+\x1b[0m",
                rect.y.saturating_add(rect.rows),
                rect.x.saturating_add(rect.columns)
            ));
        }
        if let Some(copy) = copy_state.filter(|_| index == focused) {
            cursor = Some((
                rect.y.saturating_add(copy.cursor.row).saturating_add(2),
                rect.x.saturating_add(copy.cursor.column).saturating_add(1),
            ));
        } else if index == focused && view.active && !screen.hide_cursor() {
            let (row, column) = screen.cursor_position();
            if row < content_rows && column < content_columns {
                cursor = Some((
                    rect.y.saturating_add(row).saturating_add(2),
                    rect.x.saturating_add(column).saturating_add(1),
                ));
            }
        }
    }

    let status = if menu_open {
        " MENU  c split  f float  d move  x close  h/l order  [ copy  q close ".to_string()
    } else if moving {
        " MOVE FLOAT  h/j/k/l move  H/L width  K/J height  y save  Esc cancel ".to_string()
    } else if let Some(copy) = copy_state {
        format!(
            " COPY row={} col={} scroll={}  h/j/k/l move  ^U/^D scroll  space start  V line  A all  y copy  Esc cancel ",
            copy.cursor.row + 1,
            copy.cursor.column + 1,
            copy.scrollback
        )
    } else if let Some(notice) = notice {
        format!(
            " QWE {}  panes={}  focus={}/{}  {notice} ",
            short_id(session_id),
            views.len(),
            focused + 1,
            views.len()
        )
    } else {
        format!(
            " QWE {}  panes={}  focus={}/{}  Ctrl-A n/p c/f/d/x m [  Ctrl-A ] detach ",
            short_id(session_id),
            views.len(),
            focused + 1,
            views.len()
        )
    };
    let status = truncate_display(&status, terminal_columns);
    output.push_str(&format!("\x1b[{};1H\x1b[2K\x1b[7m", terminal_rows));
    output.push_str(&status);
    output.push_str(
        &" ".repeat(usize::from(terminal_columns).saturating_sub(status.chars().count())),
    );
    output.push_str("\x1b[0m");
    if let Some((row, column)) = cursor {
        output.push_str(&format!("\x1b[{row};{column}H\x1b[?25h"));
    } else {
        output.push_str("\x1b[?25l");
    }
    output.push_str("\x1b[?2026l");

    let mut stdout = std::io::stdout().lock();
    stdout.write_all(output.as_bytes())?;
    stdout.flush()
}

fn style_of(cell: &vt100::Cell) -> CellStyle {
    CellStyle {
        foreground: cell.fgcolor(),
        background: cell.bgcolor(),
        bold: cell.bold(),
        dim: cell.dim(),
        italic: cell.italic(),
        underline: cell.underline(),
        inverse: cell.inverse(),
    }
}

fn style_sequence(style: CellStyle) -> String {
    let mut sequence = "\x1b[0".to_string();
    if style.bold {
        sequence.push_str(";1");
    }
    if style.dim {
        sequence.push_str(";2");
    }
    if style.italic {
        sequence.push_str(";3");
    }
    if style.underline {
        sequence.push_str(";4");
    }
    if style.inverse {
        sequence.push_str(";7");
    }
    push_color(&mut sequence, style.foreground, true);
    push_color(&mut sequence, style.background, false);
    sequence.push('m');
    sequence
}

fn push_color(sequence: &mut String, color: Color, foreground: bool) {
    match color {
        Color::Default => sequence.push_str(if foreground { ";39" } else { ";49" }),
        Color::Idx(index @ 0..=7) => {
            sequence.push_str(&format!(";{}", (if foreground { 30 } else { 40 }) + index));
        }
        Color::Idx(index @ 8..=15) => {
            sequence.push_str(&format!(
                ";{}",
                (if foreground { 90 } else { 100 }) + index - 8
            ));
        }
        Color::Idx(index) => {
            sequence.push_str(&format!(";{};5;{index}", if foreground { 38 } else { 48 }));
        }
        Color::Rgb(red, green, blue) => {
            sequence.push_str(&format!(
                ";{};2;{red};{green};{blue}",
                if foreground { 38 } else { 48 }
            ));
        }
    }
}

fn truncate_display(value: &str, columns: u16) -> String {
    value.chars().take(usize::from(columns)).collect()
}

fn short_id(value: &str) -> &str {
    value.get(..8).unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_sequence_preserves_true_color_and_attributes() {
        assert_eq!(
            style_sequence(CellStyle {
                foreground: Color::Rgb(1, 2, 3),
                background: Color::Idx(200),
                bold: true,
                ..CellStyle::default()
            }),
            "\x1b[0;1;38;2;1;2;3;48;5;200m"
        );
    }

    #[test]
    fn long_values_are_clipped_to_the_viewport() {
        assert_eq!(truncate_display("abcdefgh", 3), "abc");
        assert_eq!(short_id("0123456789"), "01234567");
    }

    #[test]
    fn snapshot_seed_does_not_scroll_the_last_row() {
        let mut parser = Parser::new(
            NonZeroU16::new(3).unwrap(),
            NonZeroU16::new(10).unwrap(),
            10,
        );
        seed_parser(
            &mut parser,
            &attach::TerminalSnapshot {
                rows: 3,
                columns: 10,
                cursor_row: 1,
                cursor_column: 2,
                cursor_visible: true,
                cursor_style: attach::CursorStyle::Default,
                alternate_screen: false,
                application_cursor: false,
                application_keypad: false,
                bracketed_paste: false,
                mouse_protocol_mode: attach::MouseProtocolMode::None,
                mouse_protocol_encoding: attach::MouseProtocolEncoding::Default,
                screen_text: "one\ntwo\nthree".into(),
                next_stream_offset: 0,
            },
        );
        assert_eq!(parser.screen().contents(), "one\ntwo\nthree");
    }

    #[test]
    fn clipboard_provider_nonzero_exit_is_an_explicit_error() {
        let mut process = Command::new("sh");
        process.arg("-c").arg("exit 7");
        let failure = write_clipboard_process(&mut process, "test-provider", "copy")
            .expect_err("nonzero clipboard exit must fail");
        assert!(failure.contains("test-provider"));
        assert!(failure.contains("exit status: 7"));
    }

    #[test]
    fn sgr_mouse_parser_classifies_press_motion_and_release() {
        let press = b"\x1b[<0;9;5M";
        let motion = b"\x1b[<32;12;7M";
        let release = b"\x1b[<0;12;7m";
        let MouseParse::Complete(press, 9) = parse_sgr_mouse(press, 0) else {
            panic!("press sequence must parse");
        };
        assert_eq!(press.kind, MouseEventKind::Press);
        assert_eq!((press.x, press.y), (9, 5));
        let MouseParse::Complete(motion, 11) = parse_sgr_mouse(motion, 0) else {
            panic!("motion sequence must parse");
        };
        assert_eq!(motion.kind, MouseEventKind::Motion);
        assert_eq!((motion.x, motion.y), (12, 7));
        let MouseParse::Complete(release, 10) = parse_sgr_mouse(release, 0) else {
            panic!("release sequence must parse");
        };
        assert_eq!(release.kind, MouseEventKind::Release);
        assert!(matches!(
            parse_sgr_mouse(b"\x1b[<0;9", 0),
            MouseParse::Incomplete
        ));
    }
}
