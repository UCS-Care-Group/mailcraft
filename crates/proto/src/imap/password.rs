// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Password authentication from RFC 4616 and RFC 4959, over an already secured stream.

use super::*;

impl<S: Read + Write> Imap<S> {
    /// Prefer the server's advertised PLAIN mechanism; otherwise use LOGIN. The host must
    /// secure the stream first. Unknown capabilities are queried here; known ones are reused.
    /// A refusal never triggers a second attempt with another mechanism.
    pub fn authenticate_password(&mut self, user: &str, password: &str) -> Result<(), ProtoError> {
        if password.is_empty() {
            return Err(ProtoError::Auth("enter a password to sign in".into()));
        }
        if user.is_empty() {
            return Err(ProtoError::Auth("enter a user name to sign in".into()));
        }
        if self.capabilities.is_none() {
            self.capabilities()?;
        }
        if !self.offers("AUTH=PLAIN") {
            return self.login(user, password);
        }
        if user.contains('\0') || password.contains('\0') {
            return Err(ProtoError::Auth("PLAIN needs a user name and password without NUL characters".into()));
        }
        if user.len().saturating_add(password.len()) > 32 * 1024 {
            return Err(ProtoError::Auth("the user name or password is too long".into()));
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
                return Err(ProtoError::Protocol("the server continued a cancelled password authentication".into()));
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
            _ if cancelled => Err(ProtoError::Auth("the server requested more than one PLAIN response".into())),
            Status::Ok if !sent => Err(ProtoError::Protocol("the server accepted a password that was not sent".into())),
            Status::Ok => {
                self.authenticated = true;
                Ok(())
            }
            Status::No => Err(ProtoError::Auth(self.refusal(&verdict))),
            _ => Err(ProtoError::Rejected(self.refusal(&verdict))),
        }
    }
}
