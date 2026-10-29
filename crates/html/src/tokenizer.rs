// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A tolerant HTML tokenizer: text, start tags and end tags.
//!
//! It follows the general syntax of the HTML Living Standard closely enough for mail, and reads
//! anything: a `<` that starts no tag is text, a tag or comment that never ends runs to the end of
//! the input. Comments, doctypes, processing instructions and the content of `script`, `style`,
//! `title` and friends never leave this module.

use crate::entities::unescape;

/// Most attributes kept per tag; the rest are read and dropped.
const MAX_ATTRS: usize = 32;

/// Most `<style>` texts held between two tokens.
const MAX_STYLES: usize = 256;

/// Longest tag name kept; a longer one is an unknown tag.
const MAX_TAG_NAME: usize = 24;

/// Elements whose content is raw text and never shown.
const DROPPED: [&str; 6] = ["script", "style", "title", "iframe", "noembed", "noframes"];

/// Elements whose content is text, not markup, and is shown.
const RAW_TEXT: [&str; 2] = ["textarea", "xmp"];

/// A start tag.
#[derive(Debug, PartialEq)]
pub(crate) struct Tag<'a> {
    /// Lower case; empty for a name too long to be a real element.
    pub name: String,
    /// Names and values as written.
    attrs: Vec<(&'a str, &'a str)>,
    /// What the message's style sheets say about the element, set by the one who reads them
    /// (`sheet.rs`).
    pub sheet: Option<std::rc::Rc<crate::sheet::Applied>>,
}

impl Tag<'_> {
    /// An attribute's value with character references decoded; the first one wins, as in browsers.
    pub fn attr(&self, name: &str) -> Option<String> {
        self.attrs.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| unescape(v))
    }

    /// Every attribute in the order written: the name as written, the value with character
    /// references decoded.
    pub fn attributes(&self) -> impl Iterator<Item = (&str, String)> + '_ {
        self.attrs.iter().map(|(n, v)| (*n, unescape(v)))
    }

    /// An attribute's value as written, character references and all.
    pub fn raw_attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| *v)
    }

    /// Whether the tag has the attribute, with or without a value.
    pub fn has(&self, name: &str) -> bool {
        self.attrs.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum Token<'a> {
    /// Character data as written: references are not decoded yet.
    Text(&'a str),
    Open(Tag<'a>),
    /// An end tag's name, lower case.
    Close(String),
}

pub(crate) struct Tokenizer<'a> {
    src: &'a str,
    pos: usize,
    /// Set after the start tag of a raw-text element: its content comes next.
    raw: Option<&'static str>,
    /// The text of the `<style>` elements skipped over since the last token was given, each with its
    /// `media` attribute (at most [`MAX_STYLES`]): the one who reads style sheets takes them.
    pub styles: Vec<(String, &'a str)>,
}

impl<'a> Tokenizer<'a> {
    pub fn new(src: &'a str) -> Tokenizer<'a> {
        Tokenizer { src, pos: 0, raw: None, styles: Vec::new() }
    }

    /// How far into the input the tokens given so far go, in bytes.
    pub fn pos(&self) -> usize {
        self.pos
    }

    fn bytes(&self) -> &'a [u8] {
        self.src.as_bytes()
    }

    fn slice(&self, start: usize, end: usize) -> &'a str {
        self.src.get(start..end).unwrap_or("")
    }

    /// Whether the `<` at `i` starts a tag, a comment or a declaration (rather than being text).
    fn markup_at(&self, i: usize) -> bool {
        let b = self.bytes();
        match b.get(i + 1) {
            Some(c) if c.is_ascii_alphabetic() => true,
            Some(b'!' | b'?') => true,
            Some(b'/') => matches!(b.get(i + 2), Some(c) if c.is_ascii_alphabetic() || *c == b'>'),
            _ => false,
        }
    }

    /// The end of the text run that starts at `self.pos`: the next `<` that starts markup.
    fn text_end(&self) -> usize {
        let b = self.bytes();
        let mut i = self.pos + 1;
        loop {
            match find_byte(b, i, b'<') {
                Some(j) if self.markup_at(j) => return j,
                Some(j) => i = j + 1,
                None => return b.len(),
            }
        }
    }

    /// Skip a comment, doctype or processing instruction at `self.pos`.
    fn skip_declaration(&mut self) {
        let b = self.bytes();
        let end = if b.get(self.pos..self.pos + 4) == Some(b"<!--") {
            let body = self.pos + 4;
            if b.get(body) == Some(&b'>') {
                body + 1
            } else if b.get(body..body + 2) == Some(b"->") {
                body + 2
            } else {
                find_sub(b, body, b"-->").map_or(b.len(), |i| i + 3)
            }
        } else {
            find_byte(b, self.pos, b'>').map_or(b.len(), |i| i + 1)
        };
        self.pos = end.min(b.len());
    }

    /// Read the end tag at `self.pos` (`</name ...>`).
    fn close_tag(&mut self) -> Option<Token<'a>> {
        let b = self.bytes();
        let start = self.pos + 2;
        let mut i = start;
        while let Some(c) = b.get(i) {
            if c.is_ascii_whitespace() || *c == b'/' || *c == b'>' {
                break;
            }
            i += 1;
        }
        let name = self.slice(start, i);
        self.pos = find_byte(b, i, b'>').map_or(b.len(), |j| j + 1);
        // `</>` is nothing at all.
        (!name.is_empty()).then(|| Token::Close(tag_name(name)))
    }

    /// Read the start tag at `self.pos` (`<name attr=value ...>`).
    fn open_tag(&mut self) -> Tag<'a> {
        let b = self.bytes();
        let start = self.pos + 1;
        let mut i = start;
        while let Some(c) = b.get(i) {
            if c.is_ascii_whitespace() || *c == b'/' || *c == b'>' {
                break;
            }
            i += 1;
        }
        let name = tag_name(self.slice(start, i));
        let mut attrs = Vec::new();
        loop {
            while matches!(b.get(i), Some(c) if c.is_ascii_whitespace() || *c == b'/') {
                i += 1;
            }
            match b.get(i) {
                None => break,
                Some(b'>') => {
                    i += 1;
                    break;
                }
                Some(_) => {}
            }
            // An attribute name runs to white space, `/`, `>` or `=`; a leading `=` is part of it.
            let name_start = i;
            i += 1;
            while let Some(c) = b.get(i) {
                if c.is_ascii_whitespace() || matches!(*c, b'/' | b'>' | b'=') {
                    break;
                }
                i += 1;
            }
            let attr = self.slice(name_start, i);
            let mut j = i;
            while matches!(b.get(j), Some(c) if c.is_ascii_whitespace()) {
                j += 1;
            }
            let mut value = "";
            if b.get(j) == Some(&b'=') {
                j += 1;
                while matches!(b.get(j), Some(c) if c.is_ascii_whitespace()) {
                    j += 1;
                }
                match b.get(j) {
                    Some(&q) if q == b'"' || q == b'\'' => {
                        let end = find_byte(b, j + 1, q).unwrap_or(b.len());
                        value = self.slice(j + 1, end);
                        i = (end + 1).min(b.len());
                    }
                    _ => {
                        let mut end = j;
                        while let Some(c) = b.get(end) {
                            if c.is_ascii_whitespace() || *c == b'>' {
                                break;
                            }
                            end += 1;
                        }
                        value = self.slice(j, end);
                        i = end;
                    }
                }
            } else {
                i = j;
            }
            if attrs.len() < MAX_ATTRS {
                attrs.push((attr, value));
            }
        }
        self.pos = i.min(b.len());
        Tag { name, attrs, sheet: None }
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        loop {
            let b = self.bytes();
            if self.pos >= b.len() {
                return None;
            }
            if let Some(name) = self.raw.take() {
                let end = find_end_tag(b, self.pos, name).unwrap_or(b.len());
                let text = self.slice(self.pos, end);
                self.pos = end;
                if !text.is_empty() {
                    return Some(Token::Text(text));
                }
                continue;
            }
            if b.get(self.pos) != Some(&b'<') || !self.markup_at(self.pos) {
                let start = self.pos;
                self.pos = self.text_end();
                return Some(Token::Text(self.slice(start, self.pos)));
            }
            match b.get(self.pos + 1) {
                Some(b'!' | b'?') => self.skip_declaration(),
                Some(b'/') => {
                    if let Some(token) = self.close_tag() {
                        return Some(token);
                    }
                }
                _ => {
                    let tag = self.open_tag();
                    if DROPPED.contains(&tag.name.as_str()) {
                        // Skip the content and the end tag.
                        let end = find_end_tag(b, self.pos, &tag.name).unwrap_or(b.len());
                        if tag.name == "style" && self.styles.len() < MAX_STYLES && for_screen(&tag) {
                            let media = crate::truncate(tag.raw_attr("media").unwrap_or("").trim(), crate::media::MAX_QUERY_BYTES + 1);
                            self.styles.push((media.to_string(), self.slice(self.pos, end)));
                        }
                        self.pos = find_byte(b, end, b'>').map_or(b.len(), |i| i + 1);
                        continue;
                    }
                    self.raw = RAW_TEXT.iter().copied().find(|n| *n == tag.name);
                    return Some(Token::Open(tag));
                }
            }
        }
    }
}

/// Whether a `<style>` is a style sheet: text/css. (Whether it is for this pane is for its
/// `media` to say, when it is read.)
fn for_screen(tag: &Tag<'_>) -> bool {
    let kind = tag.raw_attr("type").unwrap_or("").trim();
    kind.is_empty() || kind.eq_ignore_ascii_case("text/css")
}

/// A tag name in lower case; empty when it is too long to be a real element.
fn tag_name(raw: &str) -> String {
    if raw.len() > MAX_TAG_NAME { String::new() } else { raw.to_ascii_lowercase() }
}

fn find_byte(b: &[u8], from: usize, byte: u8) -> Option<usize> {
    b.get(from..)?.iter().position(|c| *c == byte).map(|i| i + from)
}

fn find_sub(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    b.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

/// Where `</name` starts (any letter case, followed by white space, `/`, `>` or the end), at or after `from`.
fn find_end_tag(b: &[u8], from: usize, name: &str) -> Option<usize> {
    let mut i = from;
    while let Some(lt) = find_byte(b, i, b'<') {
        let after = lt + 2 + name.len();
        let named = b.get(lt + 1) == Some(&b'/') && b.get(lt + 2..after).is_some_and(|n| n.eq_ignore_ascii_case(name.as_bytes()));
        if named && b.get(after).is_none_or(|c| c.is_ascii_whitespace() || matches!(*c, b'/' | b'>')) {
            return Some(lt);
        }
        i = lt + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(html: &str) -> Vec<String> {
        Tokenizer::new(html)
            .map(|t| match t {
                Token::Text(t) => format!("T:{t}"),
                Token::Open(tag) => {
                    let attrs: Vec<String> = tag.attrs.iter().map(|(n, v)| format!("{n}={v}")).collect();
                    if attrs.is_empty() { format!("<{}>", tag.name) } else { format!("<{} {}>", tag.name, attrs.join(" ")) }
                }
                Token::Close(n) => format!("</{n}>"),
            })
            .collect()
    }

    #[test]
    fn text_and_tags() {
        assert_eq!(tokens("Hi <B>there</b>!"), ["T:Hi ", "<b>", "T:there", "</b>", "T:!"]);
        assert_eq!(tokens(""), Vec::<String>::new());
        assert_eq!(tokens("plain"), ["T:plain"]);
        assert_eq!(tokens("<br/><hr />"), ["<br>", "<hr>"]);
    }

    #[test]
    fn attributes_in_every_quoting_style() {
        assert_eq!(tokens(r#"<a href="x y" title='it"s' id=z data-k>"#), [r#"<a href=x y title=it"s id=z data-k=>"#]);
        assert_eq!(tokens("<a href = \"u\"\n\tclass = c >t"), ["<a href=u class=c>", "T:t"]);
        assert_eq!(tokens("<img src=a.png/>"), ["<img src=a.png/>"]);
        assert_eq!(tokens("<p =x=y>"), ["<p =x=y>"]);
        // A `>` inside a quoted value does not end the tag.
        assert_eq!(tokens(r#"<a title="a>b">t</a>"#), ["<a title=a>b>", "T:t", "</a>"]);
    }

    #[test]
    fn attribute_values_are_decoded_on_demand() {
        let mut t = Tokenizer::new(r#"<a HREF="?a=1&amp;b=2" href="second">"#);
        let Some(Token::Open(tag)) = t.next() else { panic!("no tag") };
        assert_eq!(tag.attr("href").as_deref(), Some("?a=1&b=2"));
        assert_eq!(tag.attr("missing"), None);
    }

    #[test]
    fn a_lone_angle_bracket_is_text() {
        assert_eq!(tokens("a < b and c > d"), ["T:a < b and c > d"]);
        assert_eq!(tokens("1<2<3"), ["T:1<2<3"]);
        assert_eq!(tokens("<"), ["T:<"]);
        assert_eq!(tokens("x<"), ["T:x<"]);
        assert_eq!(tokens("<3 <- </ x"), ["T:<3 <- </ x"]);
        assert_eq!(tokens("a</>b"), ["T:a", "T:b"]);
    }

    #[test]
    fn comments_and_declarations_vanish() {
        assert_eq!(tokens("a<!-- x -- > y -->b"), ["T:a", "T:b"]);
        assert_eq!(tokens("<!DOCTYPE html><p>x"), ["<p>", "T:x"]);
        assert_eq!(tokens("<?xml version=\"1.0\"?>x"), ["T:x"]);
        assert_eq!(tokens("a<!-->b<!--->c"), ["T:a", "T:b", "T:c"]);
        assert_eq!(tokens("<!--[if mso]><p>only word</p><![endif]-->shown"), ["T:shown"]);
        assert_eq!(tokens("<![if !mso]>shown<![endif]>"), ["T:shown"]);
        // An unfinished comment swallows the rest.
        assert_eq!(tokens("a<!-- b <p>c"), ["T:a"]);
    }

    #[test]
    fn script_and_style_content_is_dropped() {
        assert_eq!(tokens("a<script>if (a<b) alert('</p>')</script>b"), ["T:a", "T:b"]);
        assert_eq!(tokens("a<STYLE type=text/css>p > b { color: red }</Style >b"), ["T:a", "T:b"]);
        assert_eq!(tokens("<title>Hello</title>x"), ["T:x"]);
        assert_eq!(tokens("a<script>never closed <p>x"), ["T:a"]);
        // `</scripty>` is not the end tag.
        assert_eq!(tokens("<script>x</scripty>y</script>z"), ["T:z"]);
    }

    #[test]
    fn textarea_content_is_text() {
        assert_eq!(tokens("<textarea><b>x</b></textarea>y"), ["<textarea>", "T:<b>x</b>", "</textarea>", "T:y"]);
        assert_eq!(tokens("<textarea></textarea>"), ["<textarea>", "</textarea>"]);
        assert_eq!(tokens("<xmp>a<p>b"), ["<xmp>", "T:a<p>b"]);
    }

    #[test]
    fn unfinished_tags_run_to_the_end() {
        assert_eq!(tokens("a<b"), ["T:a", "<b>"]);
        assert_eq!(tokens("a<a href=\"x"), ["T:a", "<a href=x>"]);
        assert_eq!(tokens("a</b"), ["T:a", "</b>"]);
        assert_eq!(tokens("a<b c"), ["T:a", "<b c=>"]);
        assert_eq!(tokens("a<b c="), ["T:a", "<b c=>"]);
    }

    #[test]
    fn non_ascii_everywhere() {
        assert_eq!(tokens("é<b é=ü ñ>ß</b>ø"), ["T:é", "<b é=ü ñ=>", "T:ß", "</b>", "T:ø"]);
        assert_eq!(tokens("<é>"), ["T:<é>"]);
        assert_eq!(tokens("<bé>x"), ["<bé>", "T:x"]);
    }

    #[test]
    fn long_names_and_many_attributes_are_capped() {
        let long = format!("<{}>x", "a".repeat(100));
        assert_eq!(tokens(&long), ["<>", "T:x"]);
        let many = format!("<p {}>", (0..100).map(|i| format!("a{i}=1")).collect::<Vec<_>>().join(" "));
        let Some(Token::Open(tag)) = Tokenizer::new(&many).next() else { panic!("no tag") };
        assert_eq!(tag.attrs.len(), MAX_ATTRS);
    }

    #[test]
    fn hostile_runs_are_linear() {
        for s in [
            "<".repeat(300_000),
            "<a ".repeat(100_000),
            "<!--".repeat(100_000),
            "</".repeat(150_000),
            "<script>".repeat(50_000),
            "<b x=\"".repeat(50_000),
        ] {
            let n = Tokenizer::new(&s).count();
            assert!(n <= s.len());
        }
    }

    proptest::proptest! {
        #[test]
        fn any_input_tokenizes_and_keeps_the_text(s in "[<>/!a-c =\"'&;é-]{0,80}") {
            let total: usize = Tokenizer::new(&s).map(|t| match t { Token::Text(t) => t.len(), _ => 0 }).sum();
            proptest::prop_assert!(total <= s.len());
        }
    }
}
