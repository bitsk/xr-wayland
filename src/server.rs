// SPDX-License-Identifier: GPL-3.0-or-later
use crate::bridge::{Bridge, Incoming, Outgoing};
mod application;
mod keyboard_focus;
mod xwayland;
use anyhow::{Context, Result, bail, ensure};
use keyboard_focus::KeyboardFocus;
use serde_json::{Value, json};
use smithay::wayland::seat::WaylandFocus;
use smithay::{
    backend::input::{Axis, AxisSource, ButtonState, KeyState},
    delegate_compositor, delegate_data_device, delegate_input_method_manager,
    delegate_kde_decoration, delegate_output, delegate_primary_selection, delegate_seat,
    delegate_shm, delegate_text_input_manager, delegate_virtual_keyboard_manager,
    delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{PopupKeyboardGrab, PopupKind, PopupManager, PopupPointerGrab},
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{FilterResult, XkbConfig},
        pointer::{AxisFrame, ButtonEvent, Focus, MotionEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
            shell::server::xdg_toplevel,
        },
        wayland_protocols_misc::server_decoration::server::{
            org_kde_kwin_server_decoration::{Mode as KdeMode, OrgKdeKwinServerDecoration},
            org_kde_kwin_server_decoration_manager::Mode as KdeDefaultMode,
        },
        wayland_server::{
            Client, DisplayHandle, Resource,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer::WlBuffer, wl_seat::WlSeat, wl_shm, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Rectangle, SERIAL_COUNTER, Serial, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            BufferAssignment, CompositorClientState, CompositorHandler, CompositorState,
            SubsurfaceCachedState, SurfaceAttributes, TraversalAction, get_parent,
            is_sync_subsurface, with_states, with_surface_tree_downward,
        },
        input_method::{InputMethodHandler, InputMethodManagerState, PopupSurface as ImePopup},
        output::OutputHandler,
        selection::{
            SelectionHandler, SelectionSource, SelectionTarget,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                current_data_device_selection_userdata, request_data_device_client_selection,
                set_data_device_focus, set_data_device_selection,
            },
            primary_selection::{
                PrimarySelectionHandler, PrimarySelectionState, current_primary_selection_userdata,
                request_primary_client_selection, set_primary_focus, set_primary_selection,
            },
        },
        shell::kde::decoration::{KdeDecorationHandler, KdeDecorationState},
        shell::xdg::{
            Configure, PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface,
            XdgShellHandler, XdgShellState, XdgToplevelSurfaceData,
            decoration::{XdgDecorationHandler, XdgDecorationState},
        },
        shm::{ShmHandler, ShmState, with_buffer_contents},
        text_input::{TextInputManagerState, TextInputSeat},
        virtual_keyboard::VirtualKeyboardManagerState,
    },
};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    os::{
        fd::{FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Instant,
};

#[derive(Default)]
pub struct ClientState {
    compositor: CompositorClientState,
}
impl ClientData for ClientState {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, reason: DisconnectReason) {
        println!("XR_WAYLAND_CLIENT_DISCONNECTED={reason:?}");
    }
}
#[derive(Clone)]
struct Surface {
    id: u64,
    surface: WlSurface,
    top: Option<ToplevelSurface>,
    x11: Option<smithay::xwayland::X11Surface>,
    popup: Option<PopupSurface>,
    parent: Option<u64>,
    width: i32,
    height: i32,
    configured: (i32, i32),
    ack: u32,
    commit: u64,
    frame: u64,
    role: &'static str,
    app_id: String,
    title: String,
    popup_position: (i32, i32),
    mapped: bool,
}
struct Snapshot {
    command: Outgoing,
}
struct ClipboardRead {
    bytes: Vec<u8>,
    fd: std::fs::File,
    mime: String,
    started: Instant,
    target: SelectionTarget,
    origin: &'static str,
}

pub struct State {
    dh: DisplayHandle,
    compositor: CompositorState,
    xdg: XdgShellState,
    _decoration: XdgDecorationState,
    kde_decoration: KdeDecorationState,
    kde_mode_requests: HashMap<smithay::reexports::wayland_server::backend::ObjectId, u32>,
    shm: ShmState,
    seat_state: SeatState<Self>,
    seat: Seat<Self>,
    data_device: DataDeviceState,
    primary: PrimarySelectionState,
    _text_input: TextInputManagerState,
    _input_method: InputMethodManagerState,
    _virtual_keyboard: VirtualKeyboardManagerState,
    output: Output,
    xwayland_shell: smithay::wayland::xwayland_shell::XWaylandShellState,
    xwayland_loop: Option<smithay::reexports::calloop::EventLoop<'static, State>>,
    xwm: Option<smithay::xwayland::X11Wm>,
    xwayland_handle: Option<smithay::reexports::calloop::LoopHandle<'static, State>>,
    x11_selections: Vec<(SelectionTarget, String)>,
    x11_display: Option<String>,
    x11_windows: Vec<smithay::xwayland::X11Surface>,
    pending_x11_launches: Vec<String>,
    popups: PopupManager,
    surfaces: HashMap<u64, Surface>,
    next_surface: u64,
    next_buffer: u64,
    focus_epoch: u64,
    focus: Option<u64>,
    pointer_at: (f64, f64),
    started: Instant,
    metadata: VecDeque<Outgoing>,
    snapshots: HashMap<u64, Snapshot>,
    clipboard_reads: Vec<ClipboardRead>,
    clipboard_requests: Vec<(SelectionTarget, String)>,
    children: Vec<Child>,
    socket_name: String,
    pub bridge: Bridge,
    snapshots_sent: u64,
    releases: u64,
    stopped: bool,
    pub quit_requested: bool,
}
impl State {
    pub fn new(dh: DisplayHandle, bridge: Bridge, socket_name: String) -> Result<Self> {
        let compositor = CompositorState::new::<Self>(&dh);
        let xdg = XdgShellState::new::<Self>(&dh);
        let shm = ShmState::new::<Self>(&dh, []);
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&dh, "xr-seat");
        seat.add_keyboard(XkbConfig::default(), 250, 30)?;
        seat.add_pointer();
        let output = Output::new(
            "XR-VIRTUAL-0".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "openKylin".into(),
                model: "XR private display".into(),
            },
        );
        output.create_global::<Self>(&dh);
        let mode = Mode {
            size: (1600, 1000).into(),
            refresh: 60000,
        };
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            None,
            Some((0, 0).into()),
        );
        output.set_preferred(mode);
        Ok(Self {
            compositor,
            xdg,
            _decoration: XdgDecorationState::new::<Self>(&dh),
            kde_decoration: KdeDecorationState::new::<Self>(&dh, KdeDefaultMode::Server),
            kde_mode_requests: HashMap::new(),
            shm,
            seat_state,
            seat,
            data_device: DataDeviceState::new::<Self>(&dh),
            primary: PrimarySelectionState::new::<Self>(&dh),
            _text_input: TextInputManagerState::new::<Self>(&dh),
            _input_method: InputMethodManagerState::new::<Self, _>(&dh, |_| true),
            _virtual_keyboard: VirtualKeyboardManagerState::new::<Self, _>(&dh, |_| true),
            dh: dh.clone(),
            output,
            xwayland_shell: smithay::wayland::xwayland_shell::XWaylandShellState::new::<Self>(&dh),
            xwayland_loop: None,
            xwm: None,
            xwayland_handle: None,
            x11_selections: Vec::new(),
            x11_display: None,
            x11_windows: Vec::new(),
            pending_x11_launches: Vec::new(),
            popups: PopupManager::default(),
            surfaces: HashMap::new(),
            next_surface: 1,
            next_buffer: 1,
            focus_epoch: 0,
            focus: None,
            pointer_at: (0.0, 0.0),
            started: Instant::now(),
            metadata: VecDeque::new(),
            snapshots: HashMap::new(),
            clipboard_reads: Vec::new(),
            clipboard_requests: Vec::new(),
            children: Vec::new(),
            socket_name,
            bridge,
            snapshots_sent: 0,
            releases: 0,
            stopped: false,
            quit_requested: false,
        })
    }
    fn emit(&mut self, kind: &'static str, payload: Value) {
        if kind == "WindowState" {
            if let Some(existing) =
                self.metadata.iter_mut().rev().find(|old| {
                    old.kind == kind && old.payload["surface_id"] == payload["surface_id"]
                })
            {
                existing.payload = payload;
                return;
            }
        }
        self.metadata.push_back(Outgoing {
            kind,
            payload,
            fd: None,
        });
    }
    fn id_for(&self, wl: &WlSurface) -> Option<u64> {
        self.surfaces
            .values()
            .find(|s| &s.surface == wl)
            .map(|s| s.id)
    }
    fn register(&mut self, wl: WlSurface, role: &'static str, parent: Option<u64>) -> u64 {
        if let Some(id) = self.id_for(&wl) {
            return id;
        }
        let id = self.next_surface;
        self.next_surface += 1;
        self.output.enter(&wl);
        self.surfaces.insert(
            id,
            Surface {
                id,
                surface: wl,
                top: None,
                x11: None,
                popup: None,
                parent,
                width: 800,
                height: 600,
                configured: (800, 600),
                ack: 0,
                commit: 0,
                frame: 0,
                role,
                app_id: String::new(),
                title: String::new(),
                popup_position: (0, 0),
                mapped: false,
            },
        );
        id
    }
    fn window_state(&mut self, id: u64) {
        let Some(s) = self.surfaces.get(&id) else {
            return;
        };
        // Transport (Xwayland) is not a window role. Only managed, independent
        // application windows get Shell's companion controls; menus, tooltips,
        // splash windows and transient children remain parent-owned surfaces.
        let window_controls = if let Some(x11) = &s.x11 {
            use smithay::xwayland::xwm::WmWindowType;
            !x11.is_override_redirect()
                && x11.is_transient_for().is_none()
                && matches!(
                    x11.window_type(),
                    None | Some(
                        WmWindowType::Normal | WmWindowType::Dialog | WmWindowType::Utility
                    )
                )
        } else {
            s.role == "toplevel"
        };
        self.emit("WindowState", json!({"surface_id":id.to_string(),"app_id":s.app_id,"title":s.title,
            "width":s.width,"height":s.height,"configured_width":s.configured.0,"configured_height":s.configured.1,
            "acked_configure":s.ack.to_string(),"parent_surface_id":s.parent.map(|v|v.to_string()),
            "role":s.role,"window_controls":window_controls,
            "x11_override_redirect":s.x11.as_ref().map(|x|x.is_override_redirect()),
            "x11_window_type":s.x11.as_ref().map(|x|format!("{:?}",x.window_type())),
            "client_side_decorated":s.x11.as_ref().map(|x|x.is_decorated()),
            "popup_x":s.popup_position.0,"popup_y":s.popup_position.1,
            "mapped":s.mapped,"commit_serial":s.commit.to_string()}));
    }
    fn remove(&mut self, id: u64) {
        if self.surfaces.remove(&id).is_some() {
            self.snapshots.remove(&id);
            self.emit("WindowRemoved", json!({"surface_id":id.to_string()}));
            if self.focus == Some(id) {
                self.apply_focus(None);
                self.focus = None;
            }
        }
    }
    fn update_toplevel(&mut self, top: &ToplevelSurface) {
        let Some(id) = self.id_for(top.wl_surface()) else {
            return;
        };
        let (title, app, parent) = with_states(top.wl_surface(), |st| {
            let d = st
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();
            (
                d.title.clone().unwrap_or_default(),
                d.app_id.clone().unwrap_or_default(),
                d.parent.clone(),
            )
        });
        let parent = parent.as_ref().and_then(|wl| self.id_for(wl));
        let s = self.surfaces.get_mut(&id).unwrap();
        s.title = title;
        s.app_id = app;
        s.parent = parent;
        self.window_state(id);
    }
    fn capture_surface(&mut self, wl: &WlSurface) {
        let parent = get_parent(wl).as_ref().and_then(|p| self.id_for(p));
        // Cursor and otherwise unassociated surfaces must not become desktop windows.
        if self.id_for(wl).is_none() && parent.is_none() {
            return;
        }
        let id = self
            .id_for(wl)
            .unwrap_or_else(|| self.register(wl.clone(), "subsurface", parent));
        if parent.is_some() {
            let offset = with_states(wl, |st| {
                st.cached_state
                    .get::<SubsurfaceCachedState>()
                    .current()
                    .location
            });
            self.surfaces.get_mut(&id).unwrap().popup_position = (offset.x, offset.y);
        }
        let (assignment, scale, transform) = with_states(wl, |s| {
            let mut attr = s.cached_state.get::<SurfaceAttributes>();
            let a = attr.current();
            (a.buffer.take(), a.buffer_scale, a.buffer_transform)
        });
        let Some(assignment) = assignment else {
            return;
        };
        if matches!(assignment, BufferAssignment::Removed) {
            if let Some(s) = self.surfaces.get_mut(&id) {
                s.mapped = false;
            }
            self.window_state(id);
            return;
        }
        let BufferAssignment::NewBuffer(buffer) = assignment else {
            return;
        };
        let snapshot = with_buffer_contents(
            &buffer,
            |ptr, len, b| -> Result<(Vec<u8>, i32, i32, &'static str)> {
                ensure!(
                    b.width > 0
                        && b.height > 0
                        && b.width <= 8192
                        && b.height <= 8192
                        && b.stride >= b.width * 4
                        && b.offset >= 0,
                    "invalid shm geometry"
                );
                ensure!(transform==smithay::reexports::wayland_server::protocol::wl_output::Transform::Normal,"unsupported buffer transform");
                let last = b.offset as usize
                    + (b.height as usize - 1) * b.stride as usize
                    + b.width as usize * 4;
                ensure!(
                    last <= len && b.width as usize * b.height as usize * 4 <= 134217728,
                    "shm extends beyond pool"
                );
                let format = match b.format {
                    wl_shm::Format::Argb8888 => "argb8888",
                    wl_shm::Format::Xrgb8888 => "xrgb8888",
                    _ => bail!("unsupported shm format"),
                };
                let mut bytes = vec![0; b.width as usize * b.height as usize * 4];
                for y in 0..b.height as usize {
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            ptr.add(b.offset as usize + y * b.stride as usize),
                            bytes.as_mut_ptr().add(y * b.width as usize * 4),
                            b.width as usize * 4,
                        );
                    }
                }
                Ok((bytes, b.width, b.height, format))
            },
        );
        // The snapshot owns the copied bytes before the application reuses its buffer.
        buffer.release();
        match snapshot {
            Ok(Ok((bytes, width, height, format))) => {
                let result = xr_shell_protocols::sealed_snapshot(&bytes);
                match result {
                    Ok(fd) => {
                        let s = self.surfaces.get_mut(&id).unwrap();
                        s.width = width / scale.max(1);
                        s.height = height / scale.max(1);
                        s.frame += 1;
                        s.commit += 1;
                        s.mapped = true;
                        let frame = s.frame;
                        let commit = s.commit;
                        self.window_state(id);
                        let buffer_id = self.next_buffer;
                        self.next_buffer += 1;
                        let payload = json!({"window_server_generation":self.bridge.generation,"surface_id":id.to_string(),
                            "buffer_id":buffer_id.to_string(),"frame_id":frame.to_string(),"commit_serial":commit.to_string(),
                            "width_px":width,"height_px":height,"stride_bytes":width*4,"offset_bytes":0,"length_bytes":bytes.len(),
                            "fd_index":0,"format":format,"alpha_mode":if format=="argb8888"{"premultiplied"}else{"opaque"},
                            "colour_space":"srgb","damage_rects":[[0,0,width,height]]});
                        self.snapshots.insert(
                            id,
                            Snapshot {
                                command: Outgoing {
                                    kind: "ShmBufferCommit",
                                    payload,
                                    fd: Some(fd),
                                },
                            },
                        );
                    }
                    Err(e) => eprintln!("XR_WAYLAND_SNAPSHOT_ERROR={e:#}"),
                }
            }
            Ok(Err(e)) => eprintln!("XR_WAYLAND_BUFFER_REJECTED={e:#}"),
            Err(e) => eprintln!("XR_WAYLAND_BUFFER_ACCESS={e:?}"),
        }
    }
    pub fn flush_bridge(&mut self) -> Result<()> {
        while let Some(command) = self.metadata.pop_front() {
            match self.bridge.try_send(command) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(command)) => {
                    self.metadata.push_front(command);
                    return Ok(());
                }
                Err(mpsc::TrySendError::Disconnected(_)) => bail!("Shell transport stopped"),
            }
        }
        let ids: Vec<_> = self.snapshots.keys().copied().collect();
        for id in ids {
            let snapshot = self.snapshots.remove(&id).unwrap();
            match self.bridge.try_send(snapshot.command) {
                Ok(()) => {
                    self.snapshots_sent += 1;
                }
                Err(mpsc::TrySendError::Full(command)) => {
                    self.snapshots.insert(id, Snapshot { command });
                    break;
                }
                Err(mpsc::TrySendError::Disconnected(_)) => bail!("Shell transport stopped"),
            }
        }
        Ok(())
    }
    pub fn dispatch_bridge(&mut self) -> Result<()> {
        while let Ok(event) = self.bridge.incoming.try_recv() {
            match event {
                Incoming::Disconnected(error) => bail!("Shell disconnected: {error}"),
                Incoming::Packet(message) => {
                    let p = &message["payload"];
                    match message["type"].as_str().unwrap_or("") {
                        "SessionEnding" => {
                            println!(
                                "XR_WAYLAND_SESSION_ENDING reason={}",
                                p["reason"].as_str().unwrap_or("session ending")
                            );
                            self.quit_requested = true;
                            return Ok(());
                        }
                        "BufferRelease" => self.releases += 1,
                        "FocusChanged" | "SurfaceInput" | "WindowIntent" => {
                            let kind = message["type"].as_str().unwrap();
                            let result =
                                if let Some(generation) = p["window_server_generation"].as_str() {
                                    if generation != self.bridge.generation {
                                        Err(anyhow::anyhow!("stale window server generation"))
                                    } else {
                                        self.handle_window_message(kind, p)
                                    }
                                } else {
                                    self.handle_window_message(kind, p)
                                };
                            if kind != "FocusChanged" {
                                if let (Some(origin_component), Some(origin_request)) = (
                                    p["origin_component_id"].as_str(),
                                    p["origin_request_id"].as_str(),
                                ) {
                                    let mut response = json!({"origin_component_id":origin_component,"origin_request_id":origin_request,"operation":kind,"success":result.is_ok()});
                                    if let Err(error) = &result {
                                        response["message"] = format!("{error:#}").into();
                                    }
                                    self.emit("RequestResult", response);
                                }
                            }
                            if let Err(error) = result {
                                eprintln!(
                                    "XR_WAYLAND_REQUEST_REJECTED type={kind} error={error:#}"
                                );
                                self.emit("Error", json!({"code":"invalid_request","message":format!("{error:#}"),"request_id":message["request_id"]}));
                            }
                        }
                        "ClipboardSet" => {
                            let text = p["text"].as_str().unwrap_or("");
                            ensure!(text.len() <= 60000, "clipboard exceeds protocol limit");
                            set_data_device_selection(
                                &self.dh,
                                &self.seat,
                                vec!["text/plain;charset=utf-8".into(), "text/plain".into()],
                                text.to_owned(),
                            );
                            if let Some(wm) = &mut self.xwm {
                                let _ = wm.new_selection(
                                    SelectionTarget::Clipboard,
                                    Some(vec![
                                        "text/plain;charset=utf-8".into(),
                                        "text/plain".into(),
                                    ]),
                                );
                            }
                            self.emit("ClipboardChanged",json!({"mime_type":"text/plain;charset=utf-8","text":text,"origin":"shell"}));
                        }
                        "Error" => eprintln!("XR_WAYLAND_SHELL_ERROR={p}"),
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }
    fn handle_window_message(&mut self, kind: &str, p: &Value) -> Result<()> {
        match kind {
            "FocusChanged" => self.focus_changed_message(p),
            "SurfaceInput" => self.surface_input(p),
            "WindowIntent" => self.window_intent(p),
            _ => unreachable!(),
        }
    }
    fn focus_changed_message(&mut self, p: &Value) -> Result<()> {
        let epoch = p["focus_epoch"]
            .as_str()
            .context("missing focus epoch")?
            .parse::<u64>()?;
        ensure!(epoch >= self.focus_epoch, "stale focus epoch");
        let requested = p["surface_id"]
            .as_str()
            .map(str::parse::<u64>)
            .transpose()?;
        let valid = requested.is_none_or(|id| self.surfaces.contains_key(&id));
        self.focus_epoch = epoch;
        if valid {
            self.apply_focus(requested);
            self.focus = requested;
        }
        self.emit("FocusApplied",json!({"focus_epoch":epoch.to_string(),"surface_id":requested.map(|v|v.to_string()),"success":valid}));
        Ok(())
    }
    fn apply_focus(&mut self, id: Option<u64>) {
        for s in self.surfaces.values() {
            if let Some(x11) = &s.x11 {
                let _ = x11.set_activated(Some(s.id) == id);
            }
            if let Some(top) = &s.top {
                top.with_pending_state(|pending| {
                    if Some(s.id) == id {
                        pending.states.set(xdg_toplevel::State::Activated);
                    } else {
                        pending.states.unset(xdg_toplevel::State::Activated);
                    }
                });
                top.send_pending_configure();
            }
        }
        let surface = id.and_then(|id| {
            self.surfaces.get(&id).map(|s| KeyboardFocus {
                surface: s.surface.clone(),
                x11: s.x11.clone(),
            })
        });
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
    }
    fn surface_input(&mut self, p: &Value) -> Result<()> {
        let epoch = p["focus_epoch"]
            .as_str()
            .context("missing input focus epoch")?
            .parse::<u64>()?;
        let id = p["surface_id"]
            .as_str()
            .context("missing input surface")?
            .parse::<u64>()?;
        ensure!(
            epoch == self.focus_epoch && self.focus == Some(id),
            "stale input focus"
        );
        let wl = self
            .surfaces
            .get(&id)
            .context("input surface is gone")?
            .surface
            .clone();
        let time = self.started.elapsed().as_millis() as u32;
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.seat.get_pointer().unwrap();
        match p["kind"].as_str().context("missing input kind")? {
            "pointer_motion" => {
                let x = p["x"].as_f64().context("missing x")?;
                let y = p["y"].as_f64().context("missing y")?;
                ensure!(x.is_finite() && y.is_finite(), "non-finite pointer");
                self.pointer_at = (x, y);
                pointer.motion(
                    self,
                    Some((wl, (0.0, 0.0).into())),
                    &MotionEvent {
                        location: (x, y).into(),
                        serial,
                        time,
                    },
                );
                pointer.frame(self);
            }
            "pointer_button" => {
                let button = p["button"].as_u64().context("missing Linux button")? as u32;
                let state = if p["pressed"].as_bool().unwrap_or(false) {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                pointer.button(
                    self,
                    &ButtonEvent {
                        serial,
                        time,
                        button,
                        state,
                    },
                );
                pointer.frame(self);
            }
            "key" => {
                let key = p["keycode"].as_u64().context("missing evdev keycode")?;
                ensure!(key <= 767, "invalid evdev keycode");
                let state = if p["pressed"].as_bool().unwrap_or(false) {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                };
                let keyboard = self.seat.get_keyboard().unwrap();
                keyboard.input::<(), _>(
                    self,
                    ((key as u32) + 8).into(),
                    state,
                    serial,
                    time,
                    |_, _, _| FilterResult::Forward,
                );
            }
            "text" => {
                let text = p["text"].as_str().context("missing text")?;
                let mut active = false;
                self.seat.text_input().with_active_text_input(|ti, _| {
                    ti.commit_string(Some(text.into()));
                    active = true;
                });
                self.seat.text_input().done(false);
                ensure!(
                    active,
                    "focused application has no active text-input-v3 object"
                );
            }
            "scroll" => {
                let x = p["dx"].as_f64().unwrap_or(0.0);
                let y = p["dy"].as_f64().unwrap_or(0.0);
                pointer.axis(
                    self,
                    AxisFrame::new(time)
                        .source(AxisSource::Wheel)
                        .value(Axis::Horizontal, x)
                        .value(Axis::Vertical, y),
                );
                pointer.frame(self);
            }
            _ => bail!("unsupported surface input"),
        }
        Ok(())
    }
    fn window_intent(&mut self, p: &Value) -> Result<()> {
        let intent = p["intent"].as_str().context("missing intent")?;
        if intent == "launch" {
            return self.launch(
                p["app_id"].as_str().context("missing app ID")?,
                p["x11"].as_bool().unwrap_or(false),
            );
        }
        let id = p["surface_id"]
            .as_str()
            .context("missing window surface")?
            .parse::<u64>()?;
        let s = self.surfaces.get(&id).context("window is gone")?.clone();
        match intent {
            "close" => {
                if let Some(top) = s.top {
                    top.send_close();
                } else if let Some(x11) = s.x11 {
                    x11.close()?;
                } else if let Some(popup) = s.popup {
                    popup.send_popup_done();
                }
            }
            "resize" => {
                let width = p["width"]
                    .as_i64()
                    .context("missing width")?
                    .clamp(64, 8192) as i32;
                let height = p["height"]
                    .as_i64()
                    .context("missing height")?
                    .clamp(64, 8192) as i32;
                if let Some(top) = s.top {
                    top.with_pending_state(|state| state.size = Some((width, height).into()));
                    top.send_configure();
                    self.surfaces.get_mut(&id).unwrap().configured = (width, height);
                    self.window_state(id);
                } else if let Some(x11) = s.x11 {
                    x11.configure(Some(Rectangle::new((0, 0).into(), (width, height).into())))?;
                    self.surfaces.get_mut(&id).unwrap().configured = (width, height);
                    self.window_state(id);
                }
            }
            "focus" => {} // Spatial focus is changed only by FocusChanged/FocusApplied.
            "move" => {}  // World-space movement belongs to Shell.
            _ => bail!("unsupported window intent"),
        }
        Ok(())
    }
    pub fn launch(&mut self, id: &str, x11: bool) -> Result<()> {
        ensure!(
            !id.contains('/') && !id.contains(".."),
            "application ID must be a desktop basename"
        );
        let filename = if id.ends_with(".desktop") {
            id.to_owned()
        } else {
            format!("{id}.desktop")
        };
        let mut paths = Vec::new();
        if let Some(home) = std::env::var_os("XDG_DATA_HOME") {
            paths.push(std::path::PathBuf::from(home).join("applications"));
        } else if let Some(home) = std::env::var_os("HOME") {
            paths.push(std::path::PathBuf::from(home).join(".local/share/applications"));
        }
        paths.extend(
            std::env::var("XDG_DATA_DIRS")
                .unwrap_or_else(|_| "/usr/local/share:/usr/share".into())
                .split(':')
                .map(|p| std::path::PathBuf::from(p).join("applications")),
        );
        let path = paths
            .into_iter()
            .map(|p| p.join(&filename))
            .find(|p| p.is_file())
            .context("desktop application is not installed")?;
        let text = std::fs::read_to_string(&path)?;
        let mut group = false;
        let mut exec = None;
        let mut backend = None;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                group = line == "[Desktop Entry]";
            } else if group {
                if let Some(value) = line.strip_prefix("Exec=") {
                    exec = Some(value.to_owned());
                }
                if let Some(value) = line.strip_prefix("X-XR-Backend=") {
                    backend = Some(value.to_owned());
                }
                if line == "Hidden=true" {
                    bail!("desktop application is hidden");
                }
            }
        }
        let x11 = x11 || application::use_x11(&filename, backend.as_deref())?;
        if x11 && self.x11_display.is_none() {
            ensure!(self.xwayland_loop.is_some(), "Xwayland was not enabled");
            self.pending_x11_launches.push(id.to_owned());
            return Ok(());
        }
        let words = shlex::split(&exec.context("desktop entry has no Exec")?)
            .context("invalid desktop Exec quoting")?;
        let words: Vec<String> = words
            .into_iter()
            .filter_map(|w| {
                if ["%f", "%F", "%u", "%U", "%i"].contains(&w.as_str()) {
                    None
                } else {
                    Some(
                        w.replace("%%", "%")
                            .replace("%k", &path.to_string_lossy())
                            .replace("%c", id),
                    )
                }
            })
            .collect();
        let (program, args) = words.split_first().context("desktop Exec is empty")?;
        let mut command = Command::new(program);
        command
            .args(args)
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .env("QT_QPA_PLATFORM", "wayland")
            .env("QT_QUICK_BACKEND", "software")
            .env("GDK_BACKEND", "wayland")
            .env("GSK_RENDERER", "cairo")
            .env("GTK_CSD", "0")
            .stdin(Stdio::null());
        if x11 {
            command
                .env("DISPLAY", self.x11_display.as_ref().unwrap())
                .env("QT_QPA_PLATFORM", "xcb")
                .env("GDK_BACKEND", "x11")
                // The profile's Wayland input modules require a Wayland GDK/
                // QPA display. X11 children need their own platform context.
                .env("GTK_IM_MODULE", "xim")
                .env("QT_IM_MODULE", "compose");
            let decoration = if filename.starts_with("peony") {
                Some("peony")
            } else if filename.starts_with("wps-office-") {
                Some("wps")
            } else {
                None
            };
            if let Some(policy) = decoration {
                let library = std::env::var_os("XR_QT_DECORATION_LIBRARY")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| "/usr/lib/xr/wayland/libxr-qt-decoration.so".into());
                ensure!(
                    library.is_file(),
                    "XR Qt decoration adapter is not installed"
                );
                let mut preload = library.into_os_string();
                if let Some(existing) = std::env::var_os("LD_PRELOAD").filter(|v| !v.is_empty()) {
                    preload.push(" ");
                    preload.push(existing);
                }
                command
                    .env("XR_QT_EXTERNAL_CONTROLS", policy)
                    .env("LD_PRELOAD", preload);
            }
        }
        command.process_group(0);
        // Desktop applications must not inherit the XR display owner's ambient
        // capabilities: bubblewrap rejects them when GTK starts image loaders.
        unsafe {
            command.pre_exec(application::clear_capabilities);
        }
        let child = command.spawn()?;
        println!(
            "XR_WAYLAND_APP_START id={id} pid={} backend={}",
            child.id(),
            if x11 { "x11" } else { "wayland" }
        );
        self.children.push(child);
        Ok(())
    }
    pub fn send_frames(&mut self) {
        let time = self.started.elapsed().as_millis() as u32;
        let surfaces: Vec<_> = self.surfaces.values().map(|s| s.surface.clone()).collect();
        for wl in surfaces {
            with_states(&wl, |st| {
                for cb in st
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .frame_callbacks
                    .drain(..)
                {
                    cb.done(time);
                }
            });
        }
    }
    pub fn refresh(&mut self) {
        self.popups.cleanup();
        self.poll_clipboard();
        if self.x11_display.is_some() {
            for app in std::mem::take(&mut self.pending_x11_launches) {
                if let Err(error) = self.launch(&app, true) {
                    eprintln!("XR_XWAYLAND_APP_ERROR={error:#}");
                }
            }
        }
        let dead: Vec<_> = self
            .surfaces
            .values()
            .filter(|s| !s.surface.is_alive())
            .map(|s| s.id)
            .collect();
        for id in dead {
            self.remove(id);
        }
        self.children.retain_mut(|child| match child.try_wait() {
            Ok(Some(status)) => {
                println!("XR_WAYLAND_APP_EXIT pid={} status={status}", child.id());
                false
            }
            _ => true,
        });
    }
    fn poll_clipboard(&mut self) {
        let mut requests: Vec<_> = std::mem::take(&mut self.clipboard_requests)
            .into_iter()
            .map(|(ty, mime)| (ty, mime, "wayland"))
            .collect();
        requests.extend(
            std::mem::take(&mut self.x11_selections)
                .into_iter()
                .map(|(ty, mime)| (ty, mime, "x11")),
        );
        for (ty, mime, origin) in requests {
            let mut fds = [-1; 2];
            if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
                continue;
            }
            let reader = unsafe { OwnedFd::from_raw_fd(fds[0]) };
            let writer = unsafe { OwnedFd::from_raw_fd(fds[1]) };
            unsafe {
                libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK);
            }
            let request = if origin == "x11" {
                match (&mut self.xwm, &self.xwayland_handle) {
                    (Some(wm), Some(handle)) => wm
                        .send_selection(ty, mime.clone(), writer, handle.clone())
                        .is_ok(),
                    _ => false,
                }
            } else {
                match ty {
                    SelectionTarget::Clipboard => {
                        request_data_device_client_selection(&self.seat, mime.clone(), writer)
                            .is_ok()
                    }
                    SelectionTarget::Primary => {
                        request_primary_client_selection(&self.seat, mime.clone(), writer).is_ok()
                    }
                }
            };
            if request {
                self.clipboard_reads.push(ClipboardRead {
                    bytes: Vec::new(),
                    fd: std::fs::File::from(reader),
                    mime,
                    started: Instant::now(),
                    target: ty,
                    origin,
                });
            }
        }
        let mut completed = Vec::new();
        self.clipboard_reads.retain_mut(|read| {
            let mut chunk = [0u8; 4096];
            loop {
                match read.fd.read(&mut chunk) {
                    Ok(0) => {
                        if let Ok(text) = String::from_utf8(read.bytes.clone()) {
                            completed.push((read.target, read.mime.clone(), text, read.origin));
                        }
                        return false;
                    }
                    Ok(n) => {
                        read.bytes.extend_from_slice(&chunk[..n]);
                        if read.bytes.len() > 60000 {
                            return false;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        return read.started.elapsed() < std::time::Duration::from_secs(3);
                    }
                    Err(_) => return false,
                }
            }
        });
        for (target, mime, text, origin) in completed {
            if origin == "x11" {
                let types = vec!["text/plain;charset=utf-8".into(), "text/plain".into()];
                // Keep the original X11 owner; the Wayland side gets an immutable
                // copy and must not claim CLIPBOARD back through XWM here.
                match target {
                    SelectionTarget::Clipboard => {
                        set_data_device_selection(&self.dh, &self.seat, types, text.clone())
                    }
                    SelectionTarget::Primary => {
                        set_primary_selection(&self.dh, &self.seat, types, text.clone())
                    }
                }
            }
            if target == SelectionTarget::Clipboard {
                self.emit(
                    "ClipboardChanged",
                    json!({"mime_type":mime,"text":text,"origin":origin}),
                );
            }
        }
    }
    pub fn shutdown(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        // Only process groups created by this service are signalled. Child handles
        // are retained until wait, preventing their PID from being reused here.
        for child in &mut self.children {
            if matches!(child.try_wait(), Ok(None)) {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGTERM);
                }
            }
        }
        let deadline = Instant::now() + std::time::Duration::from_millis(500);
        while Instant::now() < deadline
            && self
                .children
                .iter_mut()
                .any(|c| matches!(c.try_wait(), Ok(None)))
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        for child in &mut self.children {
            if matches!(child.try_wait(), Ok(None)) {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
            }
            let _ = child.wait();
        }
        self.children.clear();
        let ids: Vec<_> = self.surfaces.keys().copied().collect();
        for id in ids {
            self.remove(id);
        }
        if !self.quit_requested {
            let _ = self.flush_bridge();
        }
        println!(
            "XR_WAYLAND_SUMMARY snapshots={} releases={}",
            self.snapshots_sent, self.releases
        );
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _: &WlBuffer) {}
}
impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        if let Some(data) = client.get_data::<ClientState>() {
            &data.compositor
        } else {
            &client
                .get_data::<smithay::xwayland::XWaylandClientData>()
                .unwrap()
                .compositor_state
        }
    }
    fn commit(&mut self, surface: &WlSurface) {
        self.popups.commit(surface);
        if is_sync_subsurface(surface) {
            return;
        }
        let mut all = Vec::new();
        with_surface_tree_downward(
            surface,
            (),
            |_, _, &()| TraversalAction::DoChildren(()),
            |wl, _, &()| all.push(wl.clone()),
            |_, _, &()| true,
        );
        for wl in all {
            self.capture_surface(&wl);
        }
    }
}
impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm
    }
}
impl OutputHandler for State {}
impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg
    }
    fn new_toplevel(&mut self, top: ToplevelSurface) {
        let id = self.register(top.wl_surface().clone(), "toplevel", None);
        self.surfaces.get_mut(&id).unwrap().top = Some(top.clone());
        top.with_pending_state(|s| {
            s.size = Some((800, 600).into());
            s.states.set(xdg_toplevel::State::Activated);
        });
        top.send_configure();
        self.update_toplevel(&top);
    }
    fn new_popup(&mut self, popup: PopupSurface, positioner: PositionerState) {
        let parent = popup
            .get_parent_surface()
            .as_ref()
            .and_then(|p| self.id_for(p));
        let id = self.register(popup.wl_surface().clone(), "popup", parent);
        let geometry = positioner.get_geometry();
        popup.with_pending_state(|s| s.geometry = geometry);
        if let Err(error) = popup.send_configure() {
            eprintln!("XR_WAYLAND_POPUP_CONFIGURE={error}");
        }
        let _ = self.popups.track_popup(PopupKind::Xdg(popup.clone()));
        let origin = popup
            .get_parent_surface()
            .map(|parent| {
                with_states(&parent, |st| {
                    st.cached_state
                        .get::<SurfaceCachedState>()
                        .current()
                        .geometry
                        .map(|r| r.loc)
                        .unwrap_or_default()
                })
            })
            .unwrap_or_default();
        let s = self.surfaces.get_mut(&id).unwrap();
        s.popup = Some(popup);
        s.width = geometry.size.w;
        s.height = geometry.size.h;
        s.popup_position = (geometry.loc.x + origin.x, geometry.loc.y + origin.y);
        self.window_state(id);
    }
    fn grab(&mut self, popup: PopupSurface, _: WlSeat, serial: Serial) {
        let Some(mut root) = popup.get_parent_surface() else {
            return;
        };
        while let Some(parent) = self
            .id_for(&root)
            .and_then(|id| self.surfaces.get(&id))
            .and_then(|s| s.parent)
            .and_then(|id| self.surfaces.get(&id))
            .map(|s| s.surface.clone())
        {
            root = parent;
        }
        let seat = self.seat.clone();
        match self
            .popups
            .grab_popup(root.into(), PopupKind::Xdg(popup), &seat, serial)
        {
            Ok(grab) => {
                if let Some(keyboard) = seat.get_keyboard() {
                    keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
                }
                if let Some(pointer) = seat.get_pointer() {
                    pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
                }
            }
            Err(error) => eprintln!("XR_WAYLAND_POPUP_GRAB={error:?}"),
        }
    }
    fn reposition_request(&mut self, popup: PopupSurface, positioner: PositionerState, token: u32) {
        let geometry = positioner.get_geometry();
        popup.with_pending_state(|s| s.geometry = geometry);
        popup.send_repositioned(token);
        let origin = popup
            .get_parent_surface()
            .map(|parent| {
                with_states(&parent, |st| {
                    st.cached_state
                        .get::<SurfaceCachedState>()
                        .current()
                        .geometry
                        .map(|r| r.loc)
                        .unwrap_or_default()
                })
            })
            .unwrap_or_default();
        if let Some(id) = self.id_for(popup.wl_surface()) {
            let s = self.surfaces.get_mut(&id).unwrap();
            s.popup_position = (geometry.loc.x + origin.x, geometry.loc.y + origin.y);
            self.window_state(id);
        }
    }
    fn ack_configure(&mut self, wl: WlSurface, configure: Configure) {
        if let Some(id) = self.id_for(&wl) {
            let serial = match configure {
                Configure::Toplevel(c) => c.serial,
                Configure::Popup(c) => c.serial,
            };
            self.surfaces.get_mut(&id).unwrap().ack = serial.into();
            self.window_state(id);
        }
    }
    fn toplevel_destroyed(&mut self, top: ToplevelSurface) {
        if let Some(id) = self.id_for(top.wl_surface()) {
            self.remove(id);
        }
    }
    fn popup_destroyed(&mut self, popup: PopupSurface) {
        if let Some(id) = self.id_for(popup.wl_surface()) {
            self.remove(id);
        }
    }
    fn app_id_changed(&mut self, top: ToplevelSurface) {
        self.update_toplevel(&top);
    }
    fn title_changed(&mut self, top: ToplevelSurface) {
        self.update_toplevel(&top);
    }
    fn parent_changed(&mut self, top: ToplevelSurface) {
        self.update_toplevel(&top);
    }
}
impl SeatHandler for State {
    type KeyboardFocus = KeyboardFocus;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
    fn focus_changed(&mut self, seat: &Seat<Self>, focus: Option<&KeyboardFocus>) {
        let surface = focus.and_then(WaylandFocus::wl_surface);
        let focus = surface.as_deref();
        set_data_device_focus(&self.dh, seat, focus.and_then(|s| s.client()));
        set_primary_focus(&self.dh, seat, focus.and_then(|s| s.client()));
        seat.text_input().leave();
        seat.text_input().set_focus(focus.cloned());
        seat.text_input().enter();
    }
    fn cursor_image(&mut self, _: &Seat<Self>, _: smithay::input::pointer::CursorImageStatus) {}
}
impl SelectionHandler for State {
    type SelectionUserData = String;
    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _: Seat<Self>,
    ) {
        if let Some(source) = source {
            let types = source.mime_types();
            if let Some(wm) = &mut self.xwm {
                let _ = wm.new_selection(ty, Some(types.clone()));
            }
            if let Some(mime) = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"]
                .iter()
                .find(|m| types.iter().any(|t| t == **m))
            {
                self.clipboard_requests.push((ty, (*mime).into()));
            }
        } else if let Some(wm) = &mut self.xwm {
            let _ = wm.new_selection(ty, None);
        }
    }
    fn send_selection(
        &mut self,
        _: SelectionTarget,
        _: String,
        fd: OwnedFd,
        _: Seat<Self>,
        data: &String,
    ) {
        let bytes = data.clone().into_bytes();
        std::thread::spawn(move || {
            let _ = std::fs::File::from(fd).write_all(&bytes);
        });
    }
}
impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device
    }
}
impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {
    fn send(&mut self, _: String, _: OwnedFd, _: Seat<Self>) {}
}
impl PrimarySelectionHandler for State {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary
    }
}
impl InputMethodHandler for State {
    fn new_popup(&mut self, popup: ImePopup) {
        let parent = popup.get_parent().and_then(|p| self.id_for(&p.surface));
        let id = self.register(popup.wl_surface().clone(), "ime_popup", parent);
        let s = self.surfaces.get_mut(&id).unwrap();
        s.popup_position = popup.location().into();
        self.window_state(id);
    }
    fn dismiss_popup(&mut self, popup: ImePopup) {
        if let Some(id) = self.id_for(popup.wl_surface()) {
            self.remove(id);
        }
    }
    fn popup_repositioned(&mut self, popup: ImePopup) {
        if let Some(id) = self.id_for(popup.wl_surface()) {
            self.surfaces.get_mut(&id).unwrap().popup_position = popup.location().into();
            self.window_state(id);
        }
    }
    fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, Logical> {
        self.id_for(parent)
            .and_then(|id| self.surfaces.get(&id))
            .map(|s| Rectangle::from_size((s.width, s.height).into()))
            .unwrap_or_default()
    }
}
delegate_compositor!(State);
delegate_shm!(State);
delegate_xdg_shell!(State);
delegate_seat!(State);
delegate_output!(State);
delegate_data_device!(State);
delegate_primary_selection!(State);
delegate_text_input_manager!(State);
delegate_input_method_manager!(State);

delegate_virtual_keyboard_manager!(State);

// Negotiation transfers traditional decoration responsibility away from clients.
// Shell supplies external spatial controls instead of drawing a titlebar/frame.
impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        self.server_side_decoration(toplevel);
    }
    fn request_mode(&mut self, toplevel: ToplevelSurface, _: DecorationMode) {
        self.server_side_decoration(toplevel);
    }
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.server_side_decoration(toplevel);
    }
}
impl State {
    fn server_side_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel
            .with_pending_state(|state| state.decoration_mode = Some(DecorationMode::ServerSide));
        toplevel.send_configure();
        println!(
            "XR_WAYLAND_DECORATION surface={} mode=server_side traditional_frame=false",
            self.id_for(toplevel.wl_surface())
                .map(|id| id.to_string())
                .unwrap_or_else(|| "unregistered".into())
        );
    }
}
delegate_xdg_decoration!(State);

// GTK 3.24's Wayland backend checks this legacy manager's default mode,
// not xdg-decoration or GTK_CSD, before creating its fallback headerbar.
impl KdeDecorationHandler for State {
    fn kde_decoration_state(&self) -> &KdeDecorationState {
        &self.kde_decoration
    }
    fn new_decoration(&mut self, surface: &WlSurface, decoration: &OrgKdeKwinServerDecoration) {
        self.kde_mode_requests
            .insert(decoration.id(), KdeMode::Server as u32);
        decoration.mode(KdeMode::Server);
        println!(
            "XR_WAYLAND_DECORATION surface={} protocol=kde mode=server_side traditional_frame=false",
            self.id_for(surface)
                .map(|id| id.to_string())
                .unwrap_or_else(|| "unregistered".into())
        );
    }
    fn request_mode(
        &mut self,
        _: &WlSurface,
        decoration: &OrgKdeKwinServerDecoration,
        mode: smithay::reexports::wayland_server::WEnum<KdeMode>,
    ) {
        let requested = match mode {
            smithay::reexports::wayland_server::WEnum::Value(mode) => mode as u32,
            smithay::reexports::wayland_server::WEnum::Unknown(value) => value,
        };
        // A client may acknowledge by repeating a request. Do not produce an
        // endless request/event loop if it insists on the same preferred mode.
        if self.kde_mode_requests.insert(decoration.id(), requested) != Some(requested) {
            decoration.mode(KdeMode::Server);
        }
    }
    fn release(&mut self, decoration: &OrgKdeKwinServerDecoration, _: &WlSurface) {
        self.kde_mode_requests.remove(&decoration.id());
    }
}
delegate_kde_decoration!(State);
