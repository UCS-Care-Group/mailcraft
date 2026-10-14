//! A small writer of compound files and `.msg` files, for tests (here and in the crates that
//! import them): fixtures are made in code from synthetic content, never taken from anyone's
//! mailbox. Behind the `writer` feature; it is no part of what MailCraft does with a file.

/// A stream or a storage of a compound file.
#[derive(Clone, Debug)]
pub enum Node {
    Stream(String, Vec<u8>),
    Storage(String, Vec<Node>),
}

const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const FREESECT: u32 = 0xFFFF_FFFF;
const FATSECT: u32 = 0xFFFF_FFFD;
const NOSTREAM: u32 = 0xFFFF_FFFF;
const CUTOFF: usize = 4096;

struct Dir {
    name: String,
    kind: u8,
    child: u32,
    right: u32,
    start: u32,
    size: u64,
}

struct Sectors {
    size: usize,
    sectors: Vec<Vec<u8>>,
    fat: Vec<u32>,
}

impl Sectors {
    /// Put `data` in sectors of its own, chained; the first one's number (ENDOFCHAIN for none).
    fn alloc(&mut self, data: &[u8]) -> u32 {
        if data.is_empty() {
            return ENDOFCHAIN;
        }
        let first = self.sectors.len();
        for chunk in data.chunks(self.size) {
            let mut s = chunk.to_vec();
            s.resize(self.size, 0);
            self.sectors.push(s);
            self.fat.push(u32::try_from(self.sectors.len()).unwrap_or(ENDOFCHAIN));
        }
        if let Some(last) = self.fat.last_mut() {
            *last = ENDOFCHAIN;
        }
        u32::try_from(first).unwrap_or(ENDOFCHAIN)
    }
}

/// A compound file holding `children` under its root, in version 3 (512-byte sectors) or 4
/// (4096-byte sectors).
pub fn compound(children: &[Node], version4: bool) -> Vec<u8> {
    let size = if version4 { 4096 } else { 512 };
    let mut dirs = vec![Dir { name: "Root Entry".into(), kind: 5, child: NOSTREAM, right: NOSTREAM, start: ENDOFCHAIN, size: 0 }];
    let mut streams: Vec<(usize, Vec<u8>)> = Vec::new();
    flatten(children, 0, &mut dirs, &mut streams);

    // Small streams in the mini stream, the rest in sectors of their own.
    let mut mini = Vec::new();
    let mut mini_fat: Vec<u32> = Vec::new();
    let mut sectors = Sectors { size, sectors: Vec::new(), fat: Vec::new() };
    for (dir, data) in &streams {
        let start = if data.len() < CUTOFF {
            if data.is_empty() {
                ENDOFCHAIN
            } else {
                let first = mini_fat.len();
                for chunk in data.chunks(64) {
                    let mut c = chunk.to_vec();
                    c.resize(64, 0);
                    mini.extend_from_slice(&c);
                    mini_fat.push(u32::try_from(mini_fat.len() + 1).unwrap_or(ENDOFCHAIN));
                }
                if let Some(last) = mini_fat.last_mut() {
                    *last = ENDOFCHAIN;
                }
                u32::try_from(first).unwrap_or(ENDOFCHAIN)
            }
        } else {
            sectors.alloc(data)
        };
        if let Some(d) = dirs.get_mut(*dir) {
            d.start = start;
            d.size = data.len() as u64;
        }
    }
    let mini_start = sectors.alloc(&mini);
    if let Some(root) = dirs.first_mut() {
        root.start = mini_start;
        root.size = mini.len() as u64;
    }
    let mini_fat_bytes: Vec<u8> = mini_fat.iter().flat_map(|n| n.to_le_bytes()).collect();
    let mini_fat_count = mini_fat_bytes.len().div_ceil(size);
    let mini_fat_start = sectors.alloc(&mini_fat_bytes);
    let mut dir_bytes = Vec::new();
    for d in &dirs {
        let mut e = [0u8; 128];
        let units: Vec<u16> = d.name.encode_utf16().take(31).collect();
        for (i, u) in units.iter().enumerate() {
            e[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        e[64..66].copy_from_slice(&(((units.len() + 1) * 2) as u16).to_le_bytes());
        e[66] = d.kind;
        e[67] = 1;
        e[68..72].copy_from_slice(&NOSTREAM.to_le_bytes());
        e[72..76].copy_from_slice(&d.right.to_le_bytes());
        e[76..80].copy_from_slice(&d.child.to_le_bytes());
        e[116..120].copy_from_slice(&d.start.to_le_bytes());
        e[120..128].copy_from_slice(&d.size.to_le_bytes());
        dir_bytes.extend_from_slice(&e);
    }
    let dir_start = sectors.alloc(&dir_bytes);
    let dir_count = dir_bytes.len().div_ceil(size);

    // The FAT itself, at the end: enough sectors to describe every sector, its own included.
    let per = size / 4;
    let mut fat_count = 1;
    while (sectors.sectors.len() + fat_count).div_ceil(per) > fat_count {
        fat_count += 1;
    }
    let first_fat = sectors.sectors.len();
    let mut fat = sectors.fat.clone();
    fat.extend(std::iter::repeat_n(FATSECT, fat_count));
    fat.resize(fat_count * per, FREESECT);
    let fat_bytes: Vec<u8> = fat.iter().flat_map(|n| n.to_le_bytes()).collect();

    let mut header = vec![0u8; size];
    header[..8].copy_from_slice(&crate::SIGNATURE);
    header[24..26].copy_from_slice(&0x003Eu16.to_le_bytes());
    header[26..28].copy_from_slice(&(if version4 { 4u16 } else { 3 }).to_le_bytes());
    header[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes());
    header[30..32].copy_from_slice(&(if version4 { 12u16 } else { 9 }).to_le_bytes());
    header[32..34].copy_from_slice(&6u16.to_le_bytes());
    header[40..44].copy_from_slice(&(if version4 { dir_count as u32 } else { 0 }).to_le_bytes());
    header[44..48].copy_from_slice(&(fat_count as u32).to_le_bytes());
    header[48..52].copy_from_slice(&dir_start.to_le_bytes());
    header[56..60].copy_from_slice(&(CUTOFF as u32).to_le_bytes());
    header[60..64].copy_from_slice(&mini_fat_start.to_le_bytes());
    header[64..68].copy_from_slice(&(mini_fat_count as u32).to_le_bytes());
    header[68..72].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
    for i in 0..109 {
        let v = if i < fat_count { (first_fat + i) as u32 } else { FREESECT };
        header[76 + i * 4..80 + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    let mut out = header;
    for s in &sectors.sectors {
        out.extend_from_slice(s);
    }
    out.extend_from_slice(&fat_bytes);
    out
}

fn flatten(children: &[Node], parent: usize, dirs: &mut Vec<Dir>, streams: &mut Vec<(usize, Vec<u8>)>) {
    let mut previous: Option<usize> = None;
    for node in children {
        let id = dirs.len();
        let (name, kind) = match node {
            Node::Stream(name, _) => (name.clone(), 2),
            Node::Storage(name, _) => (name.clone(), 1),
        };
        dirs.push(Dir { name, kind, child: NOSTREAM, right: NOSTREAM, start: 0, size: 0 });
        // Siblings in a line to the right: a valid (if unbalanced) tree.
        match previous {
            None => {
                if let Some(p) = dirs.get_mut(parent) {
                    p.child = id as u32;
                }
            }
            Some(prev) => {
                if let Some(p) = dirs.get_mut(prev) {
                    p.right = id as u32;
                }
            }
        }
        previous = Some(id);
        match node {
            Node::Stream(_, data) => streams.push((id, data.clone())),
            Node::Storage(_, inner) => flatten(inner, id, dirs, streams),
        }
    }
}

/// The properties of a message, recipient or attachment object, and the storages under it.
#[derive(Clone, Debug, Default)]
pub struct Props {
    fixed: Vec<(u32, [u8; 8])>,
    streams: Vec<(String, Vec<u8>)>,
    children: Vec<Node>,
}

impl Props {
    pub fn new() -> Props {
        Props::default()
    }

    fn variable(mut self, id: u16, ty: u16, data: Vec<u8>) -> Props {
        let mut v = [0u8; 8];
        v[..4].copy_from_slice(&(data.len() as u32).to_le_bytes());
        self.fixed.push(((u32::from(id) << 16) | u32::from(ty), v));
        self.streams.push((format!("__substg1.0_{id:04X}{ty:04X}"), data));
        self
    }

    /// A Unicode string property.
    pub fn unicode(self, id: u16, s: &str) -> Props {
        let mut data: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        data.extend_from_slice(&[0, 0]);
        self.variable(id, 0x001F, data)
    }

    /// An 8-bit string property, in the bytes given.
    pub fn string8(self, id: u16, bytes: &[u8]) -> Props {
        let mut data = bytes.to_vec();
        data.push(0);
        self.variable(id, 0x001E, data)
    }

    pub fn binary(self, id: u16, data: &[u8]) -> Props {
        self.variable(id, 0x0102, data.to_vec())
    }

    fn fixed_value(mut self, id: u16, ty: u16, v: [u8; 8]) -> Props {
        self.fixed.push(((u32::from(id) << 16) | u32::from(ty), v));
        self
    }

    pub fn int(self, id: u16, n: i32) -> Props {
        let mut v = [0u8; 8];
        v[..4].copy_from_slice(&n.to_le_bytes());
        self.fixed_value(id, 0x0003, v)
    }

    pub fn bool(self, id: u16, b: bool) -> Props {
        let mut v = [0u8; 8];
        v[0] = u8::from(b);
        self.fixed_value(id, 0x000B, v)
    }

    /// A time property, from seconds since 1970.
    pub fn time(self, id: u16, unix: i64) -> Props {
        let ft = (unix + 11_644_473_600) * 10_000_000;
        self.fixed_value(id, 0x0040, ft.to_le_bytes())
    }

    /// A recipient (1 To, 2 Cc, 3 Bcc).
    pub fn recipient(mut self, r: Props) -> Props {
        let n = self.children.iter().filter(|c| matches!(c, Node::Storage(name, _) if name.starts_with("__recip"))).count();
        self.children.push(r.storage(&format!("__recip_version1.0_#{n:08X}"), 8));
        self
    }

    pub fn attachment(mut self, a: Props) -> Props {
        let n = self.children.iter().filter(|c| matches!(c, Node::Storage(name, _) if name.starts_with("__attach"))).count();
        self.children.push(a.storage(&format!("__attach_version1.0_#{n:08X}"), 8));
        self
    }

    /// An attachment holding a message (attach method 5).
    pub fn embedded(self, message: Props) -> Props {
        let mut a = self.int(0x3705, 5);
        let mut v = [0u8; 8];
        v[..4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        a.fixed.push(((0x3701u32 << 16) | 0x000D, v));
        a.children.push(message.storage("__substg1.0_3701000D", 24));
        a
    }

    /// The object as a storage named `name` with a property stream header of `header` bytes.
    pub fn storage(self, name: &str, header: usize) -> Node {
        Node::Storage(name.to_string(), self.nodes(header))
    }

    fn nodes(self, header: usize) -> Vec<Node> {
        let mut props = vec![0u8; header];
        for (tag, value) in &self.fixed {
            props.extend_from_slice(&tag.to_le_bytes());
            props.extend_from_slice(&6u32.to_le_bytes());
            props.extend_from_slice(value);
        }
        let mut nodes = vec![Node::Stream("__properties_version1.0".into(), props)];
        nodes.extend(self.streams.into_iter().map(|(name, data)| Node::Stream(name, data)));
        nodes.extend(self.children);
        nodes
    }

    /// The whole `.msg` file, with this as its message.
    pub fn msg(self) -> Vec<u8> {
        compound(&self.nodes(32), false)
    }

    /// The same in a version 4 compound file.
    pub fn msg_v4(self) -> Vec<u8> {
        compound(&self.nodes(32), true)
    }
}

/// RTF as compressed RTF ([MS-OXRTFCP]) made of literals only (valid, if no smaller).
pub fn compress_rtf(rtf: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    for chunk in rtf.chunks(8) {
        body.push(0);
        body.extend_from_slice(chunk);
    }
    // The end: a reference to where the dictionary is being written.
    let w = (207 + rtf.len()) % 4096;
    let end = ((w as u16) << 4).to_be_bytes();
    if rtf.len().is_multiple_of(8) {
        body.push(0x01);
    } else if let Some(control) = body.get_mut((rtf.len() / 8) * 9) {
        *control |= 1 << (rtf.len() % 8);
    }
    body.extend_from_slice(&end);
    let mut out = Vec::new();
    out.extend_from_slice(&((body.len() + 12) as u32).to_le_bytes());
    out.extend_from_slice(&(rtf.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x7546_5A4Cu32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&body);
    out
}
