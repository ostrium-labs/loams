//! The worker's half of the forwarder (L3, §49 §13.2; HS1 Task 6, R1.11).
//!
//! In the `netns` sandbox the worker's network namespace has only `lo`, so the
//! one way out is this relay: it listens on `127.0.0.1:FORWARDER_PORT` inside the
//! namespace (the endpoint the worker's `READ ON S3` grant names) and, for each
//! connection chDB makes, asks the front for a stream over the socket the worker
//! inherited on fd 4. The front answers with one end of a fresh socket pair
//! (`SCM_RIGHTS`) and serves the other end: today it relays to the configured
//! upstream; from HS1 Task 9 it is `house-cache`, which knows the worker by the
//! socket it came on. The forwarder holds no credential and can open nothing the
//! front does not hand it.
//!
//! It is a thread of the worker rather than a process of its own: it has to be in
//! the worker's network namespace, which only the worker can make (it is
//! unprivileged), and once the sandbox is in force nothing can start a process.
//! It has no privilege the worker lacks, and a `SIGKILL` of the worker ends it.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::thread::JoinHandle;

/// The byte the worker sends to ask for a stream.
pub const REQUEST: u8 = b'c';

/// The running forwarder.
#[derive(Debug)]
pub struct Forwarder {
    port: u16,
    _thread: JoinHandle<()>,
}

impl Forwarder {
    /// Serves `listener` on a thread, asking the front for a stream over
    /// `channel` for each connection.
    pub fn start(listener: TcpListener, channel: UnixStream) -> io::Result<Self> {
        let port = listener.local_addr()?.port();
        let thread = std::thread::Builder::new()
            .name("forwarder".to_string())
            .spawn(move || serve(&listener, &channel))?;
        Ok(Self {
            port,
            _thread: thread,
        })
    }

    /// The loopback port it listens on.
    pub fn port(&self) -> u16 {
        self.port
    }
}

fn serve(listener: &TcpListener, channel: &UnixStream) {
    for conn in listener.incoming() {
        let Ok(conn) = conn else {
            continue;
        };
        let mut writer = channel;
        if writer.write_all(&[REQUEST]).is_err() {
            // The front is gone; the worker exits through its `hsw1` socket.
            return;
        }
        match loams_chdb_sys::inherited::receive_socket(channel) {
            Ok(Some(fd)) => relay(conn, UnixStream::from(fd)),
            // The front closed the channel.
            Ok(None) => return,
            // A message the worker cannot use: this connection fails, the next
            // one asks again.
            Err(_) => drop(conn),
        }
    }
}

/// Copies both ways between the connection chDB made and the front's stream,
/// each direction on a thread of its own, ending each side's writes at the
/// other's end of data.
fn relay(conn: TcpStream, stream: UnixStream) {
    let (Ok(conn_back), Ok(stream_back)) = (conn.try_clone(), stream.try_clone()) else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("forwarder-out".to_string())
        .spawn(move || copy(conn, stream, |s| s.shutdown(Shutdown::Write)));
    let _ = std::thread::Builder::new()
        .name("forwarder-in".to_string())
        .spawn(move || copy(stream_back, conn_back, |s| s.shutdown(Shutdown::Write)));
}

fn copy<R: Read, W: Write>(mut from: R, mut to: W, end: impl FnOnce(&W) -> io::Result<()>) {
    let _ = io::copy(&mut from, &mut to);
    let _ = end(&to);
}
