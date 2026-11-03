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
Use a private session bus when an application has a DBus single-instance path.
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
clipboard and primary selections use the XWM selection FD protocol and the real
Wayland seat selection. Text payloads are bounded to 60 kB; unfinished transfers
expire after 3 seconds. Input-method-v2 keyboard grabs are paired with the
virtual-keyboard-v1 protocol for unconsumed keys. Shell control errors are
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

Control messages carrying `origin_component_id` and `origin_request_id` receive
`RequestResult` only after the Wayland owner handles them. A rejected focus,
missing surface or inactive text-input-v3 object produces `success:false` with
a reason. A successful result confirms protocol delivery, not that a client
rendered the resulting text; target acceptance must inspect the actual client.
`FocusChanged` keeps its separate `FocusApplied` acknowledgement.

## Debian package

The independent `xr-window-server` package depends on xr-shell >= 0.2.0
and the library versions detected by dpkg-shlibdeps. Xwayland is recommended
for the optional X11 path. No package script enables services or opens displays.

`tools/build.py` selects the exact xr-shell-protocols 0.2.0 SDK from
`XR_SHELL_PROTOCOLS_DIR`, the sibling checkout or the installed source SDK in
`/usr/share/cargo/registry`. Cargo.lock pins Smithay 0.7.0 and the complete
transitive dependency set. Populate the Cargo cache before an offline build.
Build with `dpkg-buildpackage -b -us -uc` after installing Build-Depends.
The package includes original third-party license texts and an SPDX index.

Shell `SessionEnding` is consumed before another write or EOF read and exits
through the normal cleanup path with status zero. An unannounced transport
failure remains an error. This does not change the SIGINT/SIGTERM or timer paths.

## Spatial window decoration policy

The private display advertises zxdg_decoration_manager_v1. New decorations,
client mode requests and mode resets are configured as ServerSide, transferring
traditional decoration responsibility to the compositor. This service draws no
traditional titlebar or rectangle frame; Shell owns external spatial grab/orb
controls. The entire client texture and its input coordinate system are retained.

Only applications launched by this service receive GTK_CSD=0. This environment
setting cannot remove a custom application GtkHeaderBar. Applications that do
not use xdg-decoration or paint their own chrome require a real application or
toolkit adaptation; negotiation is not proof their texture is decoration-free.
The existing desktop and global GTK/Qt settings are not changed.

GTK 3.24's Wayland backend specifically checks the legacy KDE decoration
manager's default Server mode, so that compatibility global is also advertised.
See [actual toolkit paths and capability boundaries](docs/decoration-policy.md).

X11 application children explicitly use `GTK_IM_MODULE=xim` and
`QT_IM_MODULE=compose` together with the private Xwayland DISPLAY. This avoids
inheriting a Wayland-only input module from the XR profile. Native Wayland
children retain the profile's input-method environment. XIM requires a reachable
input-method server for language conversion; selecting the context alone does
not establish CJK preedit or committed-text acceptance.
