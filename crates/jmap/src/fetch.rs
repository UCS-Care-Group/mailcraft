//! Whole messages: the raw octets of an email, as RFC 5322 has them.
//!
//! An email's `blobId` names its octets (RFC 8621 §4.1.1), and a blob is downloaded from the session's
//! `downloadUrl` (RFC 8620 §6.2). [`fetch_message`] brings one email; [`fetch_messages`] brings the few
//! that a window asks for at a time, in the shape of answer the engine takes in ([`GraphFetchResult`]).
//!
//! - **A message the server no longer has** (`Email/get` lists its id as not found, or its blob is a 404)
//!   is [`Fetched::Missing`], and named in [`GraphFetchResult::missing`]: an outcome, not an error line. A
//!   server that names an email by where it is gives a moved one a new id, so this is an everyday event.
//! - **Sizes are capped.** A message over the cap is refused, never cut: by the `size` the server states
//!   when it states one, before a byte is asked for, and by the transport while the answer comes
//!   ([`Request::max`]) when the server states none or understates it.
//! - **A batch stops when nothing else would go better**: the network is gone, the server is busy, the
//!   sign-in is refused, or the way to the server isn't one the credentials may take. A message the server
//!   won't give is passed over (named in [`GraphFetchResult::failed`]) and the rest come; three refused one
//!   after another stop the batch, since by then it is likely the request and not the messages.
//! - **A batch holds only so much at once** ([`MAX_BATCH_OCTETS`]): it keeps every message it fetched until it is
//!   done, and a browser's memory only grows. A message that would take it over that, by the size the server
//!   states, waits for the next batch with those after it: nothing is wrong, and the next request names them
//!   again. One message is always taken, however large.
//! - **Nothing a server says is trusted**: its answers are read by name and type, never by position, an
//!   id it hands out is held to [`valid_id`] before it goes into a request, and a message it states or
//!   sends beyond the cap is refused. As in the rest of the crate, the credentials, the addresses and the
//!   mail stay out of errors, log lines and `Debug` output.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_model::sync::{GraphFetchResult, GraphFetched, JmapFetchRequest};
use serde_json::{Value, json};

use crate::{Client, Error, Http, Request, Result, check_hiding, send, valid_id};

/// Largest message taken, in octets: what a caller may ask for is held to it. A bigger message is
/// refused ([`Error::TooLarge`]), not cut.
pub const MAX_MESSAGE: usize = 50 << 20;
/// Most messages one batch fetches, whatever its request names.
const MAX_BATCH: usize = 100;
/// Most octets one batch brings, give or take the one message whose size the server didn't state: past
/// this, the messages it still has to fetch wait for the next batch.
pub const MAX_BATCH_OCTETS: usize = 64 << 20;
// A batch has room for the largest message there is, so that the usual run of messages is never held up by it.
const _: () = assert!(MAX_BATCH_OCTETS >= MAX_MESSAGE);
/// Messages the server wouldn't give, one after another, before the batch stops.
const MAX_FAILED_IN_A_ROW: usize = 3;
/// The type and the file name a download is asked for: a server may put them in its answer.
const MESSAGE_TYPE: &str = "message/rfc822";
const MESSAGE_NAME: &str = "message.eml";

/// What fetching one message came to.
#[derive(Clone, PartialEq, Eq)]
pub enum Fetched {
    /// The message's raw octets, as the server has them.
    Whole(Vec<u8>),
    /// The server no longer has the message.
    Missing,
}

/// What may be shown of it: never the message.
impl fmt::Debug for Fetched {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fetched::Whole(octets) => write!(f, "Fetched::Whole({} bytes)", octets.len()),
            Fetched::Missing => f.write_str("Fetched::Missing"),
        }
    }
}

/// What the server says of one email's octets.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Located {
    /// They are in the blob `blob`, `size` octets long when the server said how many.
    At { blob: String, size: Option<u64> },
    /// The server doesn't have the email.
    Missing,
    /// The server answered, but not with a blob that can be asked for (or did not answer for this email).
    Unusable,
}

/// Where the octets of each of `ids` are, in the order given: the blob and the size of each, from the
/// `Email/get` calls it takes (the server takes so many ids at a time). An id that is none is `Unusable`
/// and is not sent; an id given twice is asked about once.
async fn locate<H: Http>(client: &Client<H>, ids: &[String]) -> Result<Vec<Located>> {
    let mut seen: HashSet<&str> = HashSet::new();
    let asked: Vec<&str> = ids.iter().map(String::as_str).filter(|id| valid_id(id) && seen.insert(*id)).collect();
    let mut known: HashMap<&str, Located> = HashMap::new();
    for chunk in asked.chunks(client.max_objects_in_get().max(1)) {
        let args = json!({"accountId": client.account_id(), "ids": chunk, "properties": ["blobId", "size"]});
        let answers = client.mail(vec![("Email/get", args)]).await?;
        read_located(answers.get(0)?, chunk, &mut known);
    }
    Ok(ids.iter().map(|id| known.get(id.as_str()).cloned().unwrap_or(Located::Unusable)).collect())
}

/// Take in what an `Email/get` answered for `asked`. Only the ids that were asked for are read, each
/// once, the first entry for it counting; an entry that isn't the shape of the standard is passed over,
/// and an email left out of both lists stays unanswered.
fn read_located<'a>(answer: &Value, asked: &[&'a str], known: &mut HashMap<&'a str, Located>) {
    let wanted: HashSet<&'a str> = asked.iter().copied().collect();
    for entry in answer.get("list").and_then(Value::as_array).into_iter().flatten() {
        if let Some(id) = entry.get("id").and_then(Value::as_str).and_then(|id| wanted.get(id).copied()) {
            known.entry(id).or_insert_with(|| located(entry));
        }
    }
    for id in answer.get("notFound").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        if let Some(id) = wanted.get(id).copied() {
            known.entry(id).or_insert(Located::Missing);
        }
    }
}

/// One entry of an `Email/get` list.
fn located(entry: &Value) -> Located {
    match entry.get("blobId").and_then(Value::as_str).filter(|blob| valid_id(blob)) {
        Some(blob) => Located::At { blob: blob.to_string(), size: entry.get("size").and_then(Value::as_u64) },
        None => Located::Unusable,
    }
}

/// Download the blob `blob`: at most `max` octets (never more than [`MAX_MESSAGE`]), and none at all when
/// the server states that it is bigger. A 404 is a message the server no longer has.
async fn download<H: Http>(client: &Client<H>, blob: &str, size: Option<u64>, max: usize) -> Result<Fetched> {
    let max = max.min(MAX_MESSAGE);
    if size.is_some_and(|size| usize::try_from(size).map_or(true, |size| size > max)) {
        return Err(Error::TooLarge(format!("the message is larger than {max} bytes")));
    }
    let url = client.download_url(blob, MESSAGE_TYPE, MESSAGE_NAME)?;
    // The address names the account (a person's address, for some servers): it is hidden as the
    // session's is, should a transport put it in an error.
    let mut secrets = client.secrets.clone();
    secrets.add(&url);
    let request = Request::get(&url, max).header("Authorization", &client.auth);
    let resp = send(&client.http, request).await.map_err(|e| e.hidden(&secrets))?;
    if resp.status == 404 {
        return Ok(Fetched::Missing);
    }
    check_hiding(&resp, &secrets)?;
    // An RFC 5322 message is never empty: an empty answer is a server that didn't mean it.
    if resp.body.is_empty() {
        return Err(Error::Protocol("the server sent no message".into()));
    }
    Ok(Fetched::Whole(resp.body))
}

/// The raw octets of the email `email`, at most `max` of them (never more than [`MAX_MESSAGE`]).
///
/// `blob` is the email's `blobId` when the caller has it: then the blob is downloaded at once, held
/// to `max` as it comes. Otherwise the server is asked for it first (`Email/get`, the blob and its
/// size), so that a message it states to be too big is not downloaded at all. A message the server
/// no longer has is [`Fetched::Missing`]; any other failure is an [`Error`].
pub async fn fetch_message<H: Http>(client: &Client<H>, email: &str, blob: Option<&str>, max: usize) -> Result<Fetched> {
    let (blob, size) = match blob {
        Some(blob) => (blob.to_string(), None),
        None if !valid_id(email) => return Err(Error::Protocol("not an email id".into())),
        None => match locate(client, &[email.to_string()]).await?.pop() {
            Some(Located::At { blob, size }) => (blob, size),
            Some(Located::Missing) => return Ok(Fetched::Missing),
            _ => return Err(Error::Protocol("the server gave no blob for the message".into())),
        },
    };
    download(client, &blob, size, max).await
}

/// Fetch the messages `request` names, in its order, as the engine takes them in. `cancel` is looked
/// at before each message; what was fetched until then is in the answer.
///
/// - What came: [`GraphFetchResult::fetched`] (each with its raw octets).
/// - What the server no longer has: [`GraphFetchResult::missing`].
/// - What it wouldn't give although it has it (a status that refuses this message, a message over
///   [`MAX_MESSAGE`], an answer that isn't one): [`GraphFetchResult::failed`], which the background
///   passes over for the session. Three of these one after another stop the batch.
/// - What stopped the batch: a line in [`GraphFetchResult::errors`], and [`GraphFetchResult::offline`]
///   when the network is gone or [`GraphFetchResult::busy_for`] when the server asked to be left alone.
///
/// At most the first hundred ids are read, and a batch brings about [`MAX_BATCH_OCTETS`] at most: what it
/// does not get to is neither fetched, missing nor failed, and the next request names it again. The
/// connection is the caller's: it connected with the address and the secret the request goes with.
pub async fn fetch_messages<H: Http>(client: &Client<H>, request: &JmapFetchRequest, cancel: &AtomicBool) -> GraphFetchResult {
    fetch_batch(client, request, cancel, MAX_MESSAGE, MAX_BATCH_OCTETS).await
}

/// [`fetch_messages`] with a cap for a message and for a batch of its own (a test's).
async fn fetch_batch<H: Http>(client: &Client<H>, request: &JmapFetchRequest, cancel: &AtomicBool, max: usize, budget: usize) -> GraphFetchResult {
    let mut out = GraphFetchResult { account: request.account, email: request.email.clone(), ..Default::default() };
    // Each message once, the first hundred. An id no server hands out names nothing there: it is
    // passed over without asking, and says nothing of the server.
    let mut seen: HashSet<&str> = HashSet::new();
    let (mut ids, mut unaskable) = (Vec::new(), Vec::new());
    for id in request.ids.iter().filter(|id| seen.insert(id.as_str())).take(MAX_BATCH) {
        if valid_id(id) { ids.push(id.clone()) } else { unaskable.push(id.clone()) }
    }
    out.failed.extend(unaskable);
    if ids.is_empty() || cancel.load(Ordering::Relaxed) {
        return out;
    }
    let asked = ids.len();
    let located = match locate(client, &ids).await {
        Ok(located) => located,
        Err(e) => {
            stop(&mut out, &e);
            return out;
        }
    };
    let (mut in_a_row, mut taken) = (0usize, 0usize);
    for (id, located) in ids.into_iter().zip(located) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let fetched = match located {
            Located::Missing => Ok(Fetched::Missing),
            Located::Unusable => Err(Error::Protocol("the server gave no blob for the message".into())),
            Located::At { blob, size } => {
                // Enough in memory for one batch: the messages not yet fetched wait for the next. The size a
                // message states is counted before it comes, so that the batch is not taken over the budget
                // for it; one that states none is counted when it has come, and the next one is held back.
                let stated = size.map_or(0, |size| usize::try_from(size).unwrap_or(usize::MAX));
                if taken > 0 && taken.saturating_add(stated) > budget {
                    break;
                }
                download(client, &blob, size, max).await
            }
        };
        match fetched {
            Ok(Fetched::Whole(mime)) => {
                in_a_row = 0;
                taken = taken.saturating_add(mime.len());
                out.fetched.push(GraphFetched { id, mime });
            }
            Ok(Fetched::Missing) => {
                in_a_row = 0;
                out.missing.push(id);
            }
            // Nothing else would go better now.
            Err(e) if stops(&e) => {
                stop(&mut out, &e);
                break;
            }
            // This message, as far as can be told: passed over, so that it can't hold the rest up.
            Err(e) => {
                log::warn!("jmap: a message couldn't be fetched whole ({}); it is passed over", kind(&e));
                out.failed.push(id);
                in_a_row += 1;
                if in_a_row >= MAX_FAILED_IN_A_ROW {
                    stop(&mut out, &e);
                    break;
                }
            }
        }
    }
    let level = if out.errors.is_empty() && out.failed.is_empty() { log::Level::Debug } else { log::Level::Warn };
    log::log!(
        level,
        "jmap: {} of {} messages fetched whole, {} no longer there, {} passed over, {} problems",
        out.fetched.len(),
        asked,
        out.missing.len(),
        out.failed.len(),
        out.errors.len()
    );
    out
}

/// Whether nothing else would go better after `e`: the network, the server's patience, the sign-in, or
/// the way to the server.
fn stops(e: &Error) -> bool {
    matches!(e, Error::Io(_) | Error::Busy { .. } | Error::Auth(_) | Error::Tls(_))
}

/// The answer of a batch that could not begin: signing in to the server (`Client::connect`) failed with
/// `error`. What [`fetch_messages`] would have said of the same trouble: nothing fetched, and why.
pub fn failed_fetch(request: &JmapFetchRequest, error: &Error) -> GraphFetchResult {
    let mut out = GraphFetchResult { account: request.account, email: request.email.clone(), ..Default::default() };
    stop(&mut out, error);
    out
}

/// Say that the batch stopped, and why: for the owner's reading (no address, no secret: the errors are
/// hidden where they are made).
fn stop(out: &mut GraphFetchResult, e: &Error) {
    match e {
        Error::Busy { retry_after, .. } => out.busy_for = Some(retry_after.unwrap_or(0)),
        Error::Io(_) => out.offline = true,
        _ => {}
    }
    out.errors.push(format!("Couldn't download the messages from the server: {e}"));
}

/// A word for what an error was, for a log line: what a server said is not in it.
fn kind(e: &Error) -> &'static str {
    match e {
        Error::Io(_) => "network",
        Error::Tls(_) => "secure connection",
        Error::Protocol(_) => "unexpected answer",
        Error::Refused { .. } => "refused",
        Error::Bad(_) => "rejected",
        Error::Auth(_) => "sign-in",
        Error::Busy { .. } => "busy",
        Error::TooLarge(_) => "too large",
    }
}

#[cfg(test)]
mod tests;
