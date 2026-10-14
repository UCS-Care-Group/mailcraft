//! The body Outlook keeps as RTF: compressed RTF ([MS-OXRTFCP]) unpacked, and the HTML or text it
//! carries taken back out ([MS-OXRTFEX]). Nothing in the RTF is acted on: it is only read for its
//! text, and for the HTML it says it encapsulates.

use crate::codepage;

/// Largest RTF unpacked, and largest text taken out of it.
const MAX_RTF: usize = 64 << 20;

/// Deepest nesting of groups that is tracked.
const MAX_DEPTH: usize = 512;

/// The dictionary compressed RTF starts with ([MS-OXRTFCP] 2.1.3.1.1).
const PREBUFFER: &[u8] = b"{\\rtf1\\ansi\\mac\\deff0\\deftab720{\\fonttbl;}{\\f0\\fnil \\froman \\fswiss \\fmodern \\fscript \\fdecor MS Sans SerifSymbolArialTimes New RomanCourier{\\colortbl\\red0\\green0\\blue0\r\n\\par \\pard\\plain\\f0\\fs20\\b\\i\\u\\tab\\tx";

const COMPRESSED: u32 = 0x7546_5A4C; // "LZFu"
const UNCOMPRESSED: u32 = 0x414C_454D; // "MELA"

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?]))
}

/// The RTF a `PidTagRtfCompressed` value holds; `None` when it isn't compressed RTF.
pub fn decompress(data: &[u8]) -> Option<Vec<u8>> {
    let comp_size = usize::try_from(u32_at(data, 0)?).ok()?;
    let raw_size = usize::try_from(u32_at(data, 4)?).ok()?.min(MAX_RTF);
    let kind = u32_at(data, 8)?;
    let end = comp_size.saturating_add(4).min(data.len());
    let body = data.get(16..end.max(16))?;
    match kind {
        UNCOMPRESSED => Some(body.get(..raw_size.min(body.len()))?.to_vec()),
        COMPRESSED => Some(lzfu(body, raw_size)),
        _ => None,
    }
}

fn lzfu(body: &[u8], raw_size: usize) -> Vec<u8> {
    let mut dict = [0u8; 4096];
    for (slot, b) in dict.iter_mut().zip(PREBUFFER) {
        *slot = *b;
    }
    let mut w = PREBUFFER.len();
    let mut out = Vec::with_capacity(raw_size.min(body.len().saturating_mul(8)));
    let mut i = 0;
    'run: while let Some(&control) = body.get(i) {
        i += 1;
        for bit in 0..8 {
            if out.len() >= MAX_RTF {
                break 'run;
            }
            if control & (1 << bit) == 0 {
                let Some(&b) = body.get(i) else { break 'run };
                i += 1;
                out.push(b);
                if let Some(slot) = dict.get_mut(w) {
                    *slot = b;
                }
                w = (w + 1) % 4096;
            } else {
                let (Some(&hi), Some(&lo)) = (body.get(i), body.get(i + 1)) else { break 'run };
                i += 2;
                let reference = u16::from_be_bytes([hi, lo]);
                let offset = usize::from(reference >> 4);
                let len = usize::from(reference & 0xF) + 2;
                if offset == w {
                    break 'run;
                }
                for k in 0..len {
                    let b = dict.get((offset + k) % 4096).copied().unwrap_or(0);
                    out.push(b);
                    if let Some(slot) = dict.get_mut(w) {
                        *slot = b;
                    }
                    w = (w + 1) % 4096;
                }
            }
        }
    }
    out
}

/// What an RTF body holds, when it can be had: the HTML it encapsulates, else its text.
#[derive(Debug, PartialEq, Eq)]
pub enum Content {
    Html(String),
    Text(String),
}

#[derive(Clone, Copy, Default)]
struct Group {
    /// An ignorable destination (font table, picture, `{\*\...}`): nothing in it is text.
    skip: bool,
    /// Inside `{\*\htmltag ...}`: HTML of the original.
    html_tag: bool,
    /// Between `\htmlrtf` and `\htmlrtf0`: RTF that isn't part of the HTML.
    html_rtf: bool,
    /// Characters skipped after a `\u` (`\uc`).
    uc: usize,
}

/// Text and bytes in the RTF's code page, put together.
struct Out {
    text: String,
    pending: Vec<u8>,
    codepage: u32,
}

impl Out {
    fn byte(&mut self, b: u8) {
        if self.text.len() + self.pending.len() < MAX_RTF {
            self.pending.push(b);
        }
    }
    fn flush(&mut self) {
        if !self.pending.is_empty() {
            let bytes = std::mem::take(&mut self.pending);
            self.text.push_str(&codepage::decode(&bytes, self.codepage));
        }
    }
    fn char(&mut self, c: char) {
        self.flush();
        if self.text.len() < MAX_RTF {
            self.text.push(c);
        }
    }
    fn str(&mut self, s: &str) {
        for c in s.chars() {
            self.char(c);
        }
    }
}

const SKIPPED: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "pict",
    "object",
    "header",
    "footer",
    "headerl",
    "headerr",
    "headerf",
    "footerl",
    "footerr",
    "footerf",
    "listtable",
    "listoverridetable",
    "revtbl",
    "rsidtbl",
    "generator",
    "xmlnstbl",
    "themedata",
    "colorschememapping",
    "latentstyles",
    "datastore",
    "filetbl",
    "fldinst",
    "pgdsctbl",
];

/// The HTML an RTF body encapsulates (`\fromhtml`), else its text.
pub fn read(rtf: &[u8]) -> Content {
    let head = rtf.get(..rtf.len().min(1024)).unwrap_or(&[]);
    let from_html = head.windows(9).any(|w| w == b"\\fromhtml");
    let mut out = Out { text: String::new(), pending: Vec::new(), codepage: 1252 };
    let mut stack: Vec<Group> = Vec::new();
    let mut overflow = 0usize;
    let mut g = Group { uc: 1, ..Group::default() };
    let mut star = false;
    let mut skip_chars = 0usize;
    let mut i = 0;
    // Whether text here is part of what is taken out.
    let emits = |g: &Group| !g.skip && (!from_html || g.html_tag || !g.html_rtf);
    while let Some(&b) = rtf.get(i) {
        i += 1;
        match b {
            b'{' => {
                if stack.len() < MAX_DEPTH {
                    stack.push(g);
                } else {
                    overflow += 1;
                }
                star = false;
                skip_chars = 0;
            }
            b'}' => {
                if overflow > 0 {
                    overflow -= 1;
                } else if let Some(parent) = stack.pop() {
                    g = parent;
                }
                star = false;
                skip_chars = 0;
            }
            b'\r' | b'\n' => {}
            b'\\' => {
                let Some(&next) = rtf.get(i) else { break };
                if next.is_ascii_alphabetic() {
                    let start = i;
                    while rtf.get(i).is_some_and(u8::is_ascii_alphabetic) && i - start < 32 {
                        i += 1;
                    }
                    let word = std::str::from_utf8(rtf.get(start..i).unwrap_or(&[])).unwrap_or("");
                    let num_start = i;
                    if rtf.get(i) == Some(&b'-') {
                        i += 1;
                    }
                    while rtf.get(i).is_some_and(u8::is_ascii_digit) && i - num_start < 11 {
                        i += 1;
                    }
                    let param: Option<i64> = std::str::from_utf8(rtf.get(num_start..i).unwrap_or(&[])).ok().and_then(|s| s.parse().ok());
                    if rtf.get(i) == Some(&b' ') {
                        i += 1;
                    }
                    if word == "bin" {
                        i = i.saturating_add(usize::try_from(param.unwrap_or(0)).unwrap_or(0));
                        continue;
                    }
                    if skip_chars > 0 {
                        skip_chars -= 1;
                        continue;
                    }
                    if star {
                        star = false;
                        if from_html && word == "htmltag" {
                            g.html_tag = true;
                            g.skip = false;
                        } else {
                            g.skip = true;
                        }
                        continue;
                    }
                    match word {
                        "htmlrtf" => g.html_rtf = param != Some(0),
                        "ansicpg" => {
                            out.flush();
                            out.codepage = u32::try_from(param.unwrap_or(1252)).unwrap_or(1252);
                        }
                        "uc" => g.uc = usize::try_from(param.unwrap_or(1)).unwrap_or(1).min(8),
                        "u" => {
                            if emits(&g) {
                                let n = param.unwrap_or(0);
                                let n = if n < 0 { n + 65536 } else { n };
                                out.char(u32::try_from(n).ok().and_then(char::from_u32).unwrap_or('\u{fffd}'));
                            }
                            skip_chars = g.uc;
                        }
                        "par" | "line" if emits(&g) => out.str(if from_html { "\r\n" } else { "\n" }),
                        "tab" if emits(&g) => out.char('\t'),
                        w if SKIPPED.contains(&w) => g.skip = true,
                        _ => {}
                    }
                } else {
                    i += 1;
                    match next {
                        b'\'' => {
                            let hex = rtf.get(i..i + 2).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                            i += 2;
                            if skip_chars > 0 {
                                skip_chars -= 1;
                            } else if let Some(byte) = hex
                                && emits(&g)
                            {
                                out.byte(byte);
                            }
                        }
                        b'*' => star = true,
                        b'\r' | b'\n' if emits(&g) => out.str(if from_html { "\r\n" } else { "\n" }),
                        b'{' | b'}' | b'\\' => {
                            if skip_chars > 0 {
                                skip_chars -= 1;
                            } else if emits(&g) {
                                out.byte(next);
                            }
                        }
                        b'~' if emits(&g) => out.char('\u{a0}'),
                        b'_' if emits(&g) => out.char('-'),
                        _ => {}
                    }
                }
            }
            _ => {
                if skip_chars > 0 {
                    skip_chars -= 1;
                } else if emits(&g) {
                    out.byte(b);
                }
            }
        }
    }
    out.flush();
    if from_html { Content::Html(out.text) } else { Content::Text(out.text) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dictionary_is_207_bytes() {
        assert_eq!(PREBUFFER.len(), 207);
    }

    #[test]
    fn the_specification_example() {
        // [MS-OXRTFCP] 3.1.1: "{\rtf1\ansi\ansicpg1252\pard hello world}\r\n" compressed.
        let data = [
            0x2d, 0x00, 0x00, 0x00, 0x2b, 0x00, 0x00, 0x00, 0x4c, 0x5a, 0x46, 0x75, 0xf1, 0xc5, 0xc7, 0xa7, 0x03, 0x00, 0x0a, 0x00, 0x72, 0x63, 0x70,
            0x67, 0x31, 0x32, 0x35, 0x42, 0x32, 0x0a, 0xf3, 0x20, 0x68, 0x65, 0x6c, 0x09, 0x00, 0x20, 0x62, 0x77, 0x05, 0xb0, 0x6c, 0x64, 0x7d, 0x0a,
            0x80, 0x0f, 0xa0,
        ];
        assert_eq!(decompress(&data).unwrap(), b"{\\rtf1\\ansi\\ansicpg1252\\pard hello world}\r\n");
    }

    #[test]
    fn uncompressed_rtf() {
        let rtf = b"{\\rtf1 hi}";
        let mut data = Vec::new();
        data.extend_from_slice(&(rtf.len() as u32 + 12).to_le_bytes());
        data.extend_from_slice(&(rtf.len() as u32).to_le_bytes());
        data.extend_from_slice(&UNCOMPRESSED.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(rtf);
        assert_eq!(decompress(&data).unwrap(), rtf);
        assert_eq!(decompress(b"short"), None);
    }

    #[test]
    fn html_comes_back_out() {
        let rtf = br"{\rtf1\ansi\ansicpg1252\fromhtml1 \deff0{\fonttbl {\f0\fswiss Arial;}}
{\*\htmltag19 <html>}{\*\htmltag34 <head>}{\*\mhtmltag84 <img src=x>}
{\*\htmltag50 <body>}\htmlrtf {\f0 \htmlrtf0 Caf\'e9 \{ok\} \u8364?\htmlrtf\par\htmlrtf0}{\*\htmltag58 </body>}{\*\htmltag27 </html>}}";
        let Content::Html(html) = read(rtf) else { panic!("not html") };
        assert_eq!(html, "<html><head><body>Café {ok} €</body></html>");
    }

    #[test]
    fn plain_rtf_is_text() {
        let rtf = br"{\rtf1\ansi{\fonttbl{\f0 Arial;}}{\colortbl;\red0\green0\blue0;}{\*\generator Riched20;}\pard Hello\par world\tab !}";
        assert_eq!(read(rtf), Content::Text("Hello\nworld\t!".into()));
    }
}
