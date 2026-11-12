// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The contact form.

use super::*;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn session() -> Session {
    Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0)
}

fn contact_id(s: &Session, name: &str) -> Id {
    s.mailbox.contacts.values().find(|c| c.name() == name).map(|c| c.id).unwrap()
}

#[test]
fn a_new_contact_sends_what_was_filled_in() {
    let mut f = ContactForm::new(&json!({"first": "Ada", "last": "Lovelace", "email": "ada@example.org"}));
    assert_eq!((f.name(), f.title().as_str()), ("contact", "New Contact"));
    assert_eq!(f.command(), ("people.new", json!({"first": "Ada", "last": "Lovelace", "emails": [{"label": "work", "value": "ada@example.org"}]})));
    f.display = "Countess of Lovelace".into();
    f.company = "Analytical Engines".into();
    f.title = "Programmer".into();
    f.department = "Research".into();
    f.emails.push(Labeled::new("home", "ada@home.example"));
    // A row left empty isn't sent.
    f.emails.push(Labeled::new("other", "  "));
    f.phones = vec![Labeled::new("mobile", "+44 20 7946 0000")];
    f.address = PostalAddress {
        label: "home".into(),
        street: "12 St James's Square\nSecond floor".into(),
        city: "London".into(),
        region: String::new(),
        postal_code: "SW1Y 4JH".into(),
        country: "UK".into(),
    };
    f.website = "https://example.org/ada".into();
    f.birthday = "1815-12-10".into();
    f.notes = "Met at the lecture.".into();
    f.categories = vec!["Blue category".into()];
    f.favorite = true;
    let (command, params) = f.command();
    assert_eq!(
        params,
        json!({
            "first": "Ada", "last": "Lovelace", "displayName": "Countess of Lovelace",
            "company": "Analytical Engines", "title": "Programmer", "department": "Research",
            "emails": [{"label": "work", "value": "ada@example.org"}, {"label": "home", "value": "ada@home.example"}],
            "phones": [{"label": "mobile", "value": "+44 20 7946 0000"}],
            "address": {"label": "home", "street": "12 St James's Square\nSecond floor", "city": "London", "region": "", "postalCode": "SW1Y 4JH", "country": "UK"},
            "website": "https://example.org/ada", "birthday": "1815-12-10", "notes": "Met at the lecture.",
            "categories": ["Blue category"], "favorite": true,
        })
    );
    // And the command takes it.
    let mut s = session();
    let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
    let c = s.mailbox.contact(id).unwrap();
    assert_eq!((c.name().as_str(), c.emails.len(), c.birthday, c.favorite), ("Countess of Lovelace", 2, Date::new(1815, 12, 10), true));
    assert_eq!(c.addresses.first().map(|a| a.one_line()).as_deref(), Some("12 St James's Square\nSecond floor, London SW1Y 4JH, UK"));
}

#[test]
fn an_existing_contact_fills_the_form_and_sends_its_changes() {
    let mut s = session();
    // Nobody selected and nobody named: no form.
    assert!(ContactForm::edit(&s, &json!({})).is_none());
    assert!(ContactForm::edit(&s, &json!({"id": 999_999})).is_none());
    let dev = contact_id(&s, "Dev Patel");
    s.sel.contact = Some(dev);
    let mut f = ContactForm::edit(&s, &json!({})).unwrap();
    assert_eq!((f.name(), f.title().as_str(), f.id), ("editContact", "Contact", Some(dev)));
    assert_eq!(
        (f.first.as_str(), f.last.as_str(), f.company.as_str(), f.title.as_str(), f.birthday.as_str()),
        ("Dev", "Patel", "Patel & Rowe", "Architect", "1988-03-14")
    );
    assert_eq!(f.emails, [Labeled::new("work", "dev.patel@example.org"), Labeled::new("home", "dev@patel.example")]);
    assert_eq!(f.phones, [Labeled::new("mobile", "+1 555 0142")]);
    // Untouched, nothing is sent but who it is.
    assert_eq!(f.command(), ("people.edit", json!({"id": dev})));
    f.birthday.clear();
    f.phones[0].value = "+1 555 0199".into();
    f.phones.push(Labeled::new("work", "+1 555 0100"));
    f.favorite = true;
    let (command, params) = f.command();
    assert_eq!(
        params,
        json!({"id": dev, "birthday": null, "favorite": true,
            "phones": [{"label": "mobile", "value": "+1 555 0199"}, {"label": "work", "value": "+1 555 0100"}]})
    );
    s.run(command, &params).unwrap();
    let c = s.mailbox.contact(dev).unwrap();
    assert_eq!((c.birthday, c.phones.len(), c.favorite, c.emails.len(), c.company.as_str()), (None, 2, true, 2, "Patel & Rowe"));
    // An address and a website; `id` wins over the selection.
    let lin = contact_id(&s, "Lin Zhao");
    let mut f = ContactForm::edit(&s, &json!({"id": lin})).unwrap();
    assert_eq!(
        (f.address.street.as_str(), f.address.city.as_str(), f.address.postal_code.as_str(), f.website.as_str()),
        ("40 Dock Road", "Port Haven", "20417", "https://harborworks.example")
    );
    assert_eq!(f.categories, ["Orange category"]);
    f.address.country = "Freedonia".into();
    assert_eq!(
        f.command().1,
        json!({"id": lin, "address": {"label": "work", "street": "40 Dock Road", "city": "Port Haven", "region": "", "postalCode": "20417", "country": "Freedonia"}})
    );
    // A contact without a phone still gets a row to type one in.
    let books = contact_id(&s, "Harbor Books");
    let f = ContactForm::edit(&s, &json!({"id": books})).unwrap();
    assert_eq!((f.phones.as_slice(), f.address.label.as_str()), ([Labeled::new("mobile", "")].as_slice(), "work"));
    assert_eq!(f.command().1, json!({"id": books}));
}

#[test]
fn a_book_says_whose_account_it_is() {
    let mut account = mailcraft_model::Account { name: "Ada".into(), email: "ada@example.org".into(), ..Default::default() };
    assert_eq!(ContactForm::book_label("Contacts", &account), "Contacts (Ada)");
    account.name.clear();
    assert_eq!(ContactForm::book_label("Contacts", &account), "Contacts (ada@example.org)");
    account.email.clear();
    assert_eq!(ContactForm::book_label("Contacts", &account), "Contacts");
}

#[test]
fn a_new_contact_defaults_to_a_writable_server_book() {
    use mailcraft_model::{ContactBook, Rights};
    let mut s = session();
    let account = s.mailbox.accounts.first().unwrap().id;
    let book = "https://mail.example.org/carddav/ada@example.org/contacts/personal/";
    s.mailbox.contact_books.push(ContactBook { account, id: book.into(), name: "Contacts".into(), rights: Rights::Read });
    let mut f = ContactForm::new(&json!({}));
    f.default_book(&s);
    assert!(f.address_book.is_empty(), "a read-only book is not a default");
    s.mailbox.contact_books[0].rights = Rights::Edit;
    f.default_book(&s);
    assert_eq!((f.address_book.as_str(), f.address_book_account), (book, account));
}

#[test]
fn a_contact_in_a_read_only_book_has_no_form() {
    use mailcraft_model::{Contact, ContactBook, Origin, Rights};
    let mut s = session();
    let account = s.mailbox.accounts.first().unwrap().id;
    let book = "https://mail.example.org/carddav/ada@example.org/contacts/personal/";
    s.mailbox.contact_books.push(ContactBook { account, id: book.into(), name: "Contacts".into(), rights: Rights::Read });
    let id = s.mailbox.add_contact(Contact {
        first: "Ada".into(),
        display_name: "Ada".into(),
        uid: "ada-book".into(),
        book: "Contacts".into(),
        book_id: book.into(),
        origin: Origin { account, id: "/carddav/ada@example.org/contacts/personal/ada-book.vcf".into(), ..Default::default() },
        ..Default::default()
    });
    assert!(ContactForm::edit(&s, &json!({"id": id})).is_none(), "a read-only book's contact cannot be opened");
    // The same contact is editable in a book the account may write to.
    s.mailbox.contact_books[0].rights = Rights::Edit;
    assert!(ContactForm::edit(&s, &json!({"id": id})).is_some());
}

#[test]
fn a_new_contact_can_say_which_address_book_to_save_it_in() {
    use mailcraft_model::{ContactBook, Rights};
    let mut s = session();
    let account = s.mailbox.accounts.first().unwrap().id;
    let book = "https://mail.example.org/carddav/ada@example.org/contacts/personal/";
    s.mailbox.contact_books.push(ContactBook { account, id: book.into(), name: "My contacts".into(), rights: Rights::Edit });
    // A new contact without a choice says nothing, and stays on this computer.
    assert!(ContactForm::new(&json!({"first": "Ada"})).command().1.get("addressBook").is_none());
    let mut f = ContactForm::new(&json!({"first": "Ada"}));
    f.address_book = book.into();
    f.address_book_account = account;
    let (command, params) = f.command();
    assert_eq!((command, params.get("addressBook").and_then(Value::as_str)), ("people.new", Some(book)));
    assert_eq!(params.get("addressBookAccount").and_then(Value::as_u64), Some(account));
    let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
    let c = s.mailbox.contact(id).unwrap();
    assert_eq!((c.book.as_str(), c.book_id.as_str(), c.origin.account), ("My contacts", book, account));
    assert!(!c.uid.is_empty());
}

#[test]
fn ok_waits_for_someone_to_save() {
    let mut f = ContactForm::new(&json!({}));
    assert!(!f.ready());
    assert_eq!(f.command(), ("people.new", json!({})));
    f.notes = "Only a note".into();
    assert!(!f.ready());
    f.company = "Harbor Books".into();
    assert!(f.ready());
    f.company.clear();
    f.emails[0].value = "x@example.org".into();
    assert!(f.ready());
    // A birthday that isn't a date holds OK back.
    f.birthday = "14 March".into();
    assert!(!f.ready());
    f.birthday = "1988-03-14".into();
    assert!(f.ready());
    assert_eq!((label_text("WORK").as_str(), label_text("pager").as_str(), label_text("").as_str()), ("Work", "pager", ""));
}

#[test]
fn hostile_text_goes_through_without_harm() {
    let junk = "\u{0}\u{202e}é\n".repeat(40_000);
    let p = json!({"first": junk, "last": 7, "email": junk, "phone": junk, "displayName": junk, "company": [junk]});
    let mut f = ContactForm::new(&p);
    assert!(f.ready() && f.last.is_empty() && f.company.is_empty());
    (f.address.street, f.address.label, f.website, f.notes) = (junk.clone(), junk.clone(), junk.clone(), junk.clone());
    f.categories = vec![junk.clone(); 50];
    f.birthday.clone_from(&junk);
    assert!(!f.ready());
    // A birthday that isn't a date goes as typed, and the command refuses it.
    let mut s = session();
    let (command, params) = f.command();
    assert!(s.run(command, &params).unwrap_err().to_string().contains("`birthday` must be a date"));
    f.birthday.clear();
    let (command, params) = f.command();
    let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
    // The command cut the one-line fields down.
    assert!(s.mailbox.contact(id).unwrap().first.chars().count() <= 300);
    assert!(label_text(&junk).chars().count() <= 41);
}
