//! Windows code page numbers (what `PidTagMessageCodepage`, `PidTagInternetCodepage` and RTF's
//! `\ansicpg` give) to the character set names the MIME reader decodes.

/// The character set a code page is, when MailCraft can read it.
pub fn label(codepage: u32) -> Option<&'static str> {
    Some(match codepage {
        65001 => "utf-8",
        1200 => "utf-16le",
        1201 => "utf-16be",
        20127 => "us-ascii",
        874 => "windows-874",
        1250 => "windows-1250",
        1251 => "windows-1251",
        1252 => "windows-1252",
        1253 => "windows-1253",
        1254 => "windows-1254",
        1255 => "windows-1255",
        1256 => "windows-1256",
        1257 => "windows-1257",
        1258 => "windows-1258",
        866 => "ibm866",
        932 => "shift_jis",
        936 => "gbk",
        949 | 51949 => "euc-kr",
        950 => "big5",
        10000 => "macintosh",
        20866 => "koi8-r",
        21866 => "koi8-u",
        28591 => "iso-8859-1",
        28592 => "iso-8859-2",
        28593 => "iso-8859-3",
        28594 => "iso-8859-4",
        28595 => "iso-8859-5",
        28596 => "iso-8859-6",
        28597 => "iso-8859-7",
        28598 => "iso-8859-8",
        38598 => "iso-8859-8-i",
        28603 => "iso-8859-13",
        28605 => "iso-8859-15",
        50220..=50222 => "iso-2022-jp",
        51932 | 20932 => "euc-jp",
        54936 => "gb18030",
        _ => return None,
    })
}

/// Text from bytes in a code page; one MailCraft can't read is taken as Windows-1252 (when the
/// bytes aren't valid UTF-8).
pub fn decode(bytes: &[u8], codepage: u32) -> String {
    match label(codepage) {
        Some(label) => mailcraft_mime::decode_charset(bytes, label),
        None => mailcraft_mime::decode_charset(bytes, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_pages() {
        assert_eq!(decode(b"caf\xe9", 1252), "café");
        assert_eq!(decode("café".as_bytes(), 65001), "café");
        assert_eq!(decode(b"\xcf\xf0\xe8", 1251), "При");
        assert_eq!(decode(b"\x82\xa0", 932), "あ");
        assert_eq!(decode(b"caf\xe9", 0), "café");
        assert_eq!(decode(b"h\0i\0", 1200), "hi");
    }
}
