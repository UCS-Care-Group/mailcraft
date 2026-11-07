// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A deserializer that charges what is read through it to the budget in force ([`super::within`]).
//!
//! Every compound value (a struct, a map, a list, a tuple, an enum, a value of any kind) is
//! charged its own size when it is made, every list element twice its size and a little more (the
//! slot it takes, the room a growing list keeps, an allocation's header), and every entry of a map
//! the same for its key and its value. That is what a value costs in memory over the text it is
//! read from, whatever the type and however deep it is: a new field of any type in a mailbox file
//! is charged without being named here. Strings and byte strings are not charged: they cost no
//! more than the text they are read from, which the caller bounds.

use std::fmt;
use std::mem::size_of;

use serde::de::{DeserializeSeed, Deserializer, EnumAccess, Error, MapAccess, SeqAccess, VariantAccess, Visitor};

use super::charge_bytes;

/// What one element of a list, or one key or value of a map, of `T` is charged.
fn slot<T>() -> u64 {
    (size_of::<T>() as u64).saturating_mul(2).saturating_add(16)
}

/// What a compound value of `T` is charged when it is made.
fn made<T, E: Error>() -> Result<(), E> {
    charge_bytes(size_of::<T>() as u64)
}

/// A deserializer whose values are charged to the budget in force as they are read.
pub struct Counting<D>(pub D);

/// Whether the entries of a map being read are a map's (charged) or a struct's fields (the
/// struct's own size has them).
#[derive(Clone, Copy)]
enum Entries {
    Charged,
    Fields,
}

/// A visitor whose nested values are read through [`Counting`].
struct Wrap<V> {
    inner: V,
    entries: Entries,
}

/// A seed whose value is read through [`Counting`].
struct Seed<S>(S);

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for Seed<S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<S::Value, D::Error> {
        self.0.deserialize(Counting(deserializer))
    }
}

macro_rules! forward_plain {
    ($($method:ident)*) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, D::Error> {
                self.0.$method(Wrap { inner: visitor, entries: Entries::Charged })
            }
        )*
    };
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for Counting<D> {
    type Error = D::Error;

    forward_plain! {
        deserialize_bool deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64 deserialize_i128
        deserialize_u8 deserialize_u16 deserialize_u32 deserialize_u64 deserialize_u128 deserialize_f32
        deserialize_f64 deserialize_char deserialize_str deserialize_string deserialize_bytes
        deserialize_byte_buf deserialize_option deserialize_unit deserialize_identifier deserialize_ignored_any
    }

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, D::Error> {
        made::<V::Value, D::Error>()?;
        self.0.deserialize_any(Wrap { inner: visitor, entries: Entries::Charged })
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, name: &'static str, visitor: V) -> Result<V::Value, D::Error> {
        self.0.deserialize_unit_struct(name, Wrap { inner: visitor, entries: Entries::Charged })
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, name: &'static str, visitor: V) -> Result<V::Value, D::Error> {
        self.0.deserialize_newtype_struct(name, Wrap { inner: visitor, entries: Entries::Charged })
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, D::Error> {
        made::<V::Value, D::Error>()?;
        self.0.deserialize_seq(Wrap { inner: visitor, entries: Entries::Charged })
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, D::Error> {
        made::<V::Value, D::Error>()?;
        self.0.deserialize_tuple(len, Wrap { inner: visitor, entries: Entries::Charged })
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(self, name: &'static str, len: usize, visitor: V) -> Result<V::Value, D::Error> {
        made::<V::Value, D::Error>()?;
        self.0.deserialize_tuple_struct(name, len, Wrap { inner: visitor, entries: Entries::Charged })
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, D::Error> {
        made::<V::Value, D::Error>()?;
        self.0.deserialize_map(Wrap { inner: visitor, entries: Entries::Charged })
    }

    fn deserialize_struct<V: Visitor<'de>>(self, name: &'static str, fields: &'static [&'static str], visitor: V) -> Result<V::Value, D::Error> {
        made::<V::Value, D::Error>()?;
        self.0.deserialize_struct(name, fields, Wrap { inner: visitor, entries: Entries::Fields })
    }

    fn deserialize_enum<V: Visitor<'de>>(self, name: &'static str, variants: &'static [&'static str], visitor: V) -> Result<V::Value, D::Error> {
        made::<V::Value, D::Error>()?;
        self.0.deserialize_enum(name, variants, Wrap { inner: visitor, entries: Entries::Charged })
    }

    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}

macro_rules! forward_visit {
    ($($method:ident: $ty:ty)*) => {
        $(
            fn $method<E: Error>(self, v: $ty) -> Result<V::Value, E> {
                self.inner.$method(v)
            }
        )*
    };
}

impl<'de, V: Visitor<'de>> Visitor<'de> for Wrap<V> {
    type Value = V::Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.inner.expecting(f)
    }

    forward_visit! {
        visit_bool: bool visit_i8: i8 visit_i16: i16 visit_i32: i32 visit_i64: i64 visit_i128: i128
        visit_u8: u8 visit_u16: u16 visit_u32: u32 visit_u64: u64 visit_u128: u128 visit_f32: f32
        visit_f64: f64 visit_char: char visit_str: &str visit_borrowed_str: &'de str visit_string: String
        visit_bytes: &[u8] visit_borrowed_bytes: &'de [u8] visit_byte_buf: Vec<u8>
    }

    fn visit_none<E: Error>(self) -> Result<V::Value, E> {
        self.inner.visit_none()
    }

    fn visit_unit<E: Error>(self) -> Result<V::Value, E> {
        self.inner.visit_unit()
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<V::Value, D::Error> {
        self.inner.visit_some(Counting(deserializer))
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, deserializer: D) -> Result<V::Value, D::Error> {
        self.inner.visit_newtype_struct(Counting(deserializer))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<V::Value, A::Error> {
        self.inner.visit_seq(Seq(seq))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<V::Value, A::Error> {
        self.inner.visit_map(Map { inner: map, entries: self.entries })
    }

    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<V::Value, A::Error> {
        self.inner.visit_enum(Enum(data))
    }
}

/// A list whose elements are charged.
struct Seq<A>(A);

impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for Seq<A> {
    type Error = A::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, A::Error> {
        let next = self.0.next_element_seed(Seed(seed))?;
        if next.is_some() {
            charge_bytes::<A::Error>(slot::<T::Value>())?;
        }
        Ok(next)
    }

    fn size_hint(&self) -> Option<usize> {
        self.0.size_hint()
    }
}

/// A map whose entries are charged (or a struct's fields, which are not).
struct Map<A> {
    inner: A,
    entries: Entries,
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for Map<A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, A::Error> {
        let next = self.inner.next_key_seed(Seed(seed))?;
        if next.is_some() && matches!(self.entries, Entries::Charged) {
            charge_bytes::<A::Error>(slot::<K::Value>())?;
        }
        Ok(next)
    }

    fn next_value_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<T::Value, A::Error> {
        let value = self.inner.next_value_seed(Seed(seed))?;
        if matches!(self.entries, Entries::Charged) {
            charge_bytes::<A::Error>(slot::<T::Value>())?;
        }
        Ok(value)
    }

    fn size_hint(&self) -> Option<usize> {
        self.inner.size_hint()
    }
}

/// An enum whose content is read through [`Counting`].
struct Enum<A>(A);

impl<'de, A: EnumAccess<'de>> EnumAccess<'de> for Enum<A> {
    type Error = A::Error;
    type Variant = Variant<A::Variant>;

    fn variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<(S::Value, Variant<A::Variant>), A::Error> {
        let (value, variant) = self.0.variant_seed(seed)?;
        Ok((value, Variant(variant)))
    }
}

struct Variant<A>(A);

impl<'de, A: VariantAccess<'de>> VariantAccess<'de> for Variant<A> {
    type Error = A::Error;

    fn unit_variant(self) -> Result<(), A::Error> {
        self.0.unit_variant()
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, A::Error> {
        self.0.newtype_variant_seed(Seed(seed))
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, A::Error> {
        made::<V::Value, A::Error>()?;
        self.0.tuple_variant(len, Wrap { inner: visitor, entries: Entries::Charged })
    }

    fn struct_variant<V: Visitor<'de>>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value, A::Error> {
        made::<V::Value, A::Error>()?;
        self.0.struct_variant(fields, Wrap { inner: visitor, entries: Entries::Fields })
    }
}
