// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rich text, as Outlook keeps it in `PidTagRtfCompressed` (0x1009): the compression of
//! [MS-OXRTFCP] ([`decompress_rtf`]) and the text of the RTF that comes out ([`rtf_text`]),
//! written from those two specifications and the public Rich Text Format specification.
//!
//! Both read hostile bytes: output is capped by the caller, a header's size is never trusted
//! for an allocation past that cap, nothing recurses (groups are counted, not nested calls),
//! and every loop ends with its input.

use crate::crc::crc32;
use crate::{Cap, Error, Result};

/// The dictionary a stream starts with ([MS-OXRTFCP] 2.1.2.1), 207 bytes.
const DICT_INIT: &[u8] = b"{\\rtf1\\ansi\\mac\\deff0\\deftab720{\\fonttbl;}{\\f0\\fnil \\froman \\fswiss \\fmodern \\fscript \\fdecor MS Sans SerifSymbolArialTimes New RomanCourier{\\colortbl\\red0\\green0\\blue0\r\n\\par \\pard\\plain\\f0\\fs20\\b\\i\\u\\tab\\tx";
const _: () = assert!(DICT_INIT.len() == 207);
const DICT_SIZE: usize = 4096;
const HEADER: usize = 16;
const LZFU: u32 = 0x7546_5A4C;
const MELA: u32 = 0x414C_454D;

fn le32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(<[u8; 4]>::try_from(s).ok()?))
}

/// The RTF inside `PidTagRtfCompressed` bytes: the 16-byte header, then either the text as it
/// is (`MELA`) or the dictionary-compressed runs (`LZFu`) whose CRC is checked. At most
/// `max_out` bytes come out; past that is [`Cap::ItemSize`], never a bigger allocation.
pub fn decompress_rtf(data: &[u8], max_out: usize) -> Result<Vec<u8>> {
    let (Some(comp_size), Some(raw_size), Some(kind), Some(crc)) = (le32(data, 0), le32(data, 4), le32(data, 8), le32(data, 12)) else {
        return Err(Error::Corrupt("rich text shorter than its header"));
    };
    let raw_size = usize::try_from(raw_size).unwrap_or(usize::MAX);
    let body = data.get(HEADER..).unwrap_or_default();
    match kind {
        MELA => {
            // The reader may take the sizes the header gives; never more than is there.
            let take = raw_size.min(body.len());
            if take > max_out {
                return Err(Error::Cap(Cap::ItemSize));
            }
            Ok(body.get(..take).unwrap_or_default().to_vec())
        }
        LZFU => {
            if raw_size > max_out {
                return Err(Error::Cap(Cap::ItemSize));
            }
            // COMPSIZE counts the 12 bytes of the header after itself.
            let content_len = usize::try_from(comp_size)
                .unwrap_or(usize::MAX)
                .checked_sub(12)
                .ok_or(Error::Corrupt("rich text with a size smaller than its header"))?;
            let content = body.get(..content_len).ok_or(Error::Corrupt("rich text cut short"))?;
            let out = lzfu(content, max_out, raw_size)?;
            if crc32(content) != crc {
                return Err(Error::BadChecksum("compressed rich text"));
            }
            Ok(out)
        }
        _ => Err(Error::Corrupt("rich text of a kind not known")),
    }
}

fn lzfu(content: &[u8], max_out: usize, raw_size: usize) -> Result<Vec<u8>> {
    let mut dict = vec![0u8; DICT_SIZE];
    if let Some(head) = dict.get_mut(..DICT_INIT.len()) {
        head.copy_from_slice(DICT_INIT);
    }
    let mut write = DICT_INIT.len();
    let mut out: Vec<u8> = Vec::with_capacity(raw_size.min(max_out).min(1 << 20));
    let mut at = 0usize;
    let cut = Error::Corrupt("compressed rich text ends before its end mark");
    loop {
        let control = *content.get(at).ok_or_else(|| cut.clone())?;
        at = at.saturating_add(1);
        for bit in 0..8u32 {
            if control >> bit & 1 == 0 {
                let b = *content.get(at).ok_or_else(|| cut.clone())?;
                at = at.saturating_add(1);
                if out.len() >= max_out {
                    return Err(Error::Cap(Cap::ItemSize));
                }
                out.push(b);
                if let Some(d) = dict.get_mut(write) {
                    *d = b;
                }
                write = write.saturating_add(1) % DICT_SIZE;
            } else {
                let (Some(&hi), Some(&lo)) = (content.get(at), content.get(at.saturating_add(1))) else { return Err(cut) };
                at = at.saturating_add(2);
                let word = usize::from(hi) << 8 | usize::from(lo);
                let mut read = word >> 4;
                if read == write {
                    return Ok(out);
                }
                for _ in 0..(word & 0xF).saturating_add(2) {
                    let b = dict.get(read).copied().unwrap_or(0);
                    if out.len() >= max_out {
                        return Err(Error::Cap(Cap::ItemSize));
                    }
                    out.push(b);
                    read = read.saturating_add(1) % DICT_SIZE;
                    if let Some(d) = dict.get_mut(write) {
                        *d = b;
                    }
                    write = write.saturating_add(1) % DICT_SIZE;
                }
            }
        }
    }
}

/// Deepest nesting of groups read.
pub const MAX_RTF_DEPTH: usize = 256;

/// Destinations whose content is not the document's text.
const SKIPPED: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "pict",
    "object",
    "header",
    "headerl",
    "headerr",
    "headerf",
    "footer",
    "footerl",
    "footerr",
    "footerf",
    "themedata",
    "colorschememapping",
    "latentstyles",
    "datastore",
    "listtable",
    "listoverridetable",
    "revtbl",
    "rsidtbl",
    "generator",
    "filetbl",
    "fldinst",
    "xmlnstbl",
    "mmathpr",
    "pgdsctbl",
    "template",
    "private",
    "bkmkstart",
    "bkmkend",
    "wgrffmtfilter",
    "shpinst",
    "nonshppict",
    "pnseclvl",
    "listtext",
    "blipuid",
    "falt",
    "panose",
    "fchars",
    "lchars",
    "xe",
    "tc",
    "tcn",
    "ud",
    "upr",
    "title",
    "subject",
    "author",
    "operator",
    "company",
    "keywords",
    "doccomm",
    "comment",
    "category",
    "manager",
    "hlinkbase",
    "buptim",
    "creatim",
    "revtim",
    "printim",
    "txe",
    "rxe",
    "pnseclvl",
    "protusertbl",
    "defchp",
    "defpap",
    "fontemb",
    "fontfile",
    "background",
    "docvar",
    "userprops",
    "mhtmltag",
    "htmltag",
    "factoidname",
    "oleclsid",
    "objdata",
];

#[derive(Clone, Copy)]
struct Group {
    /// Skip characters after `\u` (`\ucN`).
    uc: u32,
    skip: bool,
}

struct Out {
    text: String,
    max: usize,
    pending: Vec<u8>,
    code_page: u32,
    high: Option<u16>,
}

impl Out {
    fn full(&self) -> bool {
        self.text.len() >= self.max
    }
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut self.pending);
        let decoded = mailcraft_charset::decode(&bytes, label(self.code_page));
        self.push_str(&decoded);
    }
    fn push_str(&mut self, s: &str) {
        for c in s.chars() {
            if self.full() {
                return;
            }
            self.text.push(c);
        }
    }
    fn char(&mut self, c: char) {
        self.flush_high();
        self.flush();
        self.push_str(c.encode_utf8(&mut [0; 4]));
    }
    fn flush_high(&mut self) {
        if self.high.take().is_some() {
            self.push_str("\u{FFFD}");
        }
    }
    fn byte(&mut self, b: u8) {
        self.flush_high();
        if self.pending.len() < 4096 {
            self.pending.push(b);
        }
    }
    fn unit(&mut self, u: u16) {
        self.flush();
        match (self.high.take(), u) {
            (Some(h), 0xDC00..=0xDFFF) => {
                let c = 0x10000u32.saturating_add((u32::from(h).saturating_sub(0xD800)) << 10).saturating_add(u32::from(u).saturating_sub(0xDC00));
                self.push_str(&char::from_u32(c).unwrap_or('\u{FFFD}').to_string());
            }
            (Some(_), _) => {
                self.push_str("\u{FFFD}");
                self.unit(u);
            }
            (None, 0xD800..=0xDBFF) => self.high = Some(u),
            (None, 0xDC00..=0xDFFF) => self.push_str("\u{FFFD}"),
            (None, u) => self.push_str(&char::from_u32(u32::from(u)).unwrap_or('\u{FFFD}').to_string()),
        }
    }
}

/// The encoding label for a Windows code page (`\ansicpg`); 1252 for any not known.
fn label(cp: u32) -> &'static str {
    match cp {
        874 => "windows-874",
        932 => "shift_jis",
        936 => "gbk",
        949 => "euc-kr",
        950 => "big5",
        1250 => "windows-1250",
        1251 => "windows-1251",
        1253 => "windows-1253",
        1254 => "windows-1254",
        1255 => "windows-1255",
        1256 => "windows-1256",
        1257 => "windows-1257",
        1258 => "windows-1258",
        10000 => "macintosh",
        65001 => "utf-8",
        _ => "windows-1252",
    }
}

/// The text of an RTF document: paragraphs and lines as newlines, tabs, `\'hh` bytes in the
/// document's `\ansicpg` code page (Windows-1252 when it names none), `\uN` with its `\ucN`
/// fallback skipped, and none of the font table, colours, styles, pictures, objects,
/// headers or footers, nor any `{\*…}` destination (for encapsulated HTML, [MS-OXRTFEX], that
/// leaves the text the document shows and not the `\htmltag` HTML, and the text as it is for
/// `\fromtext`). At most `max` bytes of text; more is cut. Groups nest at most
/// [`MAX_RTF_DEPTH`] deep, else an error; unbalanced braces are read as they come.
pub fn rtf_text(rtf: &[u8], max: usize) -> Result<String> {
    let start = rtf.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(rtf.len());
    if rtf.get(start..start.saturating_add(5)) != Some(b"{\\rtf") {
        return Err(Error::Corrupt("not rich text"));
    }
    let mut out = Out { text: String::new(), max, pending: Vec::new(), code_page: 1252, high: None };
    let mut stack: Vec<Group> = Vec::new();
    let mut cur = Group { uc: 1, skip: false };
    let mut first = false;
    let mut skip_chars = 0u32;
    let mut i = start;
    while let Some(&b) = rtf.get(i) {
        if out.full() {
            break;
        }
        i = i.saturating_add(1);
        match b {
            b'{' => {
                if stack.len() >= MAX_RTF_DEPTH {
                    return Err(Error::Corrupt("rich text nested too deeply"));
                }
                stack.push(cur);
                first = true;
                skip_chars = 0;
            }
            b'}' => {
                out.flush();
                out.flush_high();
                if let Some(g) = stack.pop() {
                    cur = g;
                }
                first = false;
                skip_chars = 0;
            }
            b'\r' | b'\n' => {}
            b'\\' => {
                let Some(&n) = rtf.get(i) else { break };
                i = i.saturating_add(1);
                match n {
                    b'\\' | b'{' | b'}' => {
                        first = false;
                        if !cur.skip && !take(&mut skip_chars) {
                            out.char(char::from(n));
                        }
                    }
                    b'\'' => {
                        first = false;
                        let hex =
                            rtf.get(i..i.saturating_add(2)).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                        if let Some(v) = hex {
                            i = i.saturating_add(2);
                            if !cur.skip && !take(&mut skip_chars) {
                                out.byte(v);
                            }
                        }
                    }
                    b'*' => {
                        cur.skip = true;
                        first = false;
                    }
                    b'~' => symbol(&mut out, &cur, '\u{A0}', &mut first),
                    b'_' => symbol(&mut out, &cur, '\u{2011}', &mut first),
                    b'-' | b'|' | b':' => first = false,
                    n if n.is_ascii_alphabetic() => {
                        let from = i.saturating_sub(1);
                        while rtf.get(i).is_some_and(u8::is_ascii_alphabetic) {
                            i = i.saturating_add(1);
                        }
                        let word = rtf.get(from..i).and_then(|w| std::str::from_utf8(w).ok()).unwrap_or_default();
                        let word = if word.len() > 32 { word.get(..32).unwrap_or(word) } else { word };
                        // An optional parameter, then at most one space of delimiter.
                        let mut neg = false;
                        if rtf.get(i) == Some(&b'-') {
                            neg = true;
                            i = i.saturating_add(1);
                        }
                        let mut num: Option<i64> = None;
                        while let Some(d) = rtf.get(i).filter(|d| d.is_ascii_digit()) {
                            num = Some(num.unwrap_or(0).saturating_mul(10).saturating_add(i64::from(d.saturating_sub(b'0'))).min(1 << 40));
                            i = i.saturating_add(1);
                        }
                        let num = num.map(|v| if neg { v.saturating_neg() } else { v });
                        if rtf.get(i) == Some(&b' ') {
                            i = i.saturating_add(1);
                        }
                        let was_first = std::mem::replace(&mut first, false);
                        if was_first && SKIPPED.contains(&word.to_ascii_lowercase().as_str()) {
                            cur.skip = true;
                        }
                        skip_chars = 0;
                        control(word, num, &mut cur, &mut out, &mut skip_chars, &mut i, rtf);
                    }
                    _ => first = false,
                }
            }
            0x00..=0x1F => {}
            _ => {
                first = false;
                if cur.skip || take(&mut skip_chars) {
                    continue;
                }
                if b.is_ascii() {
                    out.char(char::from(b));
                } else {
                    out.byte(b);
                }
            }
        }
    }
    out.flush();
    out.flush_high();
    Ok(tidy(&out.text))
}

fn take(skip: &mut u32) -> bool {
    if *skip > 0 {
        *skip = skip.saturating_sub(1);
        true
    } else {
        false
    }
}

fn symbol(out: &mut Out, cur: &Group, c: char, first: &mut bool) {
    *first = false;
    if !cur.skip {
        out.char(c);
    }
}

fn control(word: &str, num: Option<i64>, cur: &mut Group, out: &mut Out, skip_chars: &mut u32, i: &mut usize, rtf: &[u8]) {
    match word {
        "ansicpg" => {
            if let Some(n) = num.and_then(|n| u32::try_from(n).ok()) {
                out.flush();
                out.code_page = n;
            }
        }
        "mac" if out.code_page == 1252 => out.code_page = 10000,
        "uc" => cur.uc = num.and_then(|n| u32::try_from(n).ok()).unwrap_or(1).min(16),
        "u" if !cur.skip => {
            if let Some(n) = num {
                // A negative number is the unit minus 65536; the fallback after it is skipped.
                let unit = if n < 0 { n.saturating_add(65536) } else { n };
                out.unit(u16::try_from(unit & 0xFFFF).unwrap_or(0xFFFD));
                *skip_chars = cur.uc;
            }
        }
        "bin" => {
            let n = num.and_then(|n| usize::try_from(n).ok()).unwrap_or(0);
            *i = i.saturating_add(n).min(rtf.len());
        }
        "par" | "line" | "sect" | "page" | "row" if !cur.skip => out.char('\n'),
        "tab" | "cell" if !cur.skip => out.char('\t'),
        "emdash" if !cur.skip => out.char('\u{2014}'),
        "endash" if !cur.skip => out.char('\u{2013}'),
        "lquote" if !cur.skip => out.char('\u{2018}'),
        "rquote" if !cur.skip => out.char('\u{2019}'),
        "ldblquote" if !cur.skip => out.char('\u{201C}'),
        "rdblquote" if !cur.skip => out.char('\u{201D}'),
        "bullet" if !cur.skip => out.char('\u{2022}'),
        "emspace" | "enspace" | "qmspace" if !cur.skip => out.char(' '),
        _ => {}
    }
}

/// At most one blank line in a row, and no blanks at the ends.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut newlines = 0u32;
    for c in s.chars() {
        if c == '\n' {
            newlines = newlines.saturating_add(1);
            if newlines > 2 {
                continue;
            }
        } else {
            newlines = 0;
        }
        out.push(c);
    }
    out.trim().to_string()
}
