//! Test streams: a scripted server, and a stream that only fails.

use std::io::{Error, ErrorKind, Read, Write};

/// A canned server: reads hand out what the server "says" (a few bytes at a time, so every
/// boundary gets split somewhere), writes are kept for the test to look at.
#[derive(Debug)]
pub(crate) struct Script {
    input: Vec<u8>,
    pos: usize,
    step: usize,
    trickle: bool,
    /// Where a read waits in vain once (a server with nothing to say for a while).
    pauses: Vec<usize>,
    pub(crate) written: Vec<u8>,
}

impl Script {
    /// Hands the bytes out 1–7 at a time.
    pub(crate) fn new(server: impl AsRef<[u8]>) -> Script {
        Script { input: server.as_ref().to_vec(), pos: 0, step: 0, trickle: true, pauses: Vec::new(), written: Vec::new() }
    }

    /// Says the parts one after the other, with a read that waits in vain between two of them.
    pub(crate) fn pausing(parts: &[&str]) -> Script {
        let mut script = Script::new(parts.concat());
        let mut at = 0;
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            at += part.len();
            script.pauses.push(at);
        }
        script
    }

    /// Hands out as much as each read asks for.
    pub(crate) fn whole(server: impl AsRef<[u8]>) -> Script {
        Script { trickle: false, ..Script::new(server) }
    }

    /// What the client wrote, as text.
    pub(crate) fn sent(&self) -> String {
        String::from_utf8_lossy(&self.written).into_owned()
    }

    /// Whether the client read everything the script had.
    pub(crate) fn finished(&self) -> bool {
        self.pos >= self.input.len()
    }
}

impl Read for Script {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(at) = self.pauses.iter().position(|p| *p == self.pos) {
            self.pauses.remove(at);
            return Err(Error::from(ErrorKind::WouldBlock));
        }
        let left = &self.input[self.pos.min(self.input.len())..];
        let mut n = left.len().min(buf.len());
        // A pause is not read past.
        if let Some(next) = self.pauses.iter().filter(|p| **p > self.pos).min() {
            n = n.min(next - self.pos);
        }
        if self.trickle {
            n = n.min([3, 1, 7, 2, 5, 4, 6][self.step % 7]);
            self.step += 1;
        }
        buf[..n].copy_from_slice(&left[..n]);
        self.pos += n;
        Ok(n)
    }
}

impl Write for Script {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.written.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A stream whose every read and write fails.
#[derive(Debug)]
pub(crate) struct Broken(pub(crate) ErrorKind);

impl Read for Broken {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(Error::from(self.0))
    }
}

impl Write for Broken {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(Error::from(self.0))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(Error::from(self.0))
    }
}
