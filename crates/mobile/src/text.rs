// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The caps on what a query hands to the native side. A message or a server wrote every string
//! here, so each is cut (on a character boundary) and each list is held to a size.

/// Most rows (messages, people, tasks) one window gives.
pub const MAX_WINDOW: u32 = 200;
/// Most occurrences one calendar range gives, and most days it spans.
pub const MAX_OCCURRENCES: usize = 2_000;
pub const MAX_RANGE_DAYS: i64 = 100;
/// Most folders, accounts, task lists and attachments one answer lists.
pub const MAX_FOLDERS: usize = 2_000;
pub const MAX_LISTED: usize = 500;
/// Most addresses (to, cc), categories, emails and phones of one item.
pub const MAX_PEOPLE_IN_ITEM: usize = 100;
pub const MAX_CATEGORIES: usize = 32;

pub const MAX_NAME: usize = 200;
pub const MAX_ADDRESS: usize = 320;
pub const MAX_SUBJECT: usize = 500;
pub const MAX_PREVIEW: usize = 200;
pub const MAX_TITLE: usize = 500;
pub const MAX_LOCATION: usize = 300;
pub const MAX_URL: usize = 4_096;

/// `s` cut to at most `max` characters.
pub fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((end, _)) => s.get(..end).unwrap_or_default().to_string(),
        None => s.to_string(),
    }
}

/// The count asked for, held to `1..=MAX_WINDOW`.
pub fn window(count: u32) -> usize {
    usize::try_from(count.clamp(1, MAX_WINDOW)).unwrap_or(1)
}

/// A count as the `u32` of a record, saturating.
pub fn count_of(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The most items of a list, cut.
pub fn first<T, U>(items: impl IntoIterator<Item = T>, max: usize, map: impl FnMut(T) -> U) -> Vec<U> {
    items.into_iter().take(max).map(map).collect()
}
