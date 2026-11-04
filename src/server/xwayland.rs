// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use smithay::{
    delegate_xwayland_shell,
    reexports::calloop::EventLoop,
    wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    xwayland::{
        X11Surface, X11Wm, XWayland, XWaylandEvent,
        xwm::{Reorder, ResizeEdge, WmWindowProperty, XwmHandler, XwmId},
    },
};

impl State {
    pub fn start_xwayland(&mut self) -> Result<()> {
        let event_loop = EventLoop::<Self>::try_new()?;
        let handle = event_loop.handle();
        let (xwayland, client) = XWayland::spawn(
            &self.dh,
            None,
            [("XWAYLAND_NO_GLAMOR", "1")],
            false,
            Stdio::null(),
            Stdio::inherit(),
            |_| (),
        )?;
        let wm_handle = handle.clone();
        handle
            .insert_source(xwayland, move |event, _, state| match event {
                XWaylandEvent::Ready {
                    x11_socket,
                    display_number,
                } => match X11Wm::start_wm(wm_handle.clone(), x11_socket, client.clone()) {
                    Ok(wm) => {
                        state.xwm = Some(wm);
                        state.x11_display = Some(format!(":{display_number}"));
                        println!("XR_XWAYLAND_DISPLAY=:{display_number}");
                    }
                    Err(error) => eprintln!("XR_XWAYLAND_WM_ERROR={error}"),
                },
                XWaylandEvent::Error => eprintln!("XR_XWAYLAND_START_FAILED=1"),
            })
            .map_err(|e| anyhow::anyhow!("insert Xwayland source: {e}"))?;
        self.xwayland_handle = Some(handle);
        self.xwayland_loop = Some(event_loop);
        Ok(())
    }
    pub fn dispatch_xwayland(&mut self) -> Result<()> {
        if let Some(mut event_loop) = self.xwayland_loop.take() {
            let result = event_loop.dispatch(Some(std::time::Duration::ZERO), self);
            self.xwayland_loop = Some(event_loop);
            result?;
        }
        Ok(())
    }
    fn associate_x11(&mut self, wl: WlSurface, window: X11Surface) {
        let parent = window.is_transient_for().and_then(|parent_xid| {
            self.surfaces
                .values()
                .find(|s| s.x11.as_ref().is_some_and(|x| x.window_id() == parent_xid))
                .map(|s| s.id)
        });
        let geometry = window.geometry();
        let parent_origin = parent
            .and_then(|id| self.surfaces.get(&id))
            .and_then(|s| s.x11.as_ref())
            .map(|s| s.geometry().loc)
            .unwrap_or_default();
        let id = self.register(wl.clone(), "xwayland", parent);
        let s = self.surfaces.get_mut(&id).unwrap();
        s.app_id = window.class();
        s.title = window.title();
        s.width = geometry.size.w.max(1);
        s.height = geometry.size.h.max(1);
        s.parent = parent;
        if parent.is_some() {
            s.popup_position = (
                geometry.loc.x - parent_origin.x,
                geometry.loc.y - parent_origin.y,
            );
        }
        s.x11 = Some(window);
        self.window_state(id);
        self.capture_surface(&wl);
    }
}
impl State {
    fn remove_x11_window(&mut self, window: &X11Surface) {
        // The XWM may detach wl_surface before UnmapNotify or DestroyNotify.
        // Match our own records by the stable XID instead.
        let ids: Vec<_> = self
            .surfaces
            .values()
            .filter(|s| {
                s.x11
                    .as_ref()
                    .is_some_and(|x| x.window_id() == window.window_id())
            })
            .map(|s| s.id)
            .collect();
        for id in ids {
            self.remove(id);
        }
    }
}
impl XWaylandShellHandler for State {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell
    }
    fn surface_associated(&mut self, _: XwmId, wl: WlSurface, window: X11Surface) {
        self.associate_x11(wl, window);
    }
}
impl XwmHandler for State {
    fn xwm_state(&mut self, _: XwmId) -> &mut X11Wm {
        self.xwm.as_mut().expect("active XWM")
    }
    fn new_window(&mut self, _: XwmId, window: X11Surface) {
        self.x11_windows.push(window);
    }
    fn new_override_redirect_window(&mut self, _: XwmId, window: X11Surface) {
        self.x11_windows.push(window);
    }
    fn map_window_request(&mut self, _: XwmId, window: X11Surface) {
        let mut geometry = window.geometry();
        if geometry.size.w < 64 {
            geometry.size.w = 800;
        }
        if geometry.size.h < 64 {
            geometry.size.h = 600;
        }
        if let Err(error) = window.configure(Some(geometry)) {
            eprintln!("XR_XWM_CONFIGURE={error}");
        }
        if let Err(error) = window.set_mapped(true) {
            eprintln!("XR_XWM_MAP={error}");
        }
        if let Some(wl) = window.wl_surface() {
            self.associate_x11(wl, window);
        }
    }
    fn mapped_override_redirect_window(&mut self, _: XwmId, window: X11Surface) {
        if let Some(wl) = window.wl_surface() {
            self.associate_x11(wl, window);
        }
    }
    fn unmapped_window(&mut self, _: XwmId, window: X11Surface) {
        self.remove_x11_window(&window);
    }
    fn destroyed_window(&mut self, _: XwmId, window: X11Surface) {
        self.x11_windows.retain(|w| w != &window);
        self.remove_x11_window(&window);
    }
    fn configure_request(
        &mut self,
        _: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _: Option<Reorder>,
    ) {
        let mut geometry = window.geometry();
        if let Some(x) = x {
            geometry.loc.x = x;
        }
        if let Some(y) = y {
            geometry.loc.y = y;
        }
        if let Some(w) = w {
            geometry.size.w = w.clamp(1, 8192) as i32;
        }
        if let Some(h) = h {
            geometry.size.h = h.clamp(1, 8192) as i32;
        }
        if let Err(error) = window.configure(Some(geometry)) {
            eprintln!("XR_XWM_CONFIGURE={error}");
        }
    }
    fn configure_notify(
        &mut self,
        _: XwmId,
        window: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _: Option<u32>,
    ) {
        if let Some(wl) = window.wl_surface() {
            if let Some(id) = self.id_for(&wl) {
                let s = self.surfaces.get_mut(&id).unwrap();
                s.configured = (geometry.size.w, geometry.size.h);
                self.window_state(id);
            }
        }
    }
    fn property_notify(&mut self, _: XwmId, window: X11Surface, _: WmWindowProperty) {
        if let Some(wl) = window.wl_surface() {
            if let Some(id) = self.id_for(&wl) {
                let s = self.surfaces.get_mut(&id).unwrap();
                s.app_id = window.class();
                s.title = window.title();
                self.window_state(id);
            }
        }
    }
    fn allow_selection_access(&mut self, _: XwmId, _: SelectionTarget) -> bool {
        true
    }
    fn send_selection(&mut self, _: XwmId, target: SelectionTarget, mime: String, fd: OwnedFd) {
        let text = match target {
            SelectionTarget::Clipboard => {
                current_data_device_selection_userdata(&self.seat).map(|s| s.clone())
            }
            SelectionTarget::Primary => {
                current_primary_selection_userdata(&self.seat).map(|s| s.clone())
            }
        };
        if let Some(text) = text {
            std::thread::spawn(move || {
                let _ = std::fs::File::from(fd).write_all(text.as_bytes());
            });
        } else {
            let result = match target {
                SelectionTarget::Clipboard => {
                    request_data_device_client_selection(&self.seat, mime, fd)
                        .map_err(|e| e.to_string())
                }
                SelectionTarget::Primary => request_primary_client_selection(&self.seat, mime, fd)
                    .map_err(|e| e.to_string()),
            };
            if let Err(error) = result {
                eprintln!("XR_XWM_SELECTION_ERROR={error}");
            }
        }
    }
    fn new_selection(&mut self, _: XwmId, target: SelectionTarget, types: Vec<String>) {
        if let Some(mime) = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"]
            .iter()
            .find(|m| types.iter().any(|t| t == **m))
        {
            self.x11_selections.push((target, (*mime).into()));
        }
    }
    fn cleared_selection(&mut self, _: XwmId, target: SelectionTarget) {
        // The next real owner drives the corresponding seat selection update.
        self.x11_selections.retain(|(ty, _)| *ty != target);
    }
    fn resize_request(&mut self, _: XwmId, _: X11Surface, _: u32, _: ResizeEdge) {}
    fn move_request(&mut self, _: XwmId, _: X11Surface, _: u32) {}
}
delegate_xwayland_shell!(State);
