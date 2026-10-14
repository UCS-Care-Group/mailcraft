//! Bytes to text, for the character sets mail still arrives in.
//!
//! The names are the ones the WHATWG Encoding Standard knows (what browsers read web pages and
//! mail in), with a few more that mail programs write and that standard leaves out (`cp932`,
//! `cp936`, `cp949`, `cp950`, `euc-cn`…). The decoding itself is `encoding_rs`: UTF-8, UTF-16,
//! the Windows code pages 874 and 1250–1258, ISO-8859-2 to -16, KOI8-R and -U, IBM866, the Mac
//! encodings, Shift_JIS, EUC-JP, ISO-2022-JP, GBK, GB18030, Big5 and EUC-KR. What doesn't fit its
//! character set comes out as U+FFFD; nothing a message holds makes it fail.

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};

/// Longest label looked up; no character set's name comes close.
const MAX_LABEL: usize = 64;

/// Text from bytes in a named character set.
///
/// ISO-8859-1 is read as Windows-1252, its superset, because mail that says Latin-1 very often
/// holds curly quotes and euro signs (browsers do the same). A missing name, or US-ASCII with
/// bytes above 0x7F, is guessed with [`guess`]. `UTF-16` without a byte order mark is read in the
/// byte order its text points to. A character set nobody knows (or one only ever named to be
/// refused, such as ISO-2022-KR) is read as UTF-8, with U+FFFD for what doesn't fit.
pub(crate) fn decode(bytes: &[u8], label: &str) -> String {
    let label = clean(label);
    match label.as_str() {
        "" | "us-ascii" | "ascii" | "ansi_x3.4-1968" | "iso646-us" | "us" | "csascii" => {
            return if bytes.is_ascii() { String::from_utf8_lossy(bytes).into_owned() } else { guess(bytes) };
        }
        // UTF-16 without saying which end comes first (RFC 2781).
        "utf-16" | "ucs-2" | "unicode" | "csunicode" | "iso-10646-ucs-2" | "unicodefeff" => return utf16(bytes),
        _ => {}
    }
    let Some(encoding) = encoding(&label) else { return String::from_utf8_lossy(bytes).into_owned() };
    // UTF-8 keeps a byte order mark as the character it is (a body's first one is dropped by
    // the reader, and a header that starts with one is written back the same).
    if encoding == UTF_8 || (encoding.is_ascii_compatible() && bytes.is_ascii()) {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    // UTF-16's byte order mark is no part of the text; a mark of another character set doesn't
    // overrule the name.
    encoding.decode_with_bom_removal(bytes).0.into_owned()
}

/// The label as it is looked up: no quotes or white space around it, no RFC 2231 language
/// (`iso-2022-jp*ja`), lower case, and not absurdly long.
fn clean(label: &str) -> String {
    let label = label.trim().trim_matches(['"', '\'']).trim();
    let label = label.split('*').next().unwrap_or(label);
    label.chars().take(MAX_LABEL).collect::<String>().to_ascii_lowercase()
}

/// The character set a (cleaned) label names, when it is one that can be read.
fn encoding(label: &str) -> Option<&'static Encoding> {
    let label = match label {
        // Names mail programs write that the WHATWG standard doesn't list, for the same tables.
        "cp932" | "ms_932" | "windows-932" | "x-ms-cp932" | "shift-jisx0213" | "shift_jisx0213" => "shift_jis",
        "cp936" | "ms936" | "windows-936" | "euc-cn" | "x-euc-cn" => "gbk",
        "cp949" | "uhc" | "ks_c_5601" | "x-windows-949" => "euc-kr",
        "cp950" | "windows-950" | "x-windows-950" | "big5hkscs" => "big5",
        "x-euc-jp" | "eucjp" | "euc_jp" => "euc-jp",
        "iso-2022-jp-1" | "iso-2022-jp-2" | "iso-2022-jp-3" | "iso-2022-jp-ms" | "cp50220" | "cp50221" | "cp50222" => "iso-2022-jp",
        "latin-1" | "iso8859_1" => "iso-8859-1",
        "latin-9" | "latin9" | "iso8859_15" => "iso-8859-15",
        "latin-2" => "iso-8859-2",
        "cp-1252" => "windows-1252",
        "koi8r" => "koi8-r",
        "koi8u" => "koi8-u",
        "utf-8n" => "utf-8",
        other => other,
    };
    // `replacement` stands for character sets the standard refuses to read (ISO-2022-KR,
    // ISO-2022-CN, HZ): it would turn the whole text into one U+FFFD.
    Encoding::for_label_no_replacement(label.as_bytes())
}

/// UTF-16 that doesn't say which byte comes first: a byte order mark says, else the text does.
/// RFC 2781 makes big-endian the default, but programs write little-endian without a mark often
/// enough that the zero bytes decide: in text that is mostly Latin, every other byte is zero, and
/// which ones shows the order. Without a clue, big-endian.
fn utf16(bytes: &[u8]) -> String {
    let encoding = match bytes {
        [0xFF, 0xFE, ..] => UTF_16LE,
        [0xFE, 0xFF, ..] => UTF_16BE,
        _ => {
            let (mut high_first, mut low_first) = (0usize, 0usize);
            for [a, b] in bytes.as_chunks::<2>().0.iter().take(4096) {
                high_first += usize::from(*a == 0 && *b != 0);
                low_first += usize::from(*b == 0 && *a != 0);
            }
            if low_first > high_first { UTF_16LE } else { UTF_16BE }
        }
    };
    encoding.decode_with_bom_removal(bytes).0.into_owned()
}

/// Text from bytes that don't say what they are: UTF-8 when they are valid UTF-8, else Windows-1252.
pub(crate) fn guess(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => WINDOWS_1252.decode_without_bom_handling(bytes).0.into_owned(),
    }
}

/// Line endings as `\n`: CRLF and a lone CR both become LF.
pub(crate) fn unix_lines(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_and_ascii() {
        assert_eq!(decode("héllo €".as_bytes(), "UTF-8"), "héllo €");
        assert_eq!(decode("héllo".as_bytes(), "utf8"), "héllo");
        assert_eq!(decode(b"plain", "x-unknown"), "plain");
        assert_eq!(decode(b"a\xFFb", "utf-8"), "a\u{fffd}b");
        assert_eq!(decode(b"", "utf-8"), "");
    }

    #[test]
    fn latin1_is_read_as_windows_1252() {
        assert_eq!(decode(b"caf\xE9 \xA4", "ISO-8859-1"), "café ¤");
        assert_eq!(decode(b"\x93hi\x94 \x80 \x85 \x99", "iso-8859-1"), "“hi” € … ™");
        assert_eq!(decode(b"\x93hi\x94 \x80", "\"Windows-1252\""), "“hi” €");
        assert_eq!(decode(b"\x81\x8D\x8F\x90\x9D", "cp1252"), "\u{81}\u{8d}\u{8f}\u{90}\u{9d}");
        assert_eq!(decode(b"caf\xE9", "latin-1"), "café");
        // Every byte gives a character.
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(&all, "windows-1252").chars().count(), 256);
    }

    #[test]
    fn latin9_has_the_euro() {
        assert_eq!(decode(b"\xA4 \xBC\xBD \xE9 \xA6\xA8\xB4\xB8\xBE", "ISO-8859-15"), "€ Œœ é ŠšŽžŸ");
        assert_eq!(decode(b"\x80", "latin9"), "\u{80}");
    }

    #[test]
    fn unlabelled_bytes_are_guessed() {
        assert_eq!(decode("naïve".as_bytes(), ""), "naïve");
        assert_eq!(decode(b"na\xEFve", ""), "naïve");
        assert_eq!(decode(b"na\xEFve", "us-ascii"), "naïve");
        assert_eq!(guess(b"\xE2\x82\xAC"), "€");
        assert_eq!(guess(b"\xE2\x82"), "\u{e2}\u{201a}");
    }

    #[test]
    fn unknown_charsets_fall_back_to_lossy_utf8() {
        assert_eq!(decode("grüße".as_bytes(), "x-made-up"), "grüße");
        assert_eq!(decode(b"\xC0\xC1", "x-made-up"), "\u{fffd}\u{fffd}");
        // Character sets the WHATWG standard only names to refuse don't swallow the text.
        assert_eq!(decode("grüße".as_bytes(), "iso-2022-kr"), "grüße");
        assert_eq!(decode(b"plain", "hz-gb-2312"), "plain");
        assert_eq!(decode("a\u{e9}".as_bytes(), &"x".repeat(10_000)), "a\u{e9}");
    }

    #[test]
    fn utf16_with_and_without_a_byte_order_mark() {
        let le: Vec<u8> = "Grüße €".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let be: Vec<u8> = "Grüße €".encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert_eq!(decode(&[[0xFF, 0xFE].as_slice(), &le].concat(), "UTF-16"), "Grüße €");
        assert_eq!(decode(&[[0xFE, 0xFF].as_slice(), &be].concat(), "utf-16"), "Grüße €");
        // No mark: the text says which way round it is.
        assert_eq!(decode(&le, "utf-16"), "Grüße €");
        assert_eq!(decode(&be, "utf-16"), "Grüße €");
        assert_eq!(decode(&le, "UTF-16LE"), "Grüße €");
        assert_eq!(decode(&be, "utf-16be"), "Grüße €");
        assert_eq!(decode(&[[0xFE, 0xFF].as_slice(), &be].concat(), "utf-16be"), "Grüße €");
        // ASCII bytes are not ASCII text in UTF-16.
        assert_eq!(decode(b"\0h\0i", "utf-16"), "hi");
        // An odd byte at the end is a broken character, not a crash.
        assert_eq!(decode(b"\0h\0", "utf-16be"), "h\u{fffd}");
    }

    #[test]
    fn japanese() {
        // 日本語 (Japanese), in each of the three forms Japanese mail uses.
        assert_eq!(decode(b"\x93\xFA\x96\x7B\x8C\xEA", "Shift_JIS"), "日本語");
        assert_eq!(decode(b"\x93\xFA\x96\x7B\x8C\xEA", "cp932"), "日本語");
        assert_eq!(decode(b"\xC6\xFC\xCB\xDC\xB8\xEC", "EUC-JP"), "日本語");
        // ISO-2022-JP is all 7-bit bytes, switched with escapes.
        assert_eq!(decode(b"\x1B$BF|K\\8l\x1B(B ok", "ISO-2022-JP"), "日本語 ok");
        assert_eq!(decode(b"\x1B$BF|K\\8l\x1B(B", "\"iso-2022-jp\""), "日本語");
        assert_eq!(decode(b"\x1B$BF|K\\8l\x1B(B", "iso-2022-jp*ja"), "日本語");
    }

    #[test]
    fn chinese_and_korean() {
        // 中文 (Chinese), simplified.
        assert_eq!(decode(b"\xD6\xD0\xCE\xC4", "GB2312"), "中文");
        assert_eq!(decode(b"\xD6\xD0\xCE\xC4", "gbk"), "中文");
        assert_eq!(decode(b"\xD6\xD0\xCE\xC4", "GB18030"), "中文");
        assert_eq!(decode(b"\xD6\xD0\xCE\xC4", "cp936"), "中文");
        // 中文, traditional.
        assert_eq!(decode(b"\xA4\xA4\xA4\xE5", "Big5"), "中文");
        // 한국어 (Korean).
        assert_eq!(decode(b"\xC7\xD1\xB1\xB9\xBE\xEE", "EUC-KR"), "한국어");
        assert_eq!(decode(b"\xC7\xD1\xB1\xB9\xBE\xEE", "ks_c_5601-1987"), "한국어");
    }

    #[test]
    fn cyrillic_greek_and_others() {
        // Привет (hello, Russian).
        assert_eq!(decode(b"\xF0\xD2\xC9\xD7\xC5\xD4", "KOI8-R"), "Привет");
        assert_eq!(decode(b"\xCF\xF0\xE8\xE2\xE5\xF2", "windows-1251"), "Привет");
        assert_eq!(decode(b"\xBF\xE0\xD8\xD2\xD5\xE2", "ISO-8859-5"), "Привет");
        // Ґ (Ukrainian), only in KOI8-U.
        assert_eq!(decode(b"\xBD", "koi8-u"), "Ґ");
        // Łódź (Polish).
        assert_eq!(decode(b"\xA3\xF3d\xBC", "ISO-8859-2"), "Łódź");
        assert_eq!(decode(b"\xA3\xF3d\x9F", "windows-1250"), "Łódź");
        // Ελλάδα (Greece).
        assert_eq!(decode(b"\xC5\xEB\xEB\xDC\xE4\xE1", "iso-8859-7"), "Ελλάδα");
        // İstanbul, ş (Turkish).
        assert_eq!(decode(b"\xDDstanbul \xFE", "windows-1254"), "İstanbul ş");
        // Hebrew, Arabic, Baltic, Vietnamese and Thai code pages are known too.
        assert_eq!(decode(b"\xF9\xEC\xE5\xED", "windows-1255"), "שלום");
        assert_eq!(decode(b"\xC7", "windows-1256"), "ا");
        assert_eq!(decode(b"\xE0", "windows-1257"), "ą");
        assert_eq!(decode(b"\xD2", "windows-1258"), "\u{309}");
        assert_eq!(decode(b"\xA1", "TIS-620"), "ก");
        assert_eq!(decode(b"\xA4", "ISO-8859-16"), "€");
    }

    #[test]
    fn a_byte_order_mark_doesnt_overrule_the_label() {
        assert_eq!(decode(b"\xEF\xBB\xBFhi \xC3\xA9", "utf-8"), "\u{feff}hi é");
        // A UTF-8 mark at the start of Windows-1252 text is three Latin characters.
        assert_eq!(decode(b"\xEF\xBB\xBF", "windows-1252"), "ï»¿");
    }

    #[test]
    fn line_endings() {
        assert_eq!(unix_lines("a\r\nb\rc\nd\r\n"), "a\nb\nc\nd\n");
        assert_eq!(unix_lines("\r\r\n\r"), "\n\n\n");
        assert_eq!(unix_lines("none"), "none");
    }

    const LABELS: &[&str] = &[
        "",
        "us-ascii",
        "utf-8",
        "utf-16",
        "utf-16le",
        "utf-16be",
        "iso-8859-1",
        "iso-8859-2",
        "iso-8859-5",
        "iso-8859-7",
        "iso-8859-8",
        "iso-8859-15",
        "iso-8859-16",
        "windows-1250",
        "windows-1251",
        "windows-1252",
        "windows-1253",
        "windows-1254",
        "windows-1255",
        "windows-1256",
        "windows-1257",
        "windows-1258",
        "windows-874",
        "koi8-r",
        "koi8-u",
        "ibm866",
        "macintosh",
        "shift_jis",
        "cp932",
        "euc-jp",
        "iso-2022-jp",
        "gbk",
        "gb2312",
        "gb18030",
        "big5",
        "euc-kr",
        "cp949",
        "iso-2022-kr",
        "x-user-defined",
        "x-unknown",
    ];

    proptest::proptest! {
        #[test]
        fn any_bytes_in_any_character_set_never_panic(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..300), label in "\\PC{0,30}") {
            let _ = decode(&bytes, &label);
        }

        #[test]
        fn any_bytes_in_every_known_character_set_never_panic(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..300), pick in 0usize..LABELS.len()) {
            let _ = decode(&bytes, LABELS.get(pick).copied().unwrap_or("utf-8"));
        }
    }
}
