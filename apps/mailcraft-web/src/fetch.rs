// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The browser's `fetch` as the HTTP exchange of the JMAP client ([`mailcraft_jmap::HttpExchange`]),
//! doing what that trait asks of a program and nothing more:
//!
//! - **The request goes as built**: its method, address, header fields and body. Cross-origin, with
//!   no cookies or other credentials of the browser's (`credentials: "omit"`), nothing kept in the
//!   HTTP cache (`cache: "no-store"`: these answers are someone's mail), and no `Referer`.
//! - **No redirect is followed**: the browser doesn't show a redirect's target to the page, so one is
//!   [`TransportError::RedirectRefused`] and the owner gives the address it leads to.
//! - **No more than the request may be answered with is read**: a `Content-Length` above it is refused
//!   at once, and the body is read a chunk at a time and given up as soon as it passes it.
//! - **No exchange lasts more than five minutes**, and none goes on once its job is called off (Stop,
//!   or another mailbox opened): the browser looks a few times a second, and stops it at once.
//! - Of the answer, its status, its body and the fields the client reads. A server that wants
//!   `Retry-After` and `Location` seen by a page has to expose them (`Access-Control-Expose-Headers`),
//!   as the browser hides any other field; without them a busy server is waited out for the client's
//!   own time.
//!
//! The browser says nothing of why an exchange failed (an address it couldn't reach, a refused
//! cross-origin request and a bad certificate are all a `TypeError`): every such failure is
//! [`TransportError::Unreachable`]. No text of the browser's is passed on, so nothing private can be.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_jmap::{HttpExchange, HttpRequest, HttpResponse, TransportError};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AbortController, DomException, Headers, ReadableStreamDefaultReader, ReadableStreamReadResult, ReferrerPolicy, RequestCache, RequestCredentials,
    RequestInit, RequestMode, RequestRedirect, Response, ResponseType,
};

/// The longest an exchange may take, in milliseconds, the reading of its body included.
const TIME_LIMIT_MS: u32 = 5 * 60 * 1_000;
/// How often an exchange under way looks whether it is to stop, in milliseconds.
const LOOK_MS: i32 = 250;

/// The browser's `fetch`, for the JMAP client of one job: `cancel` is the job's.
pub(crate) struct Fetch {
    cancel: Arc<AtomicBool>,
}

impl Fetch {
    pub(crate) fn new(cancel: Arc<AtomicBool>) -> Fetch {
        Fetch { cancel }
    }
}

impl HttpExchange for Fetch {
    async fn exchange(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(TransportError::Aborted);
        }
        let window = web_sys::window().ok_or(TransportError::Other)?;
        // Until the exchange is over, its body read: dropped with it.
        let watch = Watch::start(&window, self.cancel.clone())?;
        let headers = Headers::new().map_err(|_| TransportError::Other)?;
        for (name, value) in request.fields() {
            headers.set(name, value).map_err(|_| TransportError::Other)?;
        }
        let init = RequestInit::new();
        init.set_method(request.method().as_str());
        init.set_headers(&headers);
        init.set_mode(RequestMode::Cors);
        init.set_credentials(RequestCredentials::Omit);
        init.set_cache(RequestCache::NoStore);
        init.set_redirect(RequestRedirect::Manual);
        init.set_referrer_policy(ReferrerPolicy::NoReferrer);
        init.set_signal(Some(&watch.controller.signal()));
        if !request.body().is_empty() {
            init.set_body(&js_sys::Uint8Array::from(request.body()));
        }
        let answer = JsFuture::from(window.fetch_with_str_and_init(request.url(), &init)).await.map_err(|e| failure(&e))?;
        let response: Response = answer.dyn_into().map_err(|_| TransportError::Other)?;
        if response.type_() == ResponseType::Opaqueredirect {
            return Err(TransportError::RedirectRefused);
        }
        let most = request.max_response_bytes();
        let fields = response.headers();
        let field = |name: &str| fields.get(name).ok().flatten();
        if field("Content-Length").and_then(|length| length.trim().parse::<u64>().ok()).is_some_and(|length| length > most as u64) {
            return Err(TransportError::BodyTooLarge);
        }
        let mut out = HttpResponse::new(response.status(), body(&response, most).await?);
        for name in HttpResponse::FIELDS_READ {
            if let Some(value) = field(name) {
                out.set_field(name, &value);
            }
        }
        Ok(out)
    }
}

/// What stops an exchange before its end: its job called off, or the time limit. Looked at every
/// [`LOOK_MS`] while it lasts, and no longer once it is dropped.
struct Watch {
    controller: AbortController,
    interval: i32,
    _look: Closure<dyn FnMut()>,
}

impl Watch {
    fn start(window: &web_sys::Window, cancel: Arc<AtomicBool>) -> Result<Watch, TransportError> {
        let controller = AbortController::new().map_err(|_| TransportError::Other)?;
        let deadline = js_sys::Date::now() + f64::from(TIME_LIMIT_MS);
        let stops = controller.clone();
        let look = Closure::<dyn FnMut()>::new(move || {
            if stops.signal().aborted() {
                return;
            }
            if cancel.load(Ordering::Relaxed) {
                // (An `AbortError`: [`failure`] makes it `Aborted`.)
                stops.abort();
            } else if js_sys::Date::now() >= deadline {
                // (A `TimeoutError`, as `AbortSignal.timeout` gives: [`failure`] makes it `TimedOut`.)
                match DomException::new_with_message_and_name("the exchange took too long", "TimeoutError") {
                    Ok(late) => stops.abort_with_reason(&late),
                    Err(_) => stops.abort(),
                }
            }
        });
        let interval = window
            .set_interval_with_callback_and_timeout_and_arguments_0(look.as_ref().unchecked_ref(), LOOK_MS)
            .map_err(|_| TransportError::Other)?;
        Ok(Watch { controller, interval, _look: look })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(window) = web_sys::window() {
            window.clear_interval_with_handle(self.interval);
        }
        // Over: what is left of the answer, if anything (one refused for its `Content-Length`), is
        // not wanted, and the browser stops downloading it.
        self.controller.abort();
    }
}

/// The body of `response`, read a chunk at a time, and given up once it passes `most` octets.
async fn body(response: &Response, most: usize) -> Result<Vec<u8>, TransportError> {
    let Some(stream) = response.body() else { return Ok(Vec::new()) };
    let reader: ReadableStreamDefaultReader = stream.get_reader().dyn_into().map_err(|_| TransportError::Other)?;
    let mut body = Vec::new();
    loop {
        let read: ReadableStreamReadResult = JsFuture::from(reader.read()).await.map_err(|e| failure(&e))?.unchecked_into();
        if read.get_done().unwrap_or(true) {
            return Ok(body);
        }
        let chunk = js_sys::Uint8Array::new(&read.get_value());
        let size = usize::try_from(chunk.length()).unwrap_or(usize::MAX);
        if body.len().saturating_add(size) > most {
            // (What is left of the body isn't wanted: the browser stops downloading it.)
            let _ = reader.cancel();
            return Err(TransportError::BodyTooLarge);
        }
        body.extend_from_slice(&chunk.to_vec());
    }
}

/// What a failed `fetch`, or a failed read of its body, was: the time limit, an exchange called off,
/// or anything else the browser won't tell apart.
fn failure(error: &JsValue) -> TransportError {
    let name = js_sys::Reflect::get(error, &JsValue::from_str("name")).ok().and_then(|name| name.as_string());
    match name.as_deref() {
        Some("TimeoutError") => TransportError::TimedOut,
        Some("AbortError") => TransportError::Aborted,
        _ => TransportError::Unreachable,
    }
}
