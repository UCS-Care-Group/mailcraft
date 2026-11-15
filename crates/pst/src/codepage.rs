// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Windows code pages by the name `mailcraft-charset` knows them by: one table for the mail,
//! the contacts and the rich text of an import.

/// The character set a Windows code page stands for (PidTagMessageCodepage,
/// PidTagInternetCodepage), as `mailcraft-charset` names it.
pub fn code_page_label(cp: i64) -> Option<&'static str> {
    Some(match cp {
        65001 => "utf-8",
        1200 => "utf-16le",
        1201 => "utf-16be",
        874 => "windows-874",
        1250 => "windows-1250",
        1251 => "windows-1251",
        1252 | 28591 => "windows-1252",
        1253 => "windows-1253",
        1254 => "windows-1254",
        1255 => "windows-1255",
        1256 => "windows-1256",
        1257 => "windows-1257",
        1258 => "windows-1258",
        932 => "shift_jis",
        936 => "gbk",
        54936 => "gb18030",
        949 | 51949 => "euc-kr",
        950 => "big5",
        51932 => "euc-jp",
        50220..=50222 => "iso-2022-jp",
        866 => "ibm866",
        20866 => "koi8-r",
        21866 => "koi8-u",
        10000 => "macintosh",
        10007 => "x-mac-cyrillic",
        28592 => "iso-8859-2",
        28593 => "iso-8859-3",
        28594 => "iso-8859-4",
        28595 => "iso-8859-5",
        28596 => "iso-8859-6",
        28597 => "iso-8859-7",
        28598 => "iso-8859-8",
        38598 => "iso-8859-8-i",
        28599 => "iso-8859-9",
        28603 => "iso-8859-13",
        28605 => "iso-8859-15",
        20127 => "us-ascii",
        _ => return None,
    })
}
