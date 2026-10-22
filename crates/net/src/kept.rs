// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Connections kept open for Microsoft Graph (#276): a request that finds an open connection to
//! its server goes on it, instead of paying for a new TCP connection and TLS handshake each.
//!
//! A connection is made as every other one is ([`http::connect`]: TLS, or plain to this machine
//! alone, with its certificate checked against the host), and is kept only for the same host,
//! port and encryption. It is kept after an answer only when that answer said where it ends (by
//! its length or in chunks), nothing came after it, and the server didn't say it closes; a
//! connection that failed, or an answer the reader can't be sure of, is dropped. At most
//! [`PER_HOST`] connections wait for each server, [`MOST`] in all, none for longer than
//! [`IDLE_FOR`], and none carries more than [`MOST_USES`] requests.
//!
//! A server may close a waiting connection at any time, and that is only found out by asking on
//! it. So only a request that may be asked again goes on a connection that waited: a read, or a
//! change that is the same however often it is made. When such a connection turns out to have
//! closed before a byte of the answer came, the request is asked once more on a new one. A
//! request that would happen twice (making something, a move, sending) is never asked on a
//! connection that waited and never asked again here: it always has a new connection.

use std::io::{Read, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::NetError;
use crate::http::{self, Reply, Request, Url};
use crate::transport::Stream;

/// Most connections that wait for one server: as many as Graph takes at once for a mailbox.
pub(crate) const PER_HOST: usize = 4;
/// Most connections that wait, over all servers.
pub(crate) const MOST: usize = 16;
/// Longest a connection waits for its next request: well within the time servers keep one.
pub(crate) const IDLE_FOR: Duration = Duration::from_secs(30);
/// Most requests one connection carries before it is let go.
pub(crate) const MOST_USES: u32 = 500;

/// The connections of the program.
static POOL: Pool = Pool::new(PER_HOST, MOST, IDLE_FOR);

/// One request on a connection kept open, or on a new one that may then be kept. With
/// `repeatable` (a read, or a change that is the same however often it is made) a connection
/// that waited may be used, and the request is asked once more on a new connection when that
/// one had closed; without, the request always has a new connection and is asked once.
pub(crate) fn request(url: &Url, request: &Request<'_>, repeatable: bool) -> Result<Reply, NetError> {
    POOL.request(url, request, repeatable)
}

/// Which connections may carry a request: the same host (in any case), port and encryption.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    host: String,
    port: u16,
    secure: bool,
}

impl Key {
    fn of(url: &Url) -> Key {
        let port = match (url.port, url.secure) {
            (0, true) => 443,
            (0, false) => 80,
            (port, _) => port,
        };
        Key { host: url.host.to_ascii_lowercase(), port, secure: url.secure }
    }
}

/// A connection that waits for its next request.
struct Idle {
    key: Key,
    stream: Stream<'static>,
    since: Instant,
    /// Requests it has carried.
    uses: u32,
}

/// Connections that wait, with their limits.
pub(crate) struct Pool {
    idle: Mutex<Vec<Idle>>,
    per_host: usize,
    most: usize,
    idle_for: Duration,
}

impl Pool {
    pub(crate) const fn new(per_host: usize, most: usize, idle_for: Duration) -> Pool {
        Pool { idle: Mutex::new(Vec::new()), per_host, most, idle_for }
    }

    fn idle(&self) -> std::sync::MutexGuard<'_, Vec<Idle>> {
        // A thread that panicked while holding the list left it whole: it is only taken from and put to.
        self.idle.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// How many connections wait, for the tests.
    #[cfg(test)]
    pub(crate) fn waiting(&self) -> usize {
        self.idle().len()
    }

    /// See [`request`].
    pub(crate) fn request(&self, url: &Url, request: &Request<'_>, repeatable: bool) -> Result<Reply, NetError> {
        http::check_method(request.method)?;
        let key = Key::of(url);
        if repeatable
            && let Some(mut idle) = self.take(&key)
            && idle.stream.wait_for(request.patience).is_ok()
        {
            let mut counted = Counted { inner: &mut idle.stream, read: 0 };
            match http::exchange(&mut counted, url, request, true) {
                Ok((reply, reusable)) => {
                    if reusable {
                        self.put(key, idle.stream, idle.uses.saturating_add(1));
                    }
                    return Ok(reply);
                }
                // It had closed while it waited: the server never had the request.
                Err(e) if counted.read == 0 && http::is_closed(&e) => {
                    log::debug!("a connection kept open had closed; asking on a new one");
                }
                Err(e) => return Err(e),
            }
        }
        let mut stream = http::connect(url, request.patience)?;
        let (reply, reusable) = http::exchange(&mut stream, url, request, true)?;
        if reusable {
            self.put(key, stream, 1);
        }
        Ok(reply)
    }

    /// The connection for `key` that waited least, when one is still in its time.
    fn take(&self, key: &Key) -> Option<Idle> {
        let mut idle = self.idle();
        idle.retain(|waiting| waiting.since.elapsed() < self.idle_for);
        let at = idle.iter().rposition(|waiting| waiting.key == *key)?;
        Some(idle.remove(at))
    }

    /// Keep `stream` for the next request to `key`, within the limits: the connection that
    /// waited longest makes room.
    fn put(&self, key: Key, stream: Stream<'static>, uses: u32) {
        if uses >= MOST_USES || self.per_host == 0 || self.most == 0 {
            return;
        }
        let mut idle = self.idle();
        idle.retain(|waiting| waiting.since.elapsed() < self.idle_for);
        if idle.iter().filter(|waiting| waiting.key == key).count() >= self.per_host
            && let Some(oldest) = idle.iter().position(|waiting| waiting.key == key)
        {
            idle.remove(oldest);
        }
        if idle.len() >= self.most {
            idle.remove(0);
        }
        idle.push(Idle { key, stream, since: Instant::now(), uses });
    }
}

/// A stream that counts the bytes read from it: whether anything of the answer came.
struct Counted<'s, S> {
    inner: &'s mut S,
    read: u64,
}

impl<S: Read> Read for Counted<'_, S> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read = self.read.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        Ok(n)
    }
}

impl<S: Write> Write for Counted<'_, S> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests;
