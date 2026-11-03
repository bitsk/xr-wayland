// SPDX-License-Identifier: GPL-3.0-or-later
mod bridge;
mod server;
use anyhow::{Context, Result, bail};
use smithay::reexports::wayland_server::{Display, ListeningSocket};
use std::{
    env,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

struct Options {
    socket_name: String,
    shell_socket: PathBuf,
    seconds: u64,
    launch: Vec<String>,
    launch_x11: Vec<String>,
    xwayland: bool,
}
fn options() -> Result<Options> {
    let runtime = env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is required")?;
    let mut o = Options {
        socket_name: "xr-wayland-0".into(),
        shell_socket: PathBuf::from(runtime).join("xr-desktop/shell.sock"),
        seconds: 0,
        launch: Vec::new(),
        launch_x11: Vec::new(),
        xwayland: false,
    };
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket-name" => o.socket_name = args.next().context("missing socket name")?,
            "--shell-socket" => {
                o.shell_socket = args.next().context("missing Shell socket")?.into()
            }
            "--seconds" => o.seconds = args.next().context("missing duration")?.parse()?,
            "--launch" => o
                .launch
                .push(args.next().context("missing desktop app ID")?),
            "--xwayland" => o.xwayland = true,
            "--launch-x11" => o
                .launch_x11
                .push(args.next().context("missing X11 desktop ID")?),
            "--help" | "-h" => {
                println!(
                    "xr-window-server [--socket-name NAME] [--shell-socket PATH] [--seconds N] [--launch DESKTOP_ID] [--xwayland --launch-x11 DESKTOP_ID]"
                );
                std::process::exit(0);
            }
            _ => bail!("unknown option {arg}"),
        }
    }
    anyhow::ensure!(
        !o.socket_name.contains('/') && !o.socket_name.is_empty(),
        "socket name must be a basename"
    );
    Ok(o)
}
fn main() {
    if let Err(error) = run() {
        eprintln!("XR_WAYLAND_ERROR={error:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let o = options()?;
    let bridge = bridge::Bridge::connect(&o.shell_socket)?;
    let mut display = Display::<server::State>::new()?;
    let listener = ListeningSocket::bind(&o.socket_name)?;
    let mut state = server::State::new(display.handle(), bridge, o.socket_name.clone())?;
    let quit = Arc::new(AtomicBool::new(false));
    let signal = quit.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))?;
    println!(
        "XR_WAYLAND_DISPLAY={} GENERATION={}",
        o.socket_name, state.bridge.generation
    );
    if o.xwayland {
        state.start_xwayland()?;
    }
    for app in o.launch {
        state.launch(&app, false)?;
    }
    for app in o.launch_x11 {
        state.launch(&app, true)?;
    }
    let start = Instant::now();
    let mut last_frame = Instant::now();
    while !quit.load(Ordering::Relaxed)
        && (o.seconds == 0 || start.elapsed() < Duration::from_secs(o.seconds))
    {
        while let Some(stream) = listener.accept()? {
            display
                .handle()
                .insert_client(stream, Arc::new(server::ClientState::default()))?;
        }
        state.dispatch_bridge()?;
        if state.quit_requested {
            break;
        }
        state.dispatch_xwayland()?;
        display.dispatch_clients(&mut state)?;
        state.flush_bridge()?;
        if last_frame.elapsed() >= Duration::from_millis(16) {
            state.send_frames();
            last_frame = Instant::now();
        }
        state.refresh();
        display.flush_clients()?;
        std::thread::sleep(Duration::from_millis(2));
    }
    state.shutdown();
    drop(listener);
    drop(display);
    drop(state);
    println!("XR_WAYLAND_CLEANUP=1");
    Ok(())
}
