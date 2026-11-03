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
