# Application decoration policy

XR windows use their complete application surface, with external spatial grab
and orb controls supplied by Shell. The window service draws no traditional
server titlebar or rectangular frame and does not crop application textures.
The private display negotiates ServerSide through xdg-decoration and advertises
Server as the default through the legacy KDE server-decoration protocol.
GTK_CSD=0 is set only in environments of applications launched by this service.

The development guest has Mousepad 0.6.3, GTK 3.24.52 and Qt Wayland 5.15.10.
Its installed `org.xfce.mousepad.preferences.window` schema defines boolean
`client-side-decorations`, default false, requiring Mousepad restart after a
change. A read-only query under the guest root account also returned false.
No GSettings value was modified. A different application account or a private
GSettings backend can have different effective preferences.

GTK 3.24.52's Wayland path in `gtk_window_should_use_csd` directly checks
`gdk_wayland_display_prefers_ssd`. That implementation checks the legacy KDE
manager's default Server mode. GTK_CSD=0 and xdg-decoration alone therefore do
not establish a decoration-free GTK3 window. The legacy manager is deliberately
advertised for this real toolkit path. Explicit custom GtkHeaderBar widgets can
still force CSD and need application-level support; no generic texture cropping
or suppression of application controls is performed.

Qt 5.15's xdg-shell backend creates an xdg-decoration object for normal decorated
windows and stops toolkit decoration when configured ServerSide. Applications
painting their own widget titlebar can retain it. The development guest contains
`libqt5-ukui-platformtheme.so`, provided by `qt5-ukui-platformtheme`; presence
alone does not prove successful plugin loading. This service does not force
QT_QPA_PLATFORMTHEME or change the existing desktop's theme configuration.

Build/packaging success proves these protocol implementations compile. Actual
GTK/Qt surface captures and pointer/keyboard/popup checks in the isolated XR
profile are required before declaring the content surfaces decoration-free.

Primary implementation references:

- [GTK 3.24.52 GtkWindow](https://github.com/GNOME/gtk/blob/3.24.52/gtk/gtkwindow.c)
- [GTK 3.24.52 Wayland display](https://github.com/GNOME/gtk/blob/3.24.52/gdk/wayland/gdkdisplay-wayland.c)
- [Qt 5.15 xdg-shell backend](https://github.com/qt/qtwayland/blob/5.15/src/plugins/shellintegration/xdg-shell/qwaylandxdgshell.cpp)
- [Smithay 0.7 xdg decoration](https://docs.rs/smithay/0.7.0/smithay/wayland/shell/xdg/decoration/index.html)
- [Smithay 0.7 KDE decoration](https://docs.rs/smithay/0.7.0/smithay/wayland/shell/kde/decoration/index.html)
