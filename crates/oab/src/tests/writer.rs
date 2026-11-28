// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Writes version 4 details files and their compressed form, stored ([MS-OXOAB] 2.9, 2.11), for
//! the tests: this crate's, and through the `test-writer` feature other crates'.

use crate::{PropDef, Record, Value, crc32};

fn integer(out: &mut Vec<u8>, v: u32) {
    if v <= 0x7F {
        out.push(v as u8);
        return;
    }
    let bytes = v.to_le_bytes();
    let n = 4 - bytes.iter().rev().take_while(|&&b| b == 0).count();
    out.push(0x80 | n as u8);
    out.extend_from_slice(&bytes[..n]);
}

fn text(out: &mut Vec<u8>, t: &str) {
    out.extend_from_slice(t.as_bytes());
    out.push(0);
}

fn binary(out: &mut Vec<u8>, b: &[u8]) {
    integer(out, b.len() as u32);
    out.extend_from_slice(b);
}

fn value(out: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Integer(n) => integer(out, *n),
        Value::Boolean(b) => out.push(u8::from(*b)),
        Value::Text(t) => text(out, t),
        Value::Binary(b) => binary(out, b),
        Value::Integers(ns) => {
            integer(out, ns.len() as u32);
            ns.iter().for_each(|&n| integer(out, n));
        }
        Value::Texts(ts) => {
            integer(out, ts.len() as u32);
            ts.iter().for_each(|t| text(out, t));
        }
        Value::Binaries(bs) => {
            integer(out, bs.len() as u32);
            bs.iter().for_each(|b| binary(out, b));
        }
    }
}

fn table(out: &mut Vec<u8>, schema: &[PropDef]) {
    out.extend_from_slice(&(schema.len() as u32).to_le_bytes());
    for p in schema {
        out.extend_from_slice(&p.tag.to_le_bytes());
        out.extend_from_slice(&(u32::from(p.anr) | (u32::from(p.primary_key) << 1)).to_le_bytes());
    }
}

fn record(out: &mut Vec<u8>, schema: &[PropDef], r: &Record) {
    let mut presence = vec![0u8; schema.len().div_ceil(8)];
    let mut data = Vec::new();
    for (i, p) in schema.iter().enumerate() {
        if let Some((_, v)) = r.values.iter().find(|(tag, _)| *tag == p.tag) {
            presence[i / 8] |= 0x80 >> (i % 8);
            value(&mut data, v);
        }
    }
    out.extend_from_slice(&((4 + presence.len() + data.len()) as u32).to_le_bytes());
    out.extend_from_slice(&presence);
    out.extend_from_slice(&data);
}

/// An unpacked version 4 Full Details file.
pub fn details(header_schema: &[PropDef], schema: &[PropDef], header: &Record, records: &[Record]) -> Vec<u8> {
    let mut rest = Vec::new();
    let mut meta = Vec::new();
    table(&mut meta, header_schema);
    table(&mut meta, schema);
    rest.extend_from_slice(&((meta.len() + 4) as u32).to_le_bytes());
    rest.extend_from_slice(&meta);
    record(&mut rest, header_schema, header);
    for r in records {
        record(&mut rest, schema, r);
    }
    let mut file = Vec::new();
    file.extend_from_slice(&0x20u32.to_le_bytes());
    file.extend_from_slice(&crc32(&rest).to_le_bytes());
    file.extend_from_slice(&(records.len() as u32).to_le_bytes());
    file.extend_from_slice(&rest);
    file
}

/// `data` as a compressed file whose blocks of `block_max` bytes are all stored, not packed:
/// what a reader of a server's file is given in other crates' tests.
pub fn stored(data: &[u8], block_max: usize) -> Vec<u8> {
    let mut file = Vec::new();
    for word in [3u32, 1, block_max as u32, data.len() as u32] {
        file.extend_from_slice(&word.to_le_bytes());
    }
    for block in data.chunks(block_max) {
        for word in [0, block.len() as u32, block.len() as u32, crc32(block)] {
            file.extend_from_slice(&word.to_le_bytes());
        }
        file.extend_from_slice(block);
    }
    file
}

/// The sample details file of [MS-OXOAB] 3.3, as its bytes are listed there.
pub fn spec_sample() -> Vec<u8> {
    let hex = |s: &str| -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    };
    let mut rest = hex("5c000000 04000000 1f000068 00000000 1e000468 00000000 03000168 00000000 1e000268 00000000
         06000000 1e000330 02000000 1f00fe39 02000000 1f000130 01000000 0300fe0f 00000000
         03000039 00000000 03100568 00000000");
    rest.extend(hex("42000000 f0"));
    rest.extend(b"\\Global Address List\0/\0\x06d4f244a8-a8ec-442a-87a3-5236f82cabdc\0");
    let dn = b"/o=example/ou=Exchange Administrative Group (FYDIBOHF23SPDLT)/cn=Recipients/cn=";
    for (size, name, smtp) in [("80000000", "Lisa Miller", "LisaM@example.com"), ("8c000000", "Administrator", "Administrator@example.com")] {
        rest.extend(hex(size));
        rest.push(0xf8);
        rest.extend(dn);
        rest.extend(format!("{name}\0{smtp}\0{name}\0\x06\x00").as_bytes());
    }
    let mut file = hex("20000000 f7dac07f 02000000");
    file.extend(rest);
    file
}
