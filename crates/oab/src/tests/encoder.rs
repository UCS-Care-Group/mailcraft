// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An LZXD encoder for the tests, written from [MS-PATCH] like the decoder but from the other
//! side (2.6's encoding steps): greedy matches over the whole history, blocks of the kinds and
//! sizes asked for, trees sent as differences through a pretree with or without its run codes,
//! repeated offsets used or not, E8 translation, chunks of 32 KB with their sizes.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use crate::lzxd::{CHUNK, base_position, footer_bits, position_slots};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Verbatim,
    Aligned,
    Uncompressed,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub window_bits: u32,
    /// E8 translation with this file size.
    pub e8: Option<u32>,
    /// The blocks' kinds and sizes, the last repeated until the data ends.
    pub blocks: Vec<(Kind, usize)>,
    /// Whether trees use the pretree's run codes (17, 18, 19).
    pub runs: bool,
    /// Whether matches use the repeated offsets.
    pub repeats: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { window_bits: 17, e8: None, blocks: vec![(Kind::Verbatim, 1 << 20)], runs: true, repeats: true }
    }
}

/// Bits, most significant first, into little-endian 16-bit words.
#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    acc: u32,
    n: u32,
}

impl BitWriter {
    fn put(&mut self, value: u32, bits: u32) {
        for i in (0..bits).rev() {
            self.acc = (self.acc << 1) | ((value >> i) & 1);
            self.n += 1;
            if self.n == 16 {
                self.bytes.extend_from_slice(&(self.acc as u16).to_le_bytes());
                self.acc = 0;
                self.n = 0;
            }
        }
    }

    fn align(&mut self) {
        if self.n > 0 {
            self.put(0, 16 - self.n);
        }
    }

    fn raw(&mut self, bytes: &[u8]) {
        assert_eq!(self.n, 0);
        self.bytes.extend_from_slice(bytes);
    }
}

/// The stream so far: finished chunks, and the one being written.
#[derive(Default)]
struct Out {
    done: Vec<u8>,
    cur: BitWriter,
}

impl Out {
    fn flush_chunk(&mut self) {
        self.cur.align();
        let bytes = std::mem::take(&mut self.cur.bytes);
        self.done.extend_from_slice(&u16::try_from(bytes.len()).unwrap().to_le_bytes());
        self.done.extend_from_slice(&bytes);
    }
}

/// Path lengths for `freqs` no longer than `limit`, at least two when any is used.
pub fn lengths(freqs: &[u32], limit: u8) -> Vec<u8> {
    let mut f: Vec<u64> = freqs.iter().map(|&x| u64::from(x)).collect();
    loop {
        let used: Vec<usize> = (0..f.len()).filter(|&i| f[i] > 0).collect();
        let mut lens = vec![0u8; f.len()];
        match used.len() {
            0 => return lens,
            1 => {
                lens[used[0]] = 1;
                lens[if used[0] == 0 { 1 } else { 0 }] = 1;
                return lens;
            }
            _ => {}
        }
        let mut parent = vec![usize::MAX; f.len() * 2];
        let mut heap: BinaryHeap<Reverse<(u64, usize)>> = used.iter().map(|&i| Reverse((f[i], i))).collect();
        let mut next = f.len();
        while heap.len() > 1 {
            let Reverse((wa, a)) = heap.pop().unwrap();
            let Reverse((wb, b)) = heap.pop().unwrap();
            parent[a] = next;
            parent[b] = next;
            heap.push(Reverse((wa + wb, next)));
            next += 1;
        }
        let mut longest = 0;
        for &i in &used {
            let (mut depth, mut at) = (0u8, i);
            while parent[at] != usize::MAX {
                at = parent[at];
                depth += 1;
            }
            lens[i] = depth;
            longest = longest.max(depth);
        }
        if longest <= limit {
            return lens;
        }
        f = f.iter().map(|&x| if x > 0 { x.div_ceil(2) } else { 0 }).collect();
    }
}

/// The canonical codes of `lens` (2.4).
pub fn codes(lens: &[u8]) -> Vec<u32> {
    let mut count = [0u32; 17];
    for &l in lens {
        count[usize::from(l)] += 1;
    }
    count[0] = 0;
    let mut next = [0u32; 17];
    let mut code = 0;
    for bits in 1..17 {
        code = (code + count[bits - 1]) << 1;
        next[bits] = code;
    }
    lens.iter()
        .map(|&l| {
            if l == 0 {
                return 0;
            }
            let c = next[usize::from(l)];
            next[usize::from(l)] += 1;
            c
        })
        .collect()
}

struct Coded {
    lens: Vec<u8>,
    codes: Vec<u32>,
}

impl Coded {
    fn new(freqs: &[u32], limit: u8) -> Coded {
        let lens = lengths(freqs, limit);
        let codes = codes(&lens);
        Coded { lens, codes }
    }

    fn put(&self, w: &mut BitWriter, symbol: usize) {
        assert!(self.lens[symbol] > 0, "symbol {symbol} has no code");
        w.put(self.codes[symbol], u32::from(self.lens[symbol]));
    }
}

/// Sends `cur` as the difference from `prev` through a pretree (2.5).
fn write_lengths(w: &mut BitWriter, prev: &[u8], cur: &[u8], runs: bool) {
    let delta = |i: usize| ((17 + u32::from(prev[i]) - u32::from(cur[i])) % 17) as usize;
    // (pretree code, extra bits value, extra bits, and for 19 the code that follows)
    let mut codes: Vec<(usize, u32, u32, Option<usize>)> = Vec::new();
    let mut i = 0;
    while i < cur.len() {
        let same = cur[i..].iter().take_while(|&&l| l == cur[i]).count();
        if runs && cur[i] == 0 && same >= 20 {
            let run = same.min(51);
            codes.push((18, (run - 20) as u32, 5, None));
            i += run;
        } else if runs && cur[i] == 0 && same >= 4 {
            let run = same.min(19);
            codes.push((17, (run - 4) as u32, 4, None));
            i += run;
        } else if runs && same >= 4 {
            let run = same.min(5);
            codes.push((19, (run - 4) as u32, 1, Some(delta(i))));
            i += run;
        } else {
            codes.push((delta(i), 0, 0, None));
            i += 1;
        }
    }
    let mut freqs = [0u32; 20];
    for &(code, _, _, then) in &codes {
        freqs[code] += 1;
        if let Some(t) = then {
            freqs[t] += 1;
        }
    }
    let pretree = Coded::new(&freqs, 15);
    for &l in &pretree.lens {
        w.put(u32::from(l), 4);
    }
    for (code, extra, bits, then) in codes {
        pretree.put(w, code);
        w.put(extra, bits);
        if let Some(t) = then {
            pretree.put(w, t);
        }
    }
}

/// One literal or match, as it will be written.
enum Token {
    Literal(u8),
    Match { main: usize, length_footer: Option<usize>, verbatim: (u32, u32), aligned: Option<usize>, extra: Option<usize>, len: usize },
}

impl Token {
    fn len(&self) -> usize {
        match self {
            Token::Literal(_) => 1,
            Token::Match { len, .. } => *len,
        }
    }
}

struct Matcher {
    chains: HashMap<[u8; 3], Vec<usize>>,
    r: [usize; 3],
}

impl Matcher {
    fn insert(&mut self, all: &[u8], p: usize) {
        if let Some(key) = all.get(p..p + 3) {
            let chain = self.chains.entry([key[0], key[1], key[2]]).or_default();
            chain.push(p);
            if chain.len() > 16 {
                chain.remove(0);
            }
        }
    }
}

fn match_len(all: &[u8], p: usize, offset: usize, max: usize) -> usize {
    if offset == 0 || offset > p {
        return 0;
    }
    (0..max).take_while(|&i| all[p + i] == all[p + i - offset]).count()
}

/// The tokens of `all[from..to]`, `base` bytes of `all` being reference data.
#[allow(clippy::too_many_arguments)]
fn tokenize(all: &[u8], base: usize, from: usize, to: usize, kind: Kind, slots: usize, window: usize, repeats: bool, m: &mut Matcher) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut p = from;
    while p < to {
        let out = p - base;
        let max = (to - p).min(CHUNK - out % CHUNK).min(257 + 32767);
        let mut best = (0usize, 0usize);
        if repeats {
            for &r in &m.r {
                let len = match_len(all, p, r, max);
                if len > best.0 {
                    best = (len, r);
                }
            }
        }
        if let Some(key) = all.get(p..p + 3)
            && let Some(chain) = m.chains.get(&[key[0], key[1], key[2]])
        {
            for &q in chain.iter().rev() {
                let offset = p - q;
                if offset + 3 > window {
                    continue;
                }
                let len = match_len(all, p, offset, max);
                if len > best.0 + 1 {
                    best = (len, offset);
                }
            }
        }
        let (len, offset) = best;
        let repeat = repeats && m.r.contains(&offset);
        if len >= 3 || (len == 2 && repeat) {
            let formatted = match m.r.iter().position(|&r| r == offset) {
                Some(slot) if repeats => slot,
                _ => offset + 2,
            };
            match formatted {
                0 => {}
                1 => m.r.swap(0, 1),
                2 => m.r.swap(0, 2),
                _ => m.r = [offset, m.r[0], m.r[1]],
            }
            let slot = (0..slots).rev().find(|&s| base_position(s).unwrap() as usize <= formatted).unwrap();
            let footer = formatted - base_position(slot).unwrap() as usize;
            let bits = footer_bits(slot);
            let (verbatim, aligned) = if formatted < 3 {
                ((0, 0), None)
            } else if kind == Kind::Aligned && bits >= 3 {
                (((footer >> 3) as u32, bits - 3), Some(footer & 7))
            } else {
                ((footer as u32, bits), None)
            };
            let length_header = if len <= 8 { len - 2 } else { 7 };
            let length_footer = (len > 8).then(|| len.min(257) - 9);
            let extra = (len >= 257).then(|| len - 257);
            tokens.push(Token::Match { main: 256 + (slot << 3) + length_header, length_footer, verbatim, aligned, extra, len });
            for i in 0..len {
                m.insert(all, p + i);
            }
            p += len;
        } else {
            tokens.push(Token::Literal(all[p]));
            m.insert(all, p);
            p += 1;
        }
    }
    tokens
}

/// Applies E8 translation (2.2.2) before compressing.
fn e8(data: &mut [u8], size: u32) {
    let size = i64::from(size);
    for (index, chunk) in data.chunks_mut(CHUNK).enumerate() {
        let chunk_offset = (index * CHUNK) as i64;
        if chunk_offset >= 0x4000_0000 || chunk.len() <= 10 {
            continue;
        }
        let mut i = 0;
        while i < chunk.len() - 10 {
            if chunk[i] == 0xE8 {
                let current = chunk_offset + i as i64;
                let displacement = i64::from(i32::from_le_bytes(chunk[i + 1..i + 5].try_into().unwrap()));
                let mut target = current + displacement;
                if target >= 0 && target < size + current {
                    if target >= size {
                        target = displacement - size;
                    }
                    chunk[i + 1..i + 5].copy_from_slice(&(target as i32).to_le_bytes());
                }
                i += 4;
            }
            i += 1;
        }
    }
}

/// `data` packed as an LZXD stream.
pub fn encode(data: &[u8], opts: &Options) -> Vec<u8> {
    encode_with_reference(&[], data, opts)
}

/// `data` packed as an LZXD stream that may match into `reference`.
pub fn encode_with_reference(reference: &[u8], data: &[u8], opts: &Options) -> Vec<u8> {
    let slots = position_slots(opts.window_bits).unwrap();
    let window = 1usize << opts.window_bits;
    let mut data = data.to_vec();
    if let Some(size) = opts.e8 {
        e8(&mut data, size);
    }
    let base = reference.len();
    let all: Vec<u8> = reference.iter().chain(data.iter()).copied().collect();
    let mut m = Matcher { chains: HashMap::new(), r: [1, 1, 1] };
    for p in 0..base {
        m.insert(&all, p);
    }
    let mut out = Out::default();
    match opts.e8 {
        Some(size) => {
            out.cur.put(1, 1);
            out.cur.put(size >> 16, 16);
            out.cur.put(size & 0xFFFF, 16);
        }
        None => out.cur.put(0, 1),
    }
    let mut main_lens = vec![0u8; 256 + 8 * slots];
    let mut length_lens = vec![0u8; 249];
    let mut p = base;
    let mut plan = opts.blocks.iter().copied();
    let mut current = plan.next().unwrap();
    while p < all.len() {
        let (kind, size) = current;
        if let Some(next) = plan.next() {
            current = next;
        }
        let end = (p + size.clamp(1, (1 << 24) - 1)).min(all.len());
        out.cur.put(
            match kind {
                Kind::Verbatim => 1,
                Kind::Aligned => 2,
                Kind::Uncompressed => 3,
            },
            3,
        );
        let size = end - p;
        out.cur.put((size >> 16) as u32 & 0xFF, 8);
        out.cur.put((size >> 8) as u32 & 0xFF, 8);
        out.cur.put(size as u32 & 0xFF, 8);
        if kind == Kind::Uncompressed {
            if out.cur.n == 0 {
                out.cur.put(0, 16);
            } else {
                out.cur.align();
            }
            for r in m.r {
                out.cur.raw(&(r as u32).to_le_bytes());
            }
            while p < end {
                let at = p - base;
                let n = (end - p).min(CHUNK - at % CHUNK);
                out.cur.raw(&all[p..p + n]);
                for i in 0..n {
                    m.insert(&all, p + i);
                }
                p += n;
                if p == end && size % 2 == 1 {
                    out.cur.raw(&[0]);
                }
                if (p - base).is_multiple_of(CHUNK) && p < all.len() {
                    out.flush_chunk();
                }
            }
            continue;
        }
        let tokens = tokenize(&all, base, p, end, kind, slots, window, opts.repeats, &mut m);
        let mut main_freq = vec![0u32; main_lens.len()];
        let mut length_freq = vec![0u32; 249];
        let mut aligned_freq = [0u32; 8];
        for t in &tokens {
            match t {
                Token::Literal(b) => main_freq[usize::from(*b)] += 1,
                Token::Match { main, length_footer, aligned, .. } => {
                    main_freq[*main] += 1;
                    if let Some(f) = length_footer {
                        length_freq[*f] += 1;
                    }
                    if let Some(a) = aligned {
                        aligned_freq[*a] += 1;
                    }
                }
            }
        }
        let main = Coded::new(&main_freq, 16);
        let length = Coded::new(&length_freq, 16);
        let aligned = Coded::new(&aligned_freq, 7);
        if kind == Kind::Aligned {
            for &l in &aligned.lens {
                out.cur.put(u32::from(l), 3);
            }
        }
        write_lengths(&mut out.cur, &main_lens[..256], &main.lens[..256], opts.runs);
        write_lengths(&mut out.cur, &main_lens[256..], &main.lens[256..], opts.runs);
        write_lengths(&mut out.cur, &length_lens, &length.lens, opts.runs);
        main_lens.clone_from(&main.lens);
        length_lens.clone_from(&length.lens);
        for t in &tokens {
            match t {
                Token::Literal(b) => main.put(&mut out.cur, usize::from(*b)),
                Token::Match { main: element, length_footer, verbatim, aligned: a, extra, .. } => {
                    main.put(&mut out.cur, *element);
                    if let Some(f) = length_footer {
                        length.put(&mut out.cur, *f);
                    }
                    out.cur.put(verbatim.0, verbatim.1);
                    if let Some(a) = a {
                        aligned.put(&mut out.cur, *a);
                    }
                    if let Some(extra) = extra {
                        let extra = *extra as u32;
                        if extra < 256 {
                            out.cur.put(0, 1);
                            out.cur.put(extra, 8);
                        } else if extra < 256 + 1024 {
                            out.cur.put(0b10, 2);
                            out.cur.put(extra - 256, 10);
                        } else if extra < 256 + 1024 + 4096 {
                            out.cur.put(0b110, 3);
                            out.cur.put(extra - 256 - 1024, 12);
                        } else {
                            out.cur.put(0b111, 3);
                            out.cur.put(extra, 15);
                        }
                    }
                }
            }
            p += t.len();
            if (p - base).is_multiple_of(CHUNK) && p < all.len() {
                out.flush_chunk();
            }
        }
    }
    out.flush_chunk();
    out.done
}
