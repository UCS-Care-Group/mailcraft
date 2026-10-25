// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #308, part 2: the people the search box offers after `from:`, `to:` and `cc:`, from the
//! contacts and the mail on this computer, and `view.searchSuggestions`.

use mailcraft_model::{Contact, Labeled};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use super::compose::suggest;
use super::view::search_suggestions;
use crate::{Session, sample};

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn session() -> Session {
    Session::at(sample::mailbox(now(), 0), now(), 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn contact(s: &mut Session, name: &str, email: &str) {
    s.mailbox.add_contact(Contact { display_name: name.into(), emails: vec![Labeled::new("work", email)], ..Default::default() });
}

/// What is offered for `q`, the cursor at its end: (what the list shows, the address).
fn people(s: &Session, q: &str) -> Vec<(String, String)> {
    search_suggestions(s, q, q.len()).into_iter().filter_map(|x| Some((x.label, x.address?))).collect()
}

/// After `from:`, the people a new message's recipients come from, contacts first; taken, the
/// search finds that sender's mail and no other.
#[test]
fn after_from_the_people_a_message_is_written_to() {
    let mut s = session();
    let found = search_suggestions(&s, "from:pri", 8);
    let first = found.first().unwrap();
    assert_eq!((first.label.as_str(), first.address.as_deref()), ("Priya Nair", Some("priya@lumen.example")));
    assert_eq!(first.text, "from:priya@lumen.example ");
    let composer: Vec<String> = suggest(&s, "pri", 20).into_iter().map(|x| x.email).collect();
    assert_eq!(people(&s, "to:pri").into_iter().map(|(_, a)| a).collect::<Vec<_>>(), composer[..composer.len().min(8)]);
    assert!(people(&s, "subject:pri").is_empty() && people(&s, "from:").is_empty());
    let said = run(&mut s, "view.search", json!({"query": first.text, "scope": "everywhere"}));
    assert!(said["found"].as_u64().unwrap() > 0, "{said}");
    let rows = s.rows();
    assert!(rows.iter().all(|row| s.mailbox.message(row.id).unwrap().from.email.eq_ignore_ascii_case("priya@lumen.example")));
}

/// A name's accents are left out as the search leaves them out, in the search box and in a new
/// message's recipients alike: `gvoz` is Gvožđar.
#[test]
fn accents_are_left_out_of_names() {
    let mut s = session();
    contact(&mut s, "Ivana Gvožđar", "ivana@example.org");
    contact(&mut s, "Jürgen Straße", "jurgen@example.org");
    for q in ["from:gvoz", "from:GVOŽĐ", "cc:ivana gv", "to:strasse", "to:jurg"] {
        let q = if q.contains(' ') { q.replacen(':', ":\"", 1) } else { q.to_string() };
        assert!(people(&s, &q).iter().any(|(_, a)| a.ends_with("@example.org")), "{q}");
    }
    assert_eq!(suggest(&s, "gvozd", 6).first().map(|x| x.email.as_str()), Some("ivana@example.org"));
    assert_eq!(suggest(&s, "Strass", 6).first().map(|x| x.email.as_str()), Some("jurgen@example.org"));
    // Still the start of a word: not a piece in the middle of one.
    assert!(suggest(&s, "ozd", 6).is_empty() && people(&s, "from:ozd").is_empty());
}

/// An address being typed is the start of an address or of a name, not words of a name: `ada@`
/// is not Ada Smith at another address, though `ada` and `o'brien` find names by their words.
#[test]
fn an_address_being_typed_is_not_words_of_a_name() {
    let mut s = session();
    contact(&mut s, "Ada Smith", "asmith@other.example");
    contact(&mut s, "Sean O'Brien", "sean@example.org");
    let found = |q: &str, email: &str| suggest(&s, q, 20).iter().any(|x| x.email == email);
    assert!(found("ada", "asmith@other.example") && found("o'brien", "sean@example.org"));
    assert!(!found("ada@", "asmith@other.example") && !found("ada@sm", "asmith@other.example"));
    assert!(people(&s, "from:ada@").iter().all(|(_, a)| a != "asmith@other.example"));
    assert!(found("asmith@oth", "asmith@other.example"));
}

/// `view.searchSuggestions` answers what the window's list offers, with places in characters, and
/// changes nothing.
#[test]
fn the_command_answers_what_the_list_offers() {
    let mut s = session();
    let before = s.mailbox.clone();
    let said = run(&mut s, "view.searchSuggestions", json!({"query": "from:pri"}));
    assert_eq!(
        said["suggestions"][0],
        json!({"name": "Priya Nair", "email": "priya@lumen.example", "text": "from:priya@lumen.example ", "start": 0, "end": 8})
    );
    // The cursor and the places are characters: `Grüße ` is six of them and seven bytes.
    let said = run(&mut s, "view.searchSuggestions", json!({"query": "Grüße cat from:x", "cursor": 9}));
    assert_eq!(said["suggestions"], json!([{"name": "category:", "text": "category:", "start": 6, "end": 9}]));
    let said = run(&mut s, "view.searchSuggestions", json!({"query": "category:gr", "cursor": 10_000}));
    let green = &said["suggestions"][0];
    assert_eq!((green["name"].as_str(), green["start"].as_u64(), green["end"].as_u64()), (Some("Green category"), Some(0), Some(11)));
    assert!(green["color"].as_str().is_some_and(|c| c.len() == 7 && c.starts_with('#')), "{green}");
    assert_eq!(run(&mut s, "view.searchSuggestions", json!({"query": "budget"}))["suggestions"], json!([]));
    assert!(s.run("view.searchSuggestions", &json!({})).is_err(), "a query is needed");
    assert!(s.mailbox == before && !s.registry.get("view.searchSuggestions").unwrap().mutates);
    // An agent may ask it without the owner.
    assert_eq!(crate::agent::access("view.searchSuggestions"), Some(crate::agent::Access::Safe));
}

/// The owner's own addresses are offered too, after everyone else: `from:` one finds what was sent
/// from it. A new message's recipients still leave them out.
#[test]
fn the_owners_own_addresses_come_last() {
    let mut s = session();
    contact(&mut s, "Ava Stone", "ava@example.org");
    let me = ("Avery Quinn".to_string(), "avery@lumen.example".to_string());
    let found = people(&s, "from:av");
    assert_eq!(found.first().map(|(_, a)| a.as_str()), Some("ava@example.org"), "{found:?}");
    assert_eq!(found.last(), Some(&me), "{found:?}");
    assert_eq!(people(&s, "to:quinn"), std::slice::from_ref(&me));
    assert_eq!(people(&s, "cc:AVERY@"), [me]);
    assert!(suggest(&s, "av", 20).iter().chain(&suggest(&s, "quinn", 20)).all(|x| x.email != "avery@lumen.example"), "not in a new message");
    let q = "from:avery";
    let own = search_suggestions(&s, q, q.len()).pop().unwrap();
    assert_eq!(own.text, "from:avery@lumen.example ");
    run(&mut s, "view.search", json!({"query": own.text, "scope": "everywhere"}));
    let rows = s.rows();
    assert!(!rows.is_empty() && rows.iter().all(|row| s.mailbox.message(row.id).unwrap().from.email == "avery@lumen.example"));
}
