// SPDX-License-Identifier: GPL-3.0-or-later
use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    os::fd::{AsFd, OwnedFd},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
};
use xr_shell_protocols::Client;

pub struct Outgoing {
    pub kind: &'static str,
    pub payload: Value,
    pub fd: Option<OwnedFd>,
}
pub enum Incoming {
    Packet(Value),
    Disconnected(String),
}
pub struct Bridge {
    pub outgoing: SyncSender<Outgoing>,
    pub incoming: Receiver<Incoming>,
    pub generation: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Bridge {
    pub fn connect(path: &Path) -> Result<Self> {
        let mut client = Client::connect(
            path,
            "window_server",
            &["core.v1", "shm.v1", "focus.v1", "window.v1"],
        )?;
        let generation = client.welcome["component_generation"]
            .as_str()
            .context("missing generation")?
            .to_owned();
        let (outgoing, commands) = mpsc::sync_channel::<Outgoing>(8);
        let (events, incoming) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let thread = thread::spawn(move || {
            let result = (|| -> Result<()> {
                while !worker_stop.load(Ordering::Relaxed) {
                    if let Ok(command) = commands.try_recv() {
                        let fds: Vec<_> = command.fd.iter().map(|fd| fd.as_fd()).collect();
                        client.send(command.kind, command.payload, &fds)?;
                    }
                    while let Some(packet) = client.recv(0)? {
                        events.send(Incoming::Packet(packet.message))?;
                    }
                    thread::sleep(std::time::Duration::from_millis(1));
                }
                Ok(())
            })();
            if let Err(error) = result {
                let _ = events.send(Incoming::Disconnected(format!("{error:#}")));
            }
        });
        Ok(Self {
            outgoing,
            incoming,
            generation,
            stop,
            thread: Some(thread),
        })
    }
    pub fn try_send(&self, command: Outgoing) -> std::result::Result<(), TrySendError<Outgoing>> {
        self.outgoing.try_send(command)
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
