// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A saved search whose folder was removed is not marked together with the one for the folder
//! in view.

use mailcraft_model::SearchFavorite;

use super::{saved_search_exact, saved_search_on};

fn fav(folder: u64) -> SearchFavorite {
    SearchFavorite { id: folder, name: "Budget".into(), query: "budget".into(), scope: "folder".into(), folder }
}

#[test]
fn a_removed_folder_does_not_share_the_row_that_is_on() {
    let gone = fav(0);
    let here = fav(4);
    let exact = saved_search_exact(&here, "budget", "folder", 4);
    assert!(exact);
    assert!(saved_search_on(&here, "budget", "folder", 4, exact));
    assert!(!saved_search_on(&gone, "budget", "folder", 4, exact));
}

#[test]
fn a_removed_folder_is_the_row_when_nothing_names_the_folder_in_view() {
    let gone = fav(0);
    assert!(!saved_search_exact(&gone, "budget", "folder", 4));
    assert!(saved_search_on(&gone, "budget", "folder", 4, false));
    assert!(!saved_search_on(&gone, "", "folder", 4, false));
}
