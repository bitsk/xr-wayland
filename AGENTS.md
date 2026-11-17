# XR Wayland window service

Maintain the real private Wayland/Xwayland application server here. Consume
xr-shell's protocol SDK rather than copying its wire implementation. Own
wl_surface, configure/ack, seat serials, buffer snapshots and selection/IME
protocols; Shell owns spatial focus and transforms. Copy application shm before
releasing its wl_buffer; export immutable sealed memfd snapshots with SCM_RIGHTS.

Keep complete Git history with the existing contributor identity and real dates.
Package installation must not launch a competing desktop or modify runtime
selection. Public files contain no private paths, addresses or credentials.
Keep cross-component architecture in the documentation repository. Do not create
provenance directories. Record build, actual QEMU and physical validation separately.

## XR Spatial Engineering Constraints

Follow `openkylin-xr-docs/wiki/xr-spatial-guidelines-and-issues.md`:
- **Spatial DPI / `dmm` Output Scaling**: Coordinate `wl_output` logical geometry and buffer scale with Shell's angular density (`dmm`) so 2D Wayland/Xwayland surfaces render sharp text at their physical meter size in 3D space.
- **Subsurface & Popup Spatial Hierarchy**: Preserve parent-relative offsets and depth layering metadata so popups and IME candidates project onto the parent window's 3D oriented/curved frame with matching hit-test geometry.
