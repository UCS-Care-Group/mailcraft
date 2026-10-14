//! The bounded reader both protocols share: lines and counted blocks from a stream that may be hostile.

use std::io::{ErrorKind, Read, Write};

use crate::{MAX_LINE, MAX_LITERAL, ProtoError};

/// How much is asked of the stream in one read.
const CHUNK: usize = 16 * 1024;
/// Reads interrupted in a row before giving up.
const MAX_INTERRUPTS: u32 = 1_000;

/// A stream with the bytes read ahead of what was asked for.
pub(crate) struct Wire<S> {
    stream: S,
    /// Bytes read from the stream and not handed out yet (from `start`).
    buf: Vec<u8>,
    start: usize,
}

fn closed() -> ProtoError {
    ProtoError::Io("the server closed the connection".into())
}

/// What a read whose wait ran out fails with.
const WAITED: &str = "the server took too long to answer";

fn io_error(e: &std::io::Error) -> ProtoError {
    match e.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => ProtoError::Io(WAITED.into()),
        ErrorKind::UnexpectedEof | ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset | ErrorKind::BrokenPipe => closed(),
        _ => ProtoError::Io(crate::tidy(&e.to_string())),
    }
}

impl<S> Wire<S> {
    pub(crate) fn new(stream: S) -> Self {
        Wire { stream, buf: Vec::new(), start: 0 }
    }

    /// Whether the server sent more than has been read so far.
    pub(crate) fn has_pending(&self) -> bool {
        self.start < self.buf.len()
    }

    pub(crate) fn into_inner(self) -> S {
        self.stream
    }

    /// The stream itself, for its owner to set up (what a socket waits for, say). Reading from
    /// it or writing to it here would take the session apart.
    pub(crate) fn get_mut(&mut self) -> &mut S {
        &mut self.stream
    }
}

impl<S: Read + Write> Wire<S> {
    /// Read some more into the buffer; the end of the stream is an error. With `polled`, a read
    /// whose wait ran out answers `false`, and what was read before stays where it is.
    fn fill(&mut self, polled: bool) -> Result<bool, ProtoError> {
        if self.start > 0 {
            self.buf.drain(..self.start.min(self.buf.len()));
            self.start = 0;
        }
        let mut tmp = vec![0u8; CHUNK];
        let n = match read_some(&mut self.stream, &mut tmp) {
            Ok(n) => n,
            Err(ProtoError::Io(why)) if polled && why == WAITED => return Ok(false),
            Err(e) => return Err(e),
        };
        self.buf.extend_from_slice(tmp.get(..n).unwrap_or(&[]));
        Ok(true)
    }

    /// The next line without its line ending (CRLF, or a bare LF).
    pub(crate) fn read_line(&mut self) -> Result<Vec<u8>, ProtoError> {
        self.next_line(false)?.ok_or_else(|| ProtoError::Io(WAITED.into()))
    }

    /// The next line, like [`read_line`](Self::read_line), or `None` when the stream's wait for
    /// a read runs out before the line is whole: what came of it so far is kept for the next call.
    pub(crate) fn poll_line(&mut self) -> Result<Option<Vec<u8>>, ProtoError> {
        self.next_line(true)
    }

    fn next_line(&mut self, polled: bool) -> Result<Option<Vec<u8>>, ProtoError> {
        // How much of the pending bytes has been searched for a line feed already.
        let mut scanned = 0usize;
        loop {
            let pending = self.buf.get(self.start..).unwrap_or(&[]);
            if let Some(i) = pending.get(scanned..).and_then(|rest| rest.iter().position(|b| *b == b'\n')) {
                let end = scanned.saturating_add(i);
                let line = pending.get(..end).unwrap_or(&[]);
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                if line.len() > MAX_LINE {
                    return Err(too_long());
                }
                let line = line.to_vec();
                self.start = self.start.saturating_add(end).saturating_add(1);
                return Ok(Some(line));
            }
            scanned = pending.len();
            // The limit is on the line itself; a CR may still come on top.
            if scanned > MAX_LINE + 1 {
                return Err(too_long());
            }
            // Each round reads at least one byte, so this ends at the line feed, the cap or the end of the stream.
            if !self.fill(polled)? {
                return Ok(None);
            }
        }
    }

    /// Exactly `n` bytes (at most [`MAX_LITERAL`]).
    pub(crate) fn read_block(&mut self, n: usize) -> Result<Vec<u8>, ProtoError> {
        if n > MAX_LITERAL {
            return Err(ProtoError::Protocol(format!("the server sent {n} bytes in one piece, more than MailCraft accepts")));
        }
        // The size is the server's word: memory is taken as the bytes arrive.
        let mut out = Vec::with_capacity(n.min(CHUNK));
        let pending = self.buf.get(self.start..).unwrap_or(&[]);
        let take = pending.len().min(n);
        out.extend_from_slice(pending.get(..take).unwrap_or(&[]));
        self.start = self.start.saturating_add(take);
        let mut tmp = vec![0u8; CHUNK];
        while out.len() < n {
            let want = (n - out.len()).min(tmp.len());
            let got = read_some(&mut self.stream, tmp.get_mut(..want).unwrap_or(&mut []))?;
            out.extend_from_slice(tmp.get(..got.min(want)).unwrap_or(&[]));
        }
        Ok(out)
    }

    pub(crate) fn write(&mut self, mut bytes: &[u8]) -> Result<(), ProtoError> {
        // Not `write_all`: that retries an interrupted write for ever.
        let mut interrupts = 0;
        while !bytes.is_empty() {
            match self.stream.write(bytes) {
                Ok(0) => return Err(closed()),
                Ok(n) => bytes = bytes.get(n..).unwrap_or(&[]),
                Err(e) if e.kind() == ErrorKind::Interrupted && interrupts < MAX_INTERRUPTS => interrupts += 1,
                Err(e) => return Err(io_error(&e)),
            }
        }
        Ok(())
    }

    pub(crate) fn flush(&mut self) -> Result<(), ProtoError> {
        self.stream.flush().map_err(|e| io_error(&e))
    }
}

fn too_long() -> ProtoError {
    ProtoError::Protocol(format!("the server sent a line longer than {} KB", MAX_LINE / 1024))
}

/// One read of at least one byte; the end of the stream is an error.
fn read_some<S: Read>(stream: &mut S, buf: &mut [u8]) -> Result<usize, ProtoError> {
    let mut interrupts = 0;
    loop {
        match stream.read(buf) {
            Ok(0) => return Err(closed()),
            // A stream that claims more than the buffer holds is lying.
            Ok(n) => return Ok(n.min(buf.len())),
            Err(e) if e.kind() == ErrorKind::Interrupted && interrupts < MAX_INTERRUPTS => interrupts += 1,
            Err(e) => return Err(io_error(&e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{Broken, Script};

    #[test]
    fn lines_and_blocks_arrive_in_pieces() {
        let mut s = Script::new("first\r\nsecond\nab\r\ncd\r\nlast\r\n");
        let mut w = Wire::new(&mut s);
        assert_eq!(w.read_line().unwrap(), b"first");
        assert_eq!(w.read_line().unwrap(), b"second");
        assert_eq!(w.read_block(6).unwrap(), b"ab\r\ncd");
        assert_eq!(w.read_block(0).unwrap(), b"");
        assert_eq!(w.read_line().unwrap(), b"");
        assert_eq!(w.read_line().unwrap(), b"last");
        assert!(!w.has_pending());
    }

    #[test]
    fn the_end_of_the_stream_is_an_error() {
        let mut s = Script::new("no line ending");
        let mut w = Wire::new(&mut s);
        assert_eq!(w.read_line(), Err(ProtoError::Io("the server closed the connection".into())));
        let mut s = Script::new("short");
        assert!(matches!(Wire::new(&mut s).read_block(10), Err(ProtoError::Io(_))));
    }

    #[test]
    fn long_lines_and_large_blocks_are_refused() {
        let mut s = Script::whole(vec![b'x'; MAX_LINE + CHUNK * 2]);
        assert!(matches!(Wire::new(&mut s).read_line(), Err(ProtoError::Protocol(_))));
        // A line of exactly the limit is fine.
        let mut ok = vec![b'x'; MAX_LINE];
        ok.extend_from_slice(b"\r\n");
        let mut s = Script::whole(ok);
        assert_eq!(Wire::new(&mut s).read_line().unwrap().len(), MAX_LINE);
        // The claim alone is refused: nothing is read or allocated.
        let mut s = Script::new("");
        assert!(matches!(Wire::new(&mut s).read_block(MAX_LITERAL + 1), Err(ProtoError::Protocol(_))));
    }

    #[test]
    fn stream_errors_become_connection_problems() {
        let mut w = Wire::new(Broken(ErrorKind::TimedOut));
        assert_eq!(w.read_line(), Err(ProtoError::Io("the server took too long to answer".into())));
        assert!(matches!(w.write(b"x"), Err(ProtoError::Io(_))));
        // A stream that is interrupted for ever hangs neither the reader nor the writer.
        assert!(matches!(Wire::new(Broken(ErrorKind::Interrupted)).read_line(), Err(ProtoError::Io(_))));
        assert!(matches!(Wire::new(Broken(ErrorKind::Interrupted)).write(b"x"), Err(ProtoError::Io(_))));
    }
}
