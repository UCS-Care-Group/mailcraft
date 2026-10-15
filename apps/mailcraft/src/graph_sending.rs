//! Which of a shared mailbox's messages Microsoft Graph sends for the window's Send/Receive, and
//! which are left to the mail server ([`graph_with`](crate::graph_with)).
//!
//! A shared mailbox under an owner whose mail is read over IMAP sends through the owner's own
//! sending in Graph's JSON form (`mailcraft_net::graph::send_as`), which has no place for a
//! signature or an encrypted body, nor for `In-Reply-To`, `References` or any other header: a
//! message with one of those would go without it, or not at all, and the mail run that follows
//! sends it whole through the mail server instead.

use mailcraft_model::Message;

/// Whether `message`, from a shared mailbox, goes through Graph as the mailbox: nothing of it
/// would be lost in the JSON form. Otherwise the mail run sends it.
pub(crate) fn goes_as_shared(message: &Message) -> bool {
    message.secured.is_none() && message.in_reply_to.is_empty() && message.references.is_empty() && message.headers.is_empty()
}

/// Why a message is held when the server refused to send it as the shared mailbox at `address`
/// (shown with the message; `Message::hold`).
pub(crate) fn send_as_hold(address: &str) -> String {
    let address: String = address.trim().chars().filter(|c| !c.is_control()).take(254).collect();
    format!(
        "The server didn't let your account send as {address}: sending from it takes the Send As right on it, which an administrator of your organisation gives"
    )
}

#[cfg(test)]
mod tests {
    use mailcraft_model::{Protection, Secured};

    use super::*;

    #[test]
    fn a_signed_threaded_or_headed_message_from_a_shared_mailbox_is_left_to_the_mail_server() {
        let plain = Message { subject: "Your ticket".into(), ..Default::default() };
        assert!(goes_as_shared(&plain));
        let secured = Secured { kind: Protection::SmimeSigned, raw: std::sync::Arc::new(b"Content-Type: text/plain\r\n\r\nhi".to_vec()) };
        let signed = Message { secured: Some(secured), ..plain.clone() };
        assert!(!goes_as_shared(&signed), "a signature has no place in Graph's JSON form");
        let reply = Message { in_reply_to: "<a@example.org>".into(), ..plain.clone() };
        assert!(!goes_as_shared(&reply), "the thread would be lost");
        let referenced = Message { references: vec!["<a@example.org>".into()], ..plain.clone() };
        assert!(!goes_as_shared(&referenced));
        let receipt = Message { headers: vec![("Disposition-Notification-To".into(), "x@example.org".into())], ..plain };
        assert!(!goes_as_shared(&receipt), "a header the form drops");
        // The line a held message carries names the mailbox and nothing of the owner.
        let why = send_as_hold(" support@harbor.example\r\n");
        assert!(why.starts_with("The server didn't let your account send as support@harbor.example:") && !why.contains('\r'), "{why}");
    }
}
