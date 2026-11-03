# xr-wayland

Private Smithay-based Wayland application window service for UKUI XR. The
`xr-window-server` executable owns Wayland surfaces, configure serials and seat
input. It sends immutable application SHM snapshots through xr-shell's protocol
SDK; it creates no OpenXR session and acquires no physical DRM device.

## Build and launch

Build with Rust 2024, libxkbcommon development files, the checked-in Cargo.lock
and the fixed sibling `xr-shell/protocols` SDK source:

```sh
cargo build --release --locked --offline
xr-window-server --shell-socket "$XDG_RUNTIME_DIR/xr-desktop/shell.sock" \
  --socket-name xr-wayland-0 --launch qterminal
```

`--launch` selects an installed `.desktop` basename. The desktop Exec is parsed
into argv, never evaluated by a shell. Per-application environment selects the
private Wayland display and software rendering for the initial SHM path.
Applications may also be started externally with the advertised WAYLAND_DISPLAY.
Package installation must not start this server or change the existing desktop.

Current implementation has private Wayland, xdg toplevel/popup state,
configure/ack/resize/close, pointer/keyboard seat, SHM snapshots, standard data
selection and text-input-v3/input-method-v2 globals. Actual application, popup,
clipboard and IME behaviour must be checked in the target system; compilation
alone does not establish those acceptance results.

For an independently owned Xwayland instance and a real X11 application:

```sh
xr-window-server --xwayland --launch-x11 qterminal \
  --shell-socket "$XDG_RUNTIME_DIR/xr-desktop/shell.sock"
```

The service starts Xwayland with software SHM transport and prints its private
DISPLAY only after the XWM is ready. Native and X11 launch arguments may be
combined. X11 map/configure/close and title/class changes use the real XWM. X11
selection forwarding is not part of this checkpoint. Shell control errors are
reported without terminating the Wayland application service. Service exit
terminates and reaps only the child process groups it launched; externally
started clients are disconnected when the private display closes.

## Ownership

The service copies `wl_buffer` contents before sending wl_buffer.release. A
separate SDK worker transmits the owned, write-sealed memfd snapshot with
SCM_RIGHTS. Its bounded queue and per-surface pending snapshot avoid blocking the
Wayland dispatch loop on a slow renderer; superseded unsent snapshots drop
locally. Shell BufferRelease is a different lifetime from application wl_buffer
release. Only argb8888 premultiplied and xrgb8888 opaque SHM are accepted initially.
DMA-BUF is not advertised by this version.

Pointer coordinates and popup offsets are parent wl_surface logical pixels,
without Shell decoration. Key input is Linux evdev; this owner adds 8 for xkb.
Spatial focus is acknowledged using Shell's focus_epoch. Window position in
metres and workspaces belong to Shell. The service does not hard-code a workspace
on repeated metadata updates.

Artwork and renderer/runtime code remain in their owning repositories. Cross-
component architecture and acceptance records are maintained by the integration
project. No physical-device support is inferred from this application service.
