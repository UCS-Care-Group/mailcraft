// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening an attachment with the program the computer opens that kind of file with (#126).
//!
//! The engine never opens anything: a message is hostile, and so is the name its sender gave a
//! file. This is what the desktop host and the window go by before they do:
//! - [`verdict`]: programs, scripts, shortcuts, installers and disk images are never opened
//!   (they can run as soon as they open); Office files with macros and web pages ask first
//!   (macros and scripts are what they are for); anything else opens, except on Linux and the
//!   BSDs, where only a kind the desktop knows by name ([`KNOWN`]) does, and the host opens it
//!   only when the desktop types the file as that kind: there a kind it doesn't know is typed by
//!   what is in the file, and the sender chose that.
//! - [`file_name`]: the name the file is written under, from the attachment's own name, made
//!   safe to put in a folder: no folders of its own, nothing hidden, nothing that reads as an
//!   option, no name Windows keeps for itself, not too long.
//!
//! `mail.openAttachment` (`cmd::mail`) answers with both and opens nothing itself: the window
//! does the opening, through the host (`Services::open_file`), and agents may not ask for it.

/// What may be done with an attachment the owner asked to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Open it.
    Open,
    /// An Office file that can carry macros: ask first.
    Macros,
    /// A web page or a drawing that can carry scripts, which open in the browser: ask first.
    Scripts,
    /// A file the program that opens it acts on: Markdown, whose viewers can show the web content
    /// in it, and keys, certificates and signatures, which their programs can take in when opened:
    /// ask first.
    Active,
    /// A program, a script, a shortcut, an installer or a disk image: never opened from mail.
    Blocked,
}

impl Verdict {
    /// Whether the owner is asked before it opens.
    pub fn asks(self) -> bool {
        matches!(self, Verdict::Macros | Verdict::Scripts | Verdict::Active)
    }
}

/// Kinds of file that run (or run what is in them) when they are opened. Windows, macOS, Linux
/// and the BSDs together; a file of one of these is saved, never opened.
pub const BLOCKED: &[&str] = &[
    // Windows programs, libraries, control panel items, screen savers, installers and updates.
    "exe",
    "com",
    "scr",
    "pif",
    "cpl",
    "dll",
    "ocx",
    "sys",
    "drv",
    "msi",
    "msp",
    "mst",
    "msu",
    "msix",
    "msixbundle",
    "appx",
    "appxbundle",
    "application",
    "appref-ms",
    "appinstaller",
    // More kinds the shell runs or hands to a program that runs what is in them.
    "mad",
    "appcontent-ms",
    "vsmacros",
    "bgi",
    "asx",
    "mcf",
    "pcd",
    "plg",
    "osd",
    "gadget",
    "xbap",
    "xll",
    "diagcab",
    "msc",
    "ins",
    "isp",
    "inf",
    "reg",
    "scf",
    // Windows scripts and their hosts.
    "bat",
    "cmd",
    "ps1",
    "ps1xml",
    "ps2",
    "ps2xml",
    "psc1",
    "psc2",
    "psd1",
    "psm1",
    "pssc",
    "msh",
    "msh1",
    "msh2",
    "mshxml",
    "msh1xml",
    "msh2xml",
    "vb",
    "vbe",
    "vbs",
    "vbp",
    "js",
    "jse",
    "ws",
    "wsc",
    "wsf",
    "wsh",
    "wsb",
    "hta",
    "htc",
    "sct",
    "shb",
    "shs",
    "chm",
    "hlp",
    "cnt",
    "mde",
    "mda",
    "mdb",
    "mdt",
    "mdw",
    "mdz",
    "ade",
    "adp",
    "accde",
    "bas",
    "fxp",
    "prg",
    // Certificates, certificate requests' answers, stores and trust lists: Windows takes them into
    // its certificate store from a double-click.
    "crt",
    "cer",
    "der",
    "p7r",
    "sst",
    "stl",
    "spc",
    "udl",
    "printerexport",
    "settingcontent-ms",
    "theme",
    "themepack",
    "webpnp",
    "website",
    // Shortcuts and links that point somewhere else.
    "lnk",
    "url",
    "library-ms",
    "search-ms",
    "searchconnector-ms",
    "glk",
    "xnk",
    "jnlp",
    // Files that connect, fetch or run when Windows or Office opens them: remote desktop, web
    // queries and symbolic links of spreadsheets, Office add-ins, Access shortcuts, Outlook
    // profiles, help projects and Program Manager groups.
    "rdp",
    "iqy",
    "slk",
    "vsto",
    "mam",
    "maf",
    "mag",
    "maq",
    "mar",
    "mas",
    "mat",
    "mau",
    "mav",
    "maw",
    "prf",
    "hpj",
    "grp",
    // Disk images, which open as a drive with whatever is in them.
    "iso",
    "img",
    "vhd",
    "vhdx",
    "dmg",
    "sparseimage",
    "sparsebundle",
    // Java, Python, Perl, Ruby, PHP, Lua and the shells.
    "jar",
    "class",
    "py",
    "pyc",
    "pyo",
    "pyw",
    "pyz",
    "pyzw",
    "pl",
    "pm",
    "rb",
    "php",
    "lua",
    "tcl",
    "sh",
    "bash",
    "zsh",
    "ksh",
    "csh",
    "tcsh",
    "fish",
    "command",
    "tool",
    "run",
    "bin",
    "out",
    "elf",
    // macOS programs, installers, scripts and workflows.
    "app",
    "pkg",
    "mpkg",
    "scpt",
    "scptd",
    "applescript",
    "workflow",
    "action",
    "terminal",
    "webloc",
    "inetloc",
    "fileloc",
    "dylib",
    "kext",
    "prefpane",
    "osax",
    "plugin",
    "bundle",
    // Linux and BSD programs, installers and launchers.
    "desktop",
    "appimage",
    "flatpak",
    "flatpakref",
    "snap",
    "deb",
    "rpm",
    "so",
    "ko",
    // Add-in libraries, provisioning and profile packages, task and management files, and the
    // connection files and queries that connect when opened.
    "wll",
    "ppkg",
    "provxml",
    "job",
    "ops",
    "mof",
    "inx",
    "dqy",
    "oqy",
    "rqy",
    "ica",
    "mobileconfig",
    // Phone app packages and bundles: installed by sideloading, never opened from mail.
    "apk",
    "apks",
    "xapk",
    "aab",
    "ipa",
    "mobileprovision",
    "rdg",
    "term",
    "wflow",
    "flatpakrepo",
    "cdxml",
    "swf",
    // Theme packs, location files that connect to a server, plug-ins and kinds that open in a
    // program that runs what they hold.
    "deskthemepack",
    "afploc",
    "ftploc",
    "vncloc",
    "wcx",
    "its",
    "cdr",
    "smi",
    // MailCraft's own mailbox: its accounts name servers, so one sent in mail is saved, never
    // opened from the message.
    "mailcraft",
];

/// Office files that can carry macros.
pub const MACROS: &[&str] = &[
    "docm", "dotm", "xlsm", "xltm", "xlam", "xlsb", "xla", "xlm", "pptm", "potm", "ppam", "ppsm", "sldm", "accdb", "one", "onepkg", "vsdm", "vstm",
    "vssm", "doc", "dot", "xls", "xlt", "xlw", "ppt", "pot", "pps", "ppa", "pub", "wbk", "vsd", "vss", "vst", "vdx",
];

/// Whether a file of this kind (the part after the last dot, in lower case) runs, or runs what is
/// in it, when it is opened: the one list ([`BLOCKED`]) the opener refuses by and the files an
/// agent may not have written are refused by.
pub fn is_launchable(ext: &str) -> bool {
    BLOCKED.contains(&ext)
}

/// Web pages and drawings that open in the browser, where what is in them runs.
pub const SCRIPTS: &[&str] = &["htm", "html", "xhtml", "shtml", "mht", "mhtml", "svg", "svgz", "xht", "xml", "xsl"];

/// Files the program that opens them acts on ([`Verdict::Active`]): Markdown, and keys,
/// certificates and signatures.
pub const ACTIVE: &[&str] = &[
    // Markdown.
    "md", "markdown", "mkd", "mdown", "mdwn", "mkdn", "mmd", "mdx", // Keys, certificates, signatures and requests.
    "asc", "p7m", "p7s", "pem", "sig", "p12", "pfx", "p7b", "p7c", "gpg", "pgp", "key", "csr",
    "crl",
    // (`.key` is also a Keynote deck on macOS: asking first only adds a question there.)
];

/// Whether a file of this kind (the part after the last dot, in lower case) is one the opener
/// refuses or asks about first ([`BLOCKED`], [`MACROS`], [`SCRIPTS`] and [`ACTIVE`] together): the one list
/// the files an agent may not have written are refused by.
pub fn is_risky(ext: &str) -> bool {
    !ext.is_ascii() || BLOCKED.contains(&ext) || MACROS.contains(&ext) || SCRIPTS.contains(&ext) || ACTIVE.contains(&ext)
}

/// Names Windows keeps for devices: a file with one of these before its first dot is no file.
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "com¹", "com²", "com³", "lpt1", "lpt2",
    "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9", "lpt¹", "lpt²", "lpt³", "conin$", "conout$",
];

/// The longest a file name is made, in characters (a folder's path takes the rest).
pub const MAX_NAME: usize = 120;

/// The kind of a file name: what is after its last dot, folded to lower case by ASCII rules only
/// (a letter like `ſ` or `ı` is never folded into an ASCII one; empty when nothing).
pub fn extension(name: &str) -> String {
    let name = name.trim_end_matches(['.', ' ']);
    match name.rsplit_once('.') {
        Some((_, ext)) => ext.trim().to_ascii_lowercase(),
        None => String::new(),
    }
}

/// How the computer chooses the program that opens a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chooser {
    /// By its kind alone; a kind no program is set for opens none (Windows, macOS, phones).
    Name,
    /// By its kind when the desktop knows it, and by what is in it when it doesn't (Linux and the
    /// BSDs: xdg-open, GIO and KDE go by the shared MIME database, which falls back to the
    /// file's first bytes): only a kind of [`KNOWN`] is opened there.
    NameOrContent,
}

impl Chooser {
    /// The way of the computer this is built for.
    pub const THIS: Chooser =
        if cfg!(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android")))) { Chooser::NameOrContent } else { Chooser::Name };
}

/// Kinds the shared MIME database of Linux and the BSDs knows by name, each with the types (and
/// their older names) the desktop may say a file of that kind is: the kind's own, those of the
/// doubled kinds that end in it (`.tar.gz`, `.pdf.xz`) and, where the name has two types (a
/// `.png` can be `image/png` or `image/apng`) and GIO decides by what is in the file, the more
/// particular types of those (an Opus `.ogg`). With [`Chooser::NameOrContent`] no other kind is
/// opened, and the host opens a file only when the desktop types it as one of these, so a doubled
/// kind left out (a disk image `.img.xz`, a compressed PostScript or drawing) is refused. The
/// kinds of [`MACROS`], [`SCRIPTS`] and [`ACTIVE`] here still ask first.
pub const KNOWN: &[(&str, &[&str])] = &[
    // Documents and text.
    ("pdf", &["application/pdf", "application/acrobat", "application/nappdf", "application/x-pdf", "image/pdf"]),
    ("txt", &["text/plain"]),
    ("csv", &["text/csv", "text/x-comma-separated-values", "text/x-csv"]),
    ("tsv", &["text/tab-separated-values"]),
    ("rtf", &["application/rtf", "text/rtf"]),
    ("odt", &["application/vnd.oasis.opendocument.text"]),
    ("ods", &["application/vnd.oasis.opendocument.spreadsheet"]),
    ("odp", &["application/vnd.oasis.opendocument.presentation"]),
    ("odg", &["application/vnd.oasis.opendocument.graphics"]),
    ("odf", &["application/vnd.oasis.opendocument.formula"]),
    ("ott", &["application/vnd.oasis.opendocument.text-template"]),
    ("ots", &["application/vnd.oasis.opendocument.spreadsheet-template"]),
    ("otp", &["application/vnd.oasis.opendocument.presentation-template"]),
    ("otg", &["application/vnd.oasis.opendocument.graphics-template"]),
    ("docx", &["application/vnd.openxmlformats-officedocument.wordprocessingml.document"]),
    ("dotx", &["application/vnd.openxmlformats-officedocument.wordprocessingml.template"]),
    ("xlsx", &["application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"]),
    ("xltx", &["application/vnd.openxmlformats-officedocument.spreadsheetml.template"]),
    ("pptx", &["application/vnd.openxmlformats-officedocument.presentationml.presentation"]),
    ("potx", &["application/vnd.openxmlformats-officedocument.presentationml.template"]),
    ("ppsx", &["application/vnd.openxmlformats-officedocument.presentationml.slideshow"]),
    ("epub", &["application/epub+zip"]),
    ("tex", &["text/x-tex", "application/x-tex"]),
    ("log", &["text/x-log"]),
    (
        "json",
        &[
            "application/json",
            "application/schema+json",
            "application/har+json",
            "application/jrd+json",
            "application/vnd.cyclonedx+json",
            "application/ld+json",
            "application/json-patch+json",
            "application/spdx+json",
            "application/geo+json",
            "application/vnd.geo+json",
        ],
    ),
    ("yaml", &["application/yaml", "application/x-yaml", "text/x-yaml", "text/yaml"]),
    ("yml", &["application/yaml", "application/x-yaml", "text/x-yaml", "text/yaml"]),
    ("xps", &["application/vnd.ms-xpsdocument", "application/xps"]),
    ("djvu", &["image/vnd.djvu", "image/x-djvu", "image/x.djvu", "image/vnd.djvu+multipage"]),
    ("fb2", &["application/x-fictionbook+xml", "application/x-fictionbook"]),
    ("markdown", &["text/markdown", "text/x-markdown"]),
    ("md", &["text/markdown", "text/x-markdown"]),
    ("patch", &["text/x-patch", "text/x-diff"]),
    ("diff", &["text/x-patch", "text/x-diff"]),
    // Mail, calendars and cards.
    ("eml", &["message/rfc822"]),
    ("mbox", &["application/mbox"]),
    ("ics", &["text/calendar", "application/ics", "text/x-vcalendar"]),
    ("vcs", &["text/calendar", "application/ics", "text/x-vcalendar"]),
    ("vcf", &["text/vcard", "text/directory", "text/x-vcard"]),
    ("vcard", &["text/vcard", "text/directory", "text/x-vcard"]),
    // Signatures, keys and certificates (shown, never run).
    ("p7m", &["application/pkcs7-mime", "application/x-pkcs7-certificates"]),
    ("p7s", &["application/pkcs7-signature"]),
    ("asc", &["application/pgp-encrypted", "application/pgp", "application/pgp-keys", "application/pgp-signature", "text/plain"]),
    ("sig", &["application/pgp-signature"]),
    (
        "pem",
        &[
            "application/pkcs10",
            "application/pkcs12",
            "application/x-pkcs12",
            "application/pkcs7-mime",
            "application/x-pkcs7-certificates",
            "application/pkcs7-signature",
            "application/pkcs8",
            "application/pkcs8-encrypted",
            "application/pkix-cert",
            "application/x-x509-ca-cert",
            "application/x-x509-user-cert",
            "application/pkix-crl",
            "application/x-pem-file",
            "application/pkcs12+pem",
            "application/pkcs7-mime+pem",
            "application/pkcs8+pem",
            "application/pkcs10+pem",
            "application/x-pem-key",
            "application/pkix-crl+pem",
            "application/pkix-cert+pem",
        ],
    ),
    // Pictures.
    ("jpg", &["image/jpeg", "image/pjpeg"]),
    ("jpeg", &["image/jpeg", "image/pjpeg"]),
    ("jpe", &["image/jpeg", "image/pjpeg"]),
    ("png", &["image/apng", "image/vnd.mozilla.apng", "image/png"]),
    ("gif", &["image/gif"]),
    ("bmp", &["image/bmp", "image/x-MS-bmp", "image/x-bmp"]),
    ("tif", &["image/tiff"]),
    ("tiff", &["image/tiff"]),
    ("webp", &["image/webp"]),
    ("heic", &["image/heif", "image/heic", "image/heic-sequence", "image/heif-sequence"]),
    ("heif", &["image/heif", "image/heic", "image/heic-sequence", "image/heif-sequence"]),
    ("avif", &["image/avif", "image/avif-sequence"]),
    ("jxl", &["image/jxl"]),
    ("ico", &["image/vnd.microsoft.icon", "application/ico", "image/ico", "image/icon", "image/x-ico", "image/x-icon", "text/ico"]),
    (
        "psd",
        &[
            "image/vnd.adobe.photoshop",
            "application/photoshop",
            "application/x-photoshop",
            "image/photoshop",
            "image/psd",
            "image/x-photoshop",
            "image/x-psd",
        ],
    ),
    ("xcf", &["image/x-xcf"]),
    ("cr2", &["image/x-canon-cr2"]),
    ("nef", &["image/x-nikon-nef"]),
    ("arw", &["image/x-sony-arw"]),
    ("dng", &["image/x-adobe-dng"]),
    ("orf", &["image/x-olympus-orf"]),
    ("rw2", &["image/x-panasonic-rw2", "image/x-panasonic-raw2"]),
    ("raf", &["image/x-fuji-raf"]),
    // Sound and video.
    ("mp3", &["audio/mpeg", "audio/mp3", "audio/x-mp3", "audio/x-mpeg", "audio/x-mpg"]),
    ("m4a", &["audio/mp4", "audio/m4a", "audio/x-m4a"]),
    ("aac", &["audio/aac", "audio/x-aac"]),
    (
        "ogg",
        &[
            "audio/ogg",
            "audio/x-ogg",
            "audio/x-flac+ogg",
            "audio/x-oggflac",
            "audio/x-speex+ogg",
            "audio/x-vorbis+ogg",
            "audio/vorbis",
            "audio/x-vorbis",
            "video/ogg",
            "video/x-ogg",
            "video/x-theora+ogg",
            "video/x-theora",
            "video/x-ogm+ogg",
            "video/x-ogm",
            "audio/x-opus+ogg",
        ],
    ),
    (
        "oga",
        &[
            "audio/ogg",
            "audio/x-ogg",
            "audio/x-flac+ogg",
            "audio/x-oggflac",
            "audio/x-speex+ogg",
            "audio/x-vorbis+ogg",
            "audio/vorbis",
            "audio/x-vorbis",
            "audio/x-opus+ogg",
        ],
    ),
    (
        "opus",
        &[
            "audio/ogg",
            "audio/x-ogg",
            "audio/x-opus+ogg",
            "audio/x-vorbis+ogg",
            "audio/vorbis",
            "audio/x-vorbis",
            "audio/x-flac+ogg",
            "audio/x-oggflac",
            "audio/x-speex+ogg",
        ],
    ),
    ("flac", &["audio/flac", "audio/x-flac"]),
    ("wav", &["audio/vnd.wave", "audio/wav", "audio/x-wav"]),
    ("wma", &["audio/x-ms-wma", "audio/wma"]),
    ("mid", &["audio/midi", "audio/x-midi"]),
    ("midi", &["audio/midi", "audio/x-midi"]),
    ("amr", &["audio/AMR", "audio/amr-encrypted"]),
    ("mp4", &["video/mp4", "video/mp4v-es", "video/x-m4v"]),
    ("m4v", &["video/mp4", "video/mp4v-es", "video/x-m4v"]),
    ("mov", &["video/quicktime"]),
    ("avi", &["video/vnd.avi", "video/avi", "video/divx", "video/msvideo", "video/vnd.divx", "video/x-avi", "video/x-msvideo"]),
    ("mkv", &["video/matroska", "video/x-matroska"]),
    ("webm", &["video/webm"]),
    ("wmv", &["video/x-ms-wmv"]),
    ("mpg", &["video/mpeg", "video/mpeg-system", "video/x-mpeg", "video/x-mpeg-system", "video/x-mpeg2"]),
    ("mpeg", &["video/mpeg", "video/mpeg-system", "video/x-mpeg", "video/x-mpeg-system", "video/x-mpeg2"]),
    ("ogv", &["video/ogg", "video/x-ogg"]),
    (
        "3gp",
        &[
            "video/3gpp",
            "audio/3gpp",
            "audio/3gpp-encrypted",
            "audio/x-rn-3gpp-amr",
            "audio/x-rn-3gpp-amr-encrypted",
            "audio/x-rn-3gpp-amr-wb",
            "audio/x-rn-3gpp-amr-wb-encrypted",
            "video/3gp",
            "video/3gpp-encrypted",
        ],
    ),
    // Archives (with the compressed tar files and documents their doubled kinds name).
    ("zip", &["application/zip", "application/x-zip", "application/x-zip-compressed", "application/x-zip-compressed-fb2"]),
    ("7z", &["application/x-7z-compressed"]),
    ("rar", &["application/vnd.rar", "application/x-rar", "application/x-rar-compressed"]),
    ("tar", &["application/x-tar", "application/x-gtar"]),
    (
        "gz",
        &[
            "application/gzip",
            "application/x-gzip",
            "application/x-abiword",
            "application/x-compressed-tar",
            "application/x-cpio-compressed",
            "application/x-gzpdf",
            "image/x-compressed-xcf",
        ],
    ),
    ("tgz", &["application/x-compressed-tar"]),
    (
        "bz2",
        &[
            "application/x-bzip2",
            "application/bzip2",
            "application/x-bzip",
            "application/x-bzip2-compressed-tar",
            "application/x-bzip-compressed-tar",
            "application/x-bzpdf",
            "image/x-compressed-xcf",
        ],
    ),
    ("xz", &["application/x-xz", "application/x-xz-compressed-tar", "application/x-xzpdf"]),
    // Office files that can carry macros (they ask first).
    ("doc", &["application/msword", "application/vnd.ms-word", "application/x-msword"]),
    ("dot", &["application/msword-template", "text/vnd.graphviz"]),
    ("xls", &["application/vnd.ms-excel", "application/msexcel", "application/x-msexcel"]),
    ("xlt", &["application/vnd.ms-excel", "application/msexcel", "application/x-msexcel"]),
    ("ppt", &["application/vnd.ms-powerpoint", "application/mspowerpoint", "application/powerpoint", "application/x-mspowerpoint"]),
    (
        "pot",
        &[
            "application/vnd.ms-powerpoint",
            "application/mspowerpoint",
            "application/powerpoint",
            "application/x-mspowerpoint",
            "text/x-gettext-translation-template",
            "text/x-pot",
        ],
    ),
    ("pps", &["application/vnd.ms-powerpoint", "application/mspowerpoint", "application/powerpoint", "application/x-mspowerpoint"]),
    ("pub", &["application/vnd.ms-publisher", "text/x-ssh-public-key"]),
    ("vsd", &["application/vnd.visio"]),
    ("vss", &["application/vnd.visio"]),
    ("docm", &["application/vnd.ms-word.document.macroEnabled.12"]),
    ("dotm", &["application/vnd.ms-word.template.macroEnabled.12"]),
    ("xlsm", &["application/vnd.ms-excel.sheet.macroEnabled.12"]),
    ("xltm", &["application/vnd.ms-excel.template.macroEnabled.12"]),
    ("xlam", &["application/vnd.ms-excel.addin.macroEnabled.12"]),
    ("xlsb", &["application/vnd.ms-excel.sheet.binary.macroEnabled.12"]),
    ("pptm", &["application/vnd.ms-powerpoint.presentation.macroEnabled.12"]),
    ("potm", &["application/vnd.ms-powerpoint.template.macroEnabled.12"]),
    ("ppam", &["application/vnd.ms-powerpoint.addin.macroEnabled.12"]),
    ("ppsm", &["application/vnd.ms-powerpoint.slideshow.macroEnabled.12"]),
    ("vsdm", &["application/vnd.ms-visio.drawing.macroEnabled.main+xml"]),
    // Pages and drawings that can carry scripts (they ask first).
    ("htm", &["application/xhtml+xml", "text/html"]),
    ("html", &["application/xhtml+xml", "text/html"]),
    ("xhtml", &["application/xhtml+xml"]),
    ("mht", &["application/x-mimearchive"]),
    ("mhtml", &["application/x-mimearchive"]),
    ("svg", &["image/svg+xml"]),
    ("xml", &["application/xml", "text/xml", "application/vnd.cyclonedx+xml"]),
    ("xsl", &["application/xslt+xml"]),
];

/// The kinds of [`KNOWN`] whose name the shared MIME database gives two types or more, which GIO
/// tells apart by what is in the file. Inside a Flatpak sandbox the file is typed again outside it,
/// by the computer's own database, which can tell them apart otherwise: there they are not opened.
pub const SEVERAL: &[&str] = &["png", "json", "ogg", "oga", "opus", "md", "htm", "html", "djvu", "dot", "pot", "pub", "asc", "pem"];

/// The kinds of [`KNOWN`] the shared MIME database has known by name only a few years (since about
/// 2020): a computer with an older one types such a file by what is in it. Inside a Flatpak sandbox,
/// whose database is newer than many computers', they are not opened.
pub const RECENT: &[&str] = &["avif", "heic", "heif", "jxl"];

/// The types the desktop may give a file of kind `ext` ([`KNOWN`]); none for a kind not there.
pub fn desktop_types(ext: &str) -> &'static [&'static str] {
    KNOWN.iter().find(|(kind, _)| *kind == ext).map(|(_, types)| *types).unwrap_or_default()
}

/// What may be done with an attachment of this name when the owner asks to open it, on this
/// computer ([`Chooser::THIS`]); see [`verdict_for`].
pub fn verdict(name: &str) -> Verdict {
    verdict_for(name, Chooser::THIS)
}

/// What may be done with an attachment of this name when the owner asks to open it on a
/// computer that chooses its program as `chooser` says. It is read from the name the file would
/// be written under ([`file_name`]), which is what the computer goes by when it chooses the
/// program.
pub fn verdict_for(name: &str, chooser: Chooser) -> Verdict {
    let safe = file_name(name);
    let ext = extension(&safe);
    if ext.is_empty() {
        // No kind to choose a program by: the computer guesses from what is in it.
        return Verdict::Blocked;
    }
    if BLOCKED.contains(&ext.as_str()) {
        Verdict::Blocked
    } else if chooser == Chooser::NameOrContent && desktop_types(&ext).is_empty() {
        // A kind the desktop may not know: it would guess from what is in it.
        Verdict::Blocked
    } else if !ext.is_ascii() {
        // A kind the lists can't judge (a computer may fold its letters into ASCII ones): ask.
        Verdict::Macros
    } else if MACROS.contains(&ext.as_str()) {
        Verdict::Macros
    } else if SCRIPTS.contains(&ext.as_str()) {
        Verdict::Scripts
    } else if ACTIVE.contains(&ext.as_str()) {
        Verdict::Active
    } else {
        Verdict::Open
    }
}

/// What the owner is told when an attachment isn't opened because of its kind.
pub fn blocked_message(name: &str) -> String {
    let shown: String = file_name(name).chars().take(60).collect();
    format!(
        "MailCraft doesn't open {shown}: a program, a script or a file of no known kind sent in mail can harm the computer. If you trust it, save it with Save As."
    )
}

/// The name an attachment is written under to be opened: only the last part of a path the
/// sender put in its name; no control characters or characters a folder can't have; no leading
/// dots, dashes or spaces (hidden files, options); no trailing dots or spaces (Windows drops
/// them, and with them what the name said it was); no characters that change how it looks
/// without being seen; not a name Windows keeps for a device; at most [`MAX_NAME`] characters
/// with its kind kept. Never empty. Every file named from mail is named through here (see
/// [`save_name`] and [`export_name`]), and so is an attachment where it is shown, so that the
/// name seen is the name written.
pub fn file_name(name: &str) -> String {
    let leaf = name.rsplit(['/', '\\']).next().unwrap_or_default();
    // Explorer reads a comma as the end of an argument and a semicolon is no better: neither
    // stays in a name an opener may be handed. (Names of exported files keep them, see [`export_name`].)
    let out = clean(&leaf.replace([',', ';'], "_"));
    if out.is_empty() { "attachment".to_string() } else { out }
}

/// The name an attachment is saved under (Save As): its [`file_name`], with `.bin` after it when
/// it has no short kind of letters and digits, so that the computer doesn't guess one.
pub fn save_name(name: &str) -> String {
    let safe = file_name(name);
    match safe.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.chars().count() <= 8 && ext.chars().all(char::is_alphanumeric) => safe,
        _ => format!("{safe}.bin"),
    }
}

/// The name a file made from the mailbox is offered under: `stem` (a message's subject, a
/// folder's name, an event's title; at most 80 characters of it) made safe as [`file_name`]
/// makes a name, a slash in it being a character and not a folder, then `.ext`. `untitled` when
/// nothing of `stem` is left ([`export_name_or`] names another).
pub fn export_name(stem: &str, ext: &str) -> String {
    export_name_or(stem, ext, "untitled")
}

/// [`export_name`], named `fallback` when nothing of `stem` is left.
pub fn export_name_or(stem: &str, ext: &str, fallback: &str) -> String {
    let stem: String = stem.chars().take(80).map(|c| if matches!(c, '/' | '\\') { '_' } else { c }).collect();
    let stem = clean(&stem);
    let stem = if stem.is_empty() { fallback } else { stem.as_str() };
    let ext: String = ext.chars().filter(|c| c.is_alphanumeric()).take(16).collect();
    if ext.is_empty() { clean(stem) } else { clean(&format!("{stem}.{ext}")) }
}

/// [`file_name`] of a name with no folders in it, which may come out empty.
fn clean(leaf: &str) -> String {
    let cleaned: String = leaf.chars().map(|c| if c.is_control() || "\\/:*?\"<>|".contains(c) || is_invisible(c) { '_' } else { c }).collect();
    let trimmed = cleaned.trim_start_matches(['.', '-', ' ', '_', '~']).trim_end_matches(['.', ' ']);
    let mut out = shorten(trimmed);
    let stem_lower = out.split('.').next().unwrap_or_default().trim_end().to_lowercase();
    if RESERVED.contains(&stem_lower.as_str()) {
        out = format!("_{out}");
    }
    out
}

/// Characters that change how a name looks without being seen (right-to-left overrides that turn
/// `gpj.exe` into what reads as `exe.jpg`, isolates, zero-width marks and joiners, the byte-order
/// mark, a soft hyphen, the Arabic letter mark, the Mongolian vowel separator, tags).
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{061c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{e0000}'..='\u{e007f}'
    )
}

/// At most [`MAX_NAME`] characters, the kind kept when there is a short one.
fn shorten(name: &str) -> String {
    if name.chars().count() <= MAX_NAME {
        return name.to_string();
    }
    let ext = match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() && ext.chars().count() <= 16 => format!(".{ext}"),
        _ => String::new(),
    };
    let keep = MAX_NAME.saturating_sub(ext.chars().count());
    let stem: String = name.chars().take(keep).collect();
    format!("{}{ext}", stem.trim_end_matches(['.', ' ']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_names_are_made_safe() {
        let cases = [
            ("report.pdf", "report.pdf"),
            ("../../etc/passwd", "passwd"),
            ("..\\..\\Windows\\System32\\evil.pdf", "evil.pdf"),
            ("C:\\Users\\x\\a.pdf", "a.pdf"),
            (".bashrc", "bashrc"),
            ("-rf.txt", "rf.txt"),
            ("--help", "help"),
            ("...", "attachment"),
            ("", "attachment"),
            ("   ", "attachment"),
            ("a\0b\r\n.txt", "a_b__.txt"),
            ("what?.txt", "what_.txt"),
            ("CON.txt", "_CON.txt"),
            ("nul", "_nul"),
            ("Lpt1.tar.gz", "_Lpt1.tar.gz"),
            ("console.txt", "console.txt"),
            ("invoice.exe.", "invoice.exe"),
            ("invoice.exe   ", "invoice.exe"),
            ("photo\u{202e}gpj.exe", "photo_gpj.exe"),
        ];
        for (name, want) in cases {
            assert_eq!(file_name(name), want, "{name:?}");
        }
        let long = format!("{}.pdf", "a".repeat(500));
        let short = file_name(&long);
        assert!(short.chars().count() <= MAX_NAME && short.ends_with(".pdf"), "{short}");
        let wide = "é".repeat(400);
        assert!(file_name(&wide).chars().count() <= MAX_NAME);
        for name in ["", "/", "\\", "././.", "a/b/c/", "\u{feff}", "COM1", "con.", "\u{1F600}.pdf"] {
            let safe = file_name(name);
            assert!(!safe.is_empty() && !safe.contains(['/', '\\']) && !safe.starts_with(['.', '-']), "{name:?} -> {safe:?}");
        }
    }

    #[test]
    fn saved_and_exported_names_are_made_safe() {
        let cases = [
            ("invoice\u{202e}fdp.exe", "invoice_fdp.exe"),
            ("in\u{2067}voice\u{2069}.pdf", "in_voice_.pdf"),
            ("re\u{200b}port.pdf", "re_port.pdf"),
            ("\u{feff}\u{200d}", "attachment.bin"),
            ("CON.txt", "_CON.txt"),
            ("../../x", "x.bin"),
            ("..\\..\\x.pdf", "x.pdf"),
            ("a:b", "a_b.bin"),
            ("a:b.txt", "a_b.txt"),
            ("", "attachment.bin"),
            ("notes", "notes.bin"),
            ("report.pdf", "report.pdf"),
            ("archive.tar.gz", "archive.tar.gz"),
            ("x.p\u{0434}f", "x.p\u{0434}f"),
            ("x.a b", "x.a b.bin"),
        ];
        for (name, want) in cases {
            assert_eq!(save_name(name), want, "{name:?}");
        }
        let cases = [
            ("Q1/Q2 report", "eml", "Q1_Q2 report.eml"),
            ("invoice\u{202e}fdp", "eml", "invoice_fdp.eml"),
            ("CON", "mbox", "_CON.mbox"),
            ("../../x", "ics", "x.ics"),
            ("a:b", "vcf", "a_b.vcf"),
            ("", "eml", "untitled.eml"),
            ("   ", "eml", "untitled.eml"),
            ("\u{200b}\u{202e}", "eml", "untitled.eml"),
            ("Hello.", "eml", "Hello.eml"),
            ("x", "e/m:l", "x.eml"),
        ];
        for (stem, ext, want) in cases {
            assert_eq!(export_name(stem, ext), want, "{stem:?}.{ext}");
        }
        assert_eq!(export_name(&"a".repeat(500), "eml").chars().count(), 84);
    }

    #[test]
    fn commas_and_semicolons_never_reach_the_opener() {
        // AU2-V3: Explorer takes a comma as the end of its argument; the verdict and the file
        // written are of the same name.
        for name in ["a,b.pdf", "x /select,C:\\Windows\\calc.exe.pdf", ",,evil.exe", "a;b.txt"] {
            let safe = file_name(name);
            assert!(!safe.contains([',', ';']), "{safe}");
            assert_eq!(verdict(name), verdict(&safe), "{name}");
        }
        assert_eq!(file_name("a,b;c.pdf"), "a_b_c.pdf");
        assert_eq!(save_name("a,b.pdf"), "a_b.pdf");
    }

    #[test]
    fn programs_are_never_opened() {
        for name in [
            "setup.exe",
            "run.BAT",
            "x.cmd",
            "a.com",
            "s.scr",
            "i.msi",
            "p.ps1",
            "v.vbs",
            "j.js",
            "k.jar",
            "s.sh",
            "Thing.app",
            "short.lnk",
            "disk.iso",
            "invoice.pdf.exe",
            "invoice.exe.",
            "photo\u{202e}gpj.exe",
            "noextension",
            "../x.exe",
            "office.rdp",
            "rates.IQY",
            "sheet.slk",
            "addin.vsto",
            "macro.mam",
            "outlook.prf",
            "help.hpj",
            "group.grp",
            "backup.mailcraft",
            "Backup.MAILCRAFT",
            "look.deskthemepack",
            "x.AFPLOC",
            "x.ftploc",
            "x.vncloc",
            "pack.wcx",
            "x.its",
            "drawing.cdr",
            "x.smi",
            "a,b.exe",
            "a;b.exe",
        ] {
            assert_eq!(verdict(name), Verdict::Blocked, "{name}");
        }
    }

    #[test]
    fn macros_and_pages_ask_and_the_rest_opens() {
        for name in ["budget.xlsm", "Letter.DOCM", "deck.pptm"] {
            assert_eq!(verdict(name), Verdict::Macros, "{name}");
        }
        for name in ["page.html", "x.htm", "logo.svg"] {
            assert_eq!(verdict(name), Verdict::Scripts, "{name}");
        }
        for name in ["report.pdf", "photo.JPG", "notes.txt", "sheet.xlsx", "letter.docx", "archive.zip", "invite.ics"] {
            assert_eq!(verdict(name), Verdict::Open, "{name}");
        }
        assert!(Verdict::Macros.asks() && Verdict::Scripts.asks() && !Verdict::Open.asks() && !Verdict::Blocked.asks());
    }
}
