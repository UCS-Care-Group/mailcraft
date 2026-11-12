// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Synthetic TNEF streams, written as [MS-OXTNEF] lays them out, from invented files and text:
//! never a real `winmail.dat`.

/// Attribute levels.
pub const MESSAGE: u8 = 1;
pub const ATTACHMENT: u8 = 2;

/// Attribute ids with their types, as the stream writes them (2.1.3.2).
pub const TNEF_VERSION: u32 = 0x0008_9006;
pub const OEM_CODEPAGE: u32 = 0x0006_9007;
pub const MESSAGE_CLASS: u32 = 0x0007_8008;
pub const SUBJECT: u32 = 0x0001_8004;
pub const BODY: u32 = 0x0002_800C;
pub const MSG_PROPS: u32 = 0x0006_9003;
pub const ATTACH_REND_DATA: u32 = 0x0006_9002;
pub const ATTACH_DATA: u32 = 0x0006_800F;
pub const ATTACH_TITLE: u32 = 0x0001_8010;
pub const ATTACHMENT_PROPS: u32 = 0x0006_9005;

/// The interface identifier of a Message object, for an attached message.
pub const IID_MESSAGE: [u8; 16] = [0x07, 0x03, 0x02, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46];

/// A property to encode (2.1.3.4).
#[derive(Debug, Clone)]
pub enum Prop {
    Int32(u16, i32),
    Bool(u16, bool),
    Ansi(u16, Vec<u8>),
    Unicode(u16, String),
    Binary(u16, Vec<u8>),
    Object(u16, [u8; 16], Vec<u8>),
    /// A multi-valued 32-bit integer.
    MvInt32(u16, Vec<i32>),
    /// A multi-valued Unicode string.
    MvUnicode(u16, Vec<String>),
    /// A named property with a string name, holding a 32-bit integer.
    NamedInt32 {
        name: String,
        value: i32,
    },
    /// A type word and id followed by raw bytes, for types a reader can't size.
    Raw(u16, u16, Vec<u8>),
}

fn pad(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

fn variable(out: &mut Vec<u8>, value: &[u8]) {
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
    out.extend_from_slice(value);
    pad(out);
}

/// A counted list of properties (`MsgPropertyList`).
pub fn props(list: &[Prop]) -> Vec<u8> {
    let mut out = (list.len() as u32).to_le_bytes().to_vec();
    for p in list {
        let tag = |out: &mut Vec<u8>, ty: u16, id: u16| {
            out.extend_from_slice(&ty.to_le_bytes());
            out.extend_from_slice(&id.to_le_bytes());
        };
        match p {
            Prop::Int32(id, v) => {
                tag(&mut out, 0x0003, *id);
                out.extend_from_slice(&v.to_le_bytes());
            }
            Prop::Bool(id, v) => {
                tag(&mut out, 0x000B, *id);
                out.extend_from_slice(&u16::from(*v).to_le_bytes());
                pad(&mut out);
            }
            Prop::Ansi(id, v) => {
                tag(&mut out, 0x001E, *id);
                let mut z = v.clone();
                z.push(0);
                variable(&mut out, &z);
            }
            Prop::Unicode(id, v) => {
                tag(&mut out, 0x001F, *id);
                variable(&mut out, &utf16z(v));
            }
            Prop::Binary(id, v) => {
                tag(&mut out, 0x0102, *id);
                variable(&mut out, v);
            }
            Prop::Object(id, iid, v) => {
                tag(&mut out, 0x000D, *id);
                let mut data = iid.to_vec();
                data.extend_from_slice(v);
                variable(&mut out, &data);
            }
            Prop::MvInt32(id, values) => {
                tag(&mut out, 0x1003, *id);
                out.extend_from_slice(&(values.len() as u32).to_le_bytes());
                for v in values {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
            Prop::MvUnicode(id, values) => {
                tag(&mut out, 0x101F, *id);
                out.extend_from_slice(&(values.len() as u32).to_le_bytes());
                for v in values {
                    let z = utf16z(v);
                    out.extend_from_slice(&(z.len() as u32).to_le_bytes());
                    out.extend_from_slice(&z);
                    pad(&mut out);
                }
            }
            Prop::NamedInt32 { name, value } => {
                tag(&mut out, 0x0003, 0x8001);
                out.extend_from_slice(&[0x29, 0x03, 0x02, 0x00, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
                out.extend_from_slice(&1u32.to_le_bytes());
                let z = utf16z(name);
                out.extend_from_slice(&(z.len() as u32).to_le_bytes());
                out.extend_from_slice(&z);
                pad(&mut out);
                out.extend_from_slice(&value.to_le_bytes());
            }
            Prop::Raw(ty, id, bytes) => {
                tag(&mut out, *ty, *id);
                out.extend_from_slice(bytes);
            }
        }
    }
    out
}

/// A stream being written.
pub struct Writer {
    out: Vec<u8>,
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

impl Writer {
    /// The signature, a legacy key, the version and Windows-1252 as the code page.
    pub fn new() -> Writer {
        Writer::with_codepage(1252)
    }

    pub fn with_codepage(codepage: u32) -> Writer {
        let mut w = Writer { out: Vec::new() };
        w.out.extend_from_slice(&crate::SIGNATURE);
        w.out.extend_from_slice(&0x1234u16.to_le_bytes());
        w.attribute(MESSAGE, TNEF_VERSION, &[0, 0, 1, 0]);
        let mut cp = codepage.to_le_bytes().to_vec();
        cp.extend_from_slice(&[0; 4]);
        w.attribute(MESSAGE, OEM_CODEPAGE, &cp);
        w
    }

    /// An attribute with its right checksum.
    pub fn attribute(&mut self, level: u8, id: u32, data: &[u8]) -> &mut Writer {
        let sum = data.iter().fold(0u16, |s, b| s.wrapping_add(u16::from(*b)));
        self.attribute_with_checksum(level, id, data, sum)
    }

    pub fn attribute_with_checksum(&mut self, level: u8, id: u32, data: &[u8], checksum: u16) -> &mut Writer {
        self.out.push(level);
        self.out.extend_from_slice(&id.to_le_bytes());
        self.out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.out.extend_from_slice(data);
        self.out.extend_from_slice(&checksum.to_le_bytes());
        self
    }

    /// `attAttachRendData` for a file (1) or an OLE object (2).
    pub fn rend_data(&mut self, kind: u16) -> &mut Writer {
        let mut data = kind.to_le_bytes().to_vec();
        data.extend_from_slice(&u32::MAX.to_le_bytes());
        data.extend_from_slice(&32u16.to_le_bytes());
        data.extend_from_slice(&32u16.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        self.attribute(ATTACHMENT, ATTACH_REND_DATA, &data)
    }

    /// A file as the legacy attributes give it: its title (8-bit, with its NUL) and its data.
    pub fn legacy_file(&mut self, title: &[u8], data: &[u8]) -> &mut Writer {
        self.rend_data(1);
        let mut t = title.to_vec();
        t.push(0);
        self.attribute(ATTACHMENT, ATTACH_TITLE, &t);
        self.attribute(ATTACHMENT, ATTACH_DATA, data)
    }

    /// A file as Outlook writes one today: the legacy attributes, then its properties with the
    /// long name, media type and data.
    pub fn file(&mut self, name: &str, mime: &str, data: &[u8]) -> &mut Writer {
        self.legacy_file(b"SHORT~1.DAT", data);
        let list = vec![
            Prop::Int32(0x3705, 1),
            Prop::Unicode(0x3707, name.to_string()),
            Prop::Unicode(0x370E, mime.to_string()),
            Prop::Binary(0x3701, data.to_vec()),
        ];
        self.attribute(ATTACHMENT, ATTACHMENT_PROPS, &props(&list))
    }

    /// An attachment's property list alone, after its `attAttachRendData`.
    pub fn attachment(&mut self, kind: u16, list: &[Prop]) -> &mut Writer {
        self.rend_data(kind);
        self.attribute(ATTACHMENT, ATTACHMENT_PROPS, &props(list))
    }

    pub fn message_props(&mut self, list: &[Prop]) -> &mut Writer {
        self.attribute(MESSAGE, MSG_PROPS, &props(list))
    }

    pub fn raw(&mut self, bytes: &[u8]) -> &mut Writer {
        self.out.extend_from_slice(bytes);
        self
    }

    pub fn finish(&self) -> Vec<u8> {
        self.out.clone()
    }
}

/// Compressed rich text stored as it is (`MELA`, [MS-OXRTFCP] 2.1.3.1.1), for PidTagRtfCompressed.
pub fn uncompressed_rtf(rtf: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&((rtf.len() + 12) as u32).to_le_bytes());
    out.extend_from_slice(&(rtf.len() as u32).to_le_bytes());
    out.extend_from_slice(b"MELA");
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(rtf);
    out
}
