// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Password authentication from RFC 4616 and RFC 4959, over an already secured stream.

use super::*;

impl<S: Read + Write> Imap<S> {
    /// The password mechanism preferred on this connection, using cached capabilities
    /// or asking once when they are unknown. The host can preserve this choice across
    /// an authentication-only reconnect without downgrading PLAIN to LOGIN.
    pub fn password_uses_plain(&mut self) -> Result<bool, ProtoError> {
        if self.capabilities.is_none() {
            self.capabilities()?;
        }
        Ok(self.offers("AUTH=PLAIN"))
    }

    /// Prefer the server's advertised PLAIN mechanism; otherwise use LOGIN. The host must
    /// secure the stream first. Unknown capabilities are queried here; known ones are reused.
    /// A refusal never triggers a second attempt with another mechanism.
    pub fn authenticate_password(&mut self, user: &str, password: &str) -> Result<(), ProtoError> {
        self.password_refused = false;
        if password.is_empty() {
            return Err(ProtoError::Auth(said!("enter a password to sign in")));
        }
        if user.is_empty() {
            return Err(ProtoError::Auth(said!("enter a user name to sign in")));
        }
        if !self.password_uses_plain()? {
            return self.login(user, password);
        }
        if user.contains('\0') || password.contains('\0') {
            return Err(ProtoError::Auth(said!("PLAIN needs a user name and password without NUL characters")));
        }
        if user.len().saturating_add(password.len()) > 32 * 1024 {
            return Err(ProtoError::Auth(said!("the user name or password is too long")));
        }
        // Empty authorization identity: the server derives it from the authentication identity.
        let response = mailcraft_charset::base64::STANDARD.encode(format!("\0{user}\0{password}").as_bytes());
        self.secrets.add(password);
        self.secrets.add(&response);
        let inline = self.offers("SASL-IR");
        let tag = self.tag();
        let command = if inline { format!("{tag} AUTHENTICATE PLAIN {response}\r\n") } else { format!("{tag} AUTHENTICATE PLAIN\r\n") };
        log::trace!("C: {tag} AUTHENTICATE PLAIN{}", if inline { " ***" } else { "" });
        self.wire.write(command.as_bytes())?;
        self.wire.flush()?;
        let mut sent = inline;
        let mut cancelled = false;
        let verdict = loop {
            if let Some(verdict) = self.read_until(&tag, true, &mut ignore)? {
                break verdict;
            }
            if cancelled {
                return Err(ProtoError::Protocol(said!("the server continued a cancelled password authentication")));
            }
            let answer = if sent {
                cancelled = true;
                "*"
            } else {
                sent = true;
                response.as_str()
            };
            log::trace!("C: ***");
            self.wire.write(format!("{answer}\r\n").as_bytes())?;
            self.wire.flush()?;
        };
        match verdict.status {
            _ if cancelled => Err(ProtoError::Auth(said!("the server requested more than one PLAIN response"))),
            Status::Ok if !sent => Err(ProtoError::Protocol(said!("the server accepted a password that was not sent"))),
            Status::Ok => self.signed_in(&verdict),
            Status::No => Err(self.password_refusal(&verdict)),
            _ => Err(ProtoError::Rejected(self.refusal(&verdict))),
        }
    }
}
