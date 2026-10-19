//! HTTP through the browser's `fetch`, for the JMAP transport (`mailcraft_jmap::Http`).
//!
//! What the transport owes the client (`mailcraft_jmap::Http`), and how the browser gives it:
//! - no cookies go or come: `credentials: "omit"`; the credential is the `Authorization` header alone, which
//!   the browser drops itself on a redirect to another origin, and never follows from https to http;
//! - an answer larger than the request's `max` is refused as soon as that is known: by its
//!   `Content-Length` when it says one, else while it is read, a piece at a time;
//! - the seconds of a `Retry-After` header go into the response;
//! - the text of an error holds neither the address nor the headers of the request;
//! - a request that is not over after [`GIVE_UP_MS`] (its answer read whole) is given up: a server that never
//!   answers would otherwise hold up whatever waits for it, a download the owner is looking at for one.

use mailcraft_jmap::{Error, Request, Response, Result};
use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;

/// How long a request may take, its answer read whole, in milliseconds: a message of the largest size taken
/// in comes in this time over a slow line.
pub const GIVE_UP_MS: i32 = 300_000;

/// The browser's HTTP client.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fetch;

/// What the browser said, as text: never the address (the browser's messages don't carry it).
fn js(e: &wasm_bindgen::JsValue) -> String {
    let said = e.as_string().or_else(|| e.dyn_ref::<js_sys::Error>().map(|e| String::from(e.message()))).unwrap_or_else(|| "no reason given".into());
    said.chars().filter(|c| !c.is_control()).take(200).collect()
}

fn too_large(max: usize) -> Error {
    Error::TooLarge(format!("the answer is larger than {max} bytes"))
}

impl mailcraft_jmap::Http for Fetch {
    async fn send(&self, req: Request) -> Result<Response> {
        let window = web_sys::window().ok_or_else(|| Error::Io("no window".into()))?;
        let controller = web_sys::AbortController::new().map_err(|e| Error::Io(js(&e)))?;
        let give_up = {
            let controller = controller.clone();
            Closure::once(move || controller.abort())
        };
        let timer = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(give_up.as_ref().unchecked_ref(), GIVE_UP_MS)
            .map_err(|e| Error::Io(js(&e)))?;
        let answer = exchange(&window, req, &controller.signal()).await;
        window.clear_timeout_with_handle(timer);
        // The timer is cleared before its closure goes.
        drop(give_up);
        // An answer refused before it was read whole (too large, by what it said of itself) is not read on.
        if answer.is_err() && !controller.signal().aborted() {
            controller.abort();
            return answer;
        }
        match answer {
            Err(Error::Io(_)) if controller.signal().aborted() => Err(Error::Io("the server took too long to answer".into())),
            answer => answer,
        }
    }
}

/// One request and its answer read whole, given up when `signal` says so.
async fn exchange(window: &web_sys::Window, req: Request, signal: &web_sys::AbortSignal) -> Result<Response> {
    let init = web_sys::RequestInit::new();
    init.set_signal(Some(signal));
    init.set_method(&req.method);
    init.set_credentials(web_sys::RequestCredentials::Omit);
    init.set_mode(web_sys::RequestMode::Cors);
    // Not kept by the browser's cache: every answer is the server's of now.
    init.set_cache(web_sys::RequestCache::NoStore);
    let headers = web_sys::Headers::new().map_err(|e| Error::Io(js(&e)))?;
    for (k, v) in &req.headers {
        headers.set(k, v).map_err(|e| Error::Protocol(js(&e)))?;
    }
    init.set_headers(&headers);
    if !req.body.is_empty() {
        init.set_body(&js_sys::Uint8Array::from(req.body.as_slice()));
    }
    let request = web_sys::Request::new_with_str_and_init(&req.url, &init).map_err(|e| Error::Protocol(js(&e)))?;
    let answer = JsFuture::from(window.fetch_with_request(&request)).await.map_err(|e| {
        // A browser says no more than this when the server doesn't let this page reach it (CORS), or can't be reached.
        Error::Io(format!("{} (the server may not let this page reach it)", js(&e)))
    })?;
    let resp: web_sys::Response = answer.dyn_into().map_err(|e| Error::Protocol(js(&e)))?;
    let status = resp.status();
    let retry_after = resp.headers().get("Retry-After").ok().flatten().and_then(|v| v.trim().parse::<u32>().ok());
    // Larger than allowed, by what it says of itself: refused before a byte is read.
    let length = resp.headers().get("Content-Length").ok().flatten().and_then(|v| v.trim().parse::<u64>().ok());
    if length.is_some_and(|n| usize::try_from(n).map_or(true, |n| n > req.max)) {
        return Err(too_large(req.max));
    }
    let body = read(&resp, req.max).await?;
    Ok(Response { status, body, retry_after })
}

/// The body of `resp`, read a piece at a time and given up past `max` bytes.
async fn read(resp: &web_sys::Response, max: usize) -> Result<Vec<u8>> {
    let Some(stream) = resp.body() else { return Ok(Vec::new()) };
    let reader: web_sys::ReadableStreamDefaultReader = stream.get_reader().dyn_into().map_err(|e| Error::Protocol(js(&e)))?;
    let mut body: Vec<u8> = Vec::new();
    loop {
        let chunk = JsFuture::from(reader.read()).await.map_err(|e| Error::Io(js(&e)))?;
        let field = |name: &str| js_sys::Reflect::get(&chunk, &wasm_bindgen::JsValue::from_str(name)).unwrap_or(wasm_bindgen::JsValue::UNDEFINED);
        if field("done").as_bool().unwrap_or(true) {
            return Ok(body);
        }
        let Ok(piece) = field("value").dyn_into::<js_sys::Uint8Array>() else { continue };
        let length = usize::try_from(piece.length()).unwrap_or(usize::MAX);
        if body.len().saturating_add(length) > max {
            // The rest isn't wanted: the browser stops downloading it.
            let _ = reader.cancel();
            return Err(too_large(max));
        }
        let start = body.len();
        body.resize(start + length, 0);
        if let Some(room) = body.get_mut(start..) {
            piece.copy_to(room);
        }
    }
}

/// Wait `ms` milliseconds without holding the window's thread.
pub async fn sleep(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms.max(0));
        }
    });
    let _ = JsFuture::from(promise).await;
}
