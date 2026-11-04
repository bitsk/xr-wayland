// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use smithay::{
    input::keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
    utils::IsAlive,
    xwayland::X11Surface,
};
use std::borrow::Cow;

/// X11Surface negotiates ICCCM input focus as well as wl_keyboard delivery.
/// Retain the Wayland identity for selections and native popup grabs.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyboardFocus {
    pub(super) surface: WlSurface,
    pub(super) x11: Option<X11Surface>,
}
impl From<WlSurface> for KeyboardFocus {
    fn from(surface: WlSurface) -> Self {
        Self { surface, x11: None }
    }
}
impl From<PopupKind> for KeyboardFocus {
    fn from(popup: PopupKind) -> Self {
        WlSurface::from(popup).into()
    }
}
impl From<KeyboardFocus> for WlSurface {
    fn from(focus: KeyboardFocus) -> Self {
        focus.surface
    }
}
impl IsAlive for KeyboardFocus {
    fn alive(&self) -> bool {
        self.surface.alive()
    }
}
impl WaylandFocus for KeyboardFocus {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        Some(Cow::Borrowed(&self.surface))
    }
}
impl KeyboardTarget<State> for KeyboardFocus {
    fn enter(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match &self.x11 {
            Some(w) => KeyboardTarget::enter(w, seat, data, keys, serial),
            None => KeyboardTarget::enter(&self.surface, seat, data, keys, serial),
        }
    }
    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {
        match &self.x11 {
            Some(w) => KeyboardTarget::leave(w, seat, data, serial),
            None => KeyboardTarget::leave(&self.surface, seat, data, serial),
        }
    }
    fn key(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        match &self.x11 {
            Some(w) => KeyboardTarget::key(w, seat, data, key, state, serial, time),
            None => KeyboardTarget::key(&self.surface, seat, data, key, state, serial, time),
        }
    }
    fn modifiers(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        match &self.x11 {
            Some(w) => KeyboardTarget::modifiers(w, seat, data, modifiers, serial),
            None => KeyboardTarget::modifiers(&self.surface, seat, data, modifiers, serial),
        }
    }
}
