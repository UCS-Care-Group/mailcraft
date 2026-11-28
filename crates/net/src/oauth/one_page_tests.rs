// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One page for every permission an owner can give (2026-11-17): a sign-in's page asks for the
//! shared mailboxes' and the shared calendars' permissions beside the rest; nothing asks for one of
//! them by itself any more; a page that won't have them is asked again without them; and the one
//! page an older sign-in is put (`sign_in_for_sync`) asks for them too, but never for a personal
//! account, and never past an administrator's no. Nothing here leaves this machine.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use mailcraft_model::Directory;

use super::*;
use crate::fake::{FakeHttp, json_answer};

const SCHEME: &str = "org.example.mail";
const PATH: &str = "/oauth2redirect";
const TOKENS: &str = r#"{"token_type":"Bearer","expires_in":3599,"access_token":"acc-ess","refresh_token":"re-fresh"}"#;
const PATIENCE: Duration = Duration::from_secs(5);

const SHARED_MAIL: &str = "https://graph.microsoft.com/Mail.ReadWrite.Shared";
const SHARED_SEND: &str = "https://graph.microsoft.com/Mail.Send.Shared";
const SHARED_CALENDARS: &str = "https://graph.microsoft.com/Calendars.ReadWrite.Shared";
const TASKS: &str = "https://graph.microsoft.com/Tasks.ReadWrite";
const SETTINGS_WRITE: &str = "https://graph.microsoft.com/MailboxSettings.ReadWrite";
const FAVORITES: [&str; 2] = ["https://graph.microsoft.com/MailboxConfigItem.Read", "https://graph.microsoft.com/Mail.ReadBasic"];

fn oauth() -> OAuth {
    OAuth { provider: OAuthProvider::Microsoft, client_id: "client-1".into(), ..Default::default() }
}

fn words(scope: &str) -> BTreeSet<String> {
    scope.split_whitespace().map(str::to_string).collect()
}

fn has(scope: &str, permission: &str) -> bool {
    scope.split_whitespace().any(|s| s == permission)
}

/// The pages a sign-in opened, by what each asked for, and an answer for each in turn
/// (`{state}` is that page's own).
type Pages = Arc<Mutex<Vec<String>>>;

fn query(url: &str) -> HashMap<String, String> {
    http::pairs(url.split_once('?').map_or("", |(_, q)| q)).into_iter().collect()
}

/// A way back that answers the pages in turn with `answers` (the last one again after that), what
/// opens a page, and the page's `scope`s as they were asked.
fn answering(answers: &[&str]) -> (Box<Handback>, Box<dyn Fn(&str)>, Pages) {
    let pages: Pages = Arc::default();
    let opened: Arc<Mutex<String>> = Arc::default();
    let (log, last) = (pages.clone(), opened.clone());
    let answers: Vec<String> = answers.iter().map(|a| format!("{SCHEME}:{PATH}?{a}&state={{state}}")).collect();
    let handback = move |_: &AtomicBool, _: Duration| {
        let page = query(&last.lock().unwrap());
        let mut log = log.lock().unwrap();
        log.push(page.get("scope").cloned().unwrap_or_default());
        let answer = answers.get(log.len() - 1).or(answers.last()).cloned().unwrap_or_default();
        Ok(answer.replace("{state}", page.get("state").map_or("", String::as_str)))
    };
    let open = move |url: &str| *opened.lock().unwrap() = url.to_string();
    (Box::new(handback), Box::new(open), pages)
}

fn via(handback: &Handback) -> RedirectStrategy<'_> {
    RedirectStrategy::CustomScheme(CustomScheme::new(SCHEME, PATH, handback).unwrap())
}

fn at(server: &FakeHttp, endpoints: Endpoints) -> Endpoints {
    Endpoints { authorize: server.url("/authorize"), token: server.url("/token"), ..endpoints }
}

const YES: &str = "code=c";
const NO_SUCH_SCOPE: &str = "error=invalid_scope&error_description=AADSTS70011%3A+The+provided+value+for+scope+is+not+valid.";
const ADMIN_ONLY: &str = "error=access_denied&error_description=AADSTS90094%3A+An+administrator+of+the+organisation+has+to+approve.";

#[test]
fn a_sign_ins_page_asks_for_every_permission_an_owner_can_give_but_the_favorites() {
    for resource in [Resource::Mail, Resource::Graph, Resource::GraphMail] {
        let page = Endpoints::for_resource(&oauth(), resource);
        for permission in [SHARED_MAIL, SHARED_SEND, SHARED_CALENDARS, TASKS, SETTINGS_WRITE] {
            assert!(has(&page.consent, permission), "{resource:?} misses {permission}: {}", page.consent);
        }
        let asked = format!("{} {} {} {}", page.scope, page.consent, page.fewer, page.lesser);
        for permission in FAVORITES {
            assert!(!has(&asked, permission), "{resource:?}: {asked}");
        }
        // What a page that won't have all of it is asked again for: first without the shared
        // mailboxes' and calendars' (a personal account has no such permissions), then what was
        // asked before any of the extras.
        let without_shared: BTreeSet<String> =
            words(&page.consent).into_iter().filter(|p| ![SHARED_MAIL, SHARED_SEND, SHARED_CALENDARS].contains(&p.as_str())).collect();
        assert_eq!(words(&page.fewer), without_shared, "{resource:?}");
        assert!(has(&page.fewer, TASKS) && has(&page.fewer, SETTINGS_WRITE), "{resource:?}");
        assert!(!page.lesser.contains("Shared") && !has(&page.lesser, TASKS) && !has(&page.lesser, SETTINGS_WRITE), "{resource:?}");
        // The tokens of a sign-in are never for any of the extras: their jobs ask for their own.
        assert!(!page.scope.contains("Shared") && !has(&page.scope, TASKS), "{resource:?}: {}", page.scope);
    }
}

#[test]
fn the_phones_page_asks_for_the_shared_mailboxes_it_reads_and_nothing_it_keeps_no_part_of() {
    for resource in [Resource::Mail, Resource::Graph, Resource::GraphMail] {
        let page = Endpoints::for_resource_without_sync(&oauth(), resource);
        assert!(has(&page.consent, SHARED_MAIL) && has(&page.consent, SHARED_SEND), "{resource:?}: {}", page.consent);
        assert!(!has(&page.consent, SHARED_CALENDARS) && !has(&page.consent, TASKS) && !has(&page.consent, SETTINGS_WRITE), "{}", page.consent);
        // Refused, it is asked again for what it asked before.
        assert!(page.fewer.is_empty() && !page.lesser.is_empty() && !page.lesser.contains("Shared"), "{page:?}");
    }
}

#[test]
fn one_approval_is_exactly_what_the_pages_ask_and_the_favorites() {
    let mut asked = BTreeSet::new();
    for resource in [Resource::Mail, Resource::Graph, Resource::GraphMail] {
        for page in [Endpoints::for_resource(&oauth(), resource), Endpoints::for_resource_without_sync(&oauth(), resource)] {
            asked.extend(words(&format!("{} {} {} {}", page.scope, page.consent, page.fewer, page.lesser)));
        }
    }
    for mail_write in [false, true] {
        let page = sync_endpoints(&oauth(), "ada@example.org", mail_write);
        asked.extend(words(&format!("{} {}", page.scope, page.consent)));
    }
    asked.extend(FAVORITES.iter().map(|p| p.to_string()));
    assert_eq!(words(&admin_scope()), asked);
}

#[test]
fn a_page_that_wont_have_the_shared_permissions_is_asked_again_without_them_and_keeps_the_rest() {
    // An account whose directory isn't known yet (its tenant is `common`).
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let (back, open, pages) = answering(&[NO_SUCH_SCOPE, YES]);
    let page = at(&server, Endpoints::for_resource(&oauth(), Resource::Mail));
    let got = sign_in_refused_via(&page, &oauth(), "ada@example.org", &*open, &AtomicBool::new(false), PATIENCE, &via(&*back));
    assert!(got.is_ok(), "{got:?}");
    let pages = pages.lock().unwrap().clone();
    assert_eq!(pages.len(), 2, "{pages:?}");
    assert!(has(&pages[0], SHARED_MAIL) && has(&pages[0], TASKS));
    assert!(!pages[1].contains("Shared") && has(&pages[1], TASKS) && has(&pages[1], SETTINGS_WRITE), "{}", pages[1]);
}

#[test]
fn a_personal_accounts_first_page_has_no_shared_permissions_and_is_the_only_one() {
    // Microsoft has no shared mailboxes for a personal account: it is not asked for them, and a
    // page that is given is not shown a second time.
    let personal = OAuth { tenant: "consumers".into(), ..oauth() };
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let (back, open, pages) = answering(&[YES]);
    let page = at(&server, Endpoints::for_resource(&personal, Resource::Mail));
    let got = sign_in_refused_via(&page, &personal, "ada@example.org", &*open, &AtomicBool::new(false), PATIENCE, &via(&*back));
    assert!(got.is_ok(), "{got:?}");
    let pages = pages.lock().unwrap().clone();
    assert_eq!(pages.len(), 1, "{pages:?}");
    assert!(!pages[0].contains("Shared") && has(&pages[0], TASKS) && has(&pages[0], SETTINGS_WRITE), "{}", pages[0]);
}

#[test]
fn an_organisation_that_wants_an_administrator_for_the_new_permissions_still_signs_in_with_what_it_had() {
    // The page with everything is refused for an administrator: there is no asking again for a bit
    // less in between, only once more for what was asked before #661, which an administrator
    // approved long ago, so that signing in to mail never waits on the rest.
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let (back, open, pages) = answering(&[ADMIN_ONLY, YES]);
    let page = at(&server, Endpoints::for_resource(&oauth(), Resource::GraphMail));
    let got = sign_in_refused_via(&page, &oauth(), "ada@example.org", &*open, &AtomicBool::new(false), PATIENCE, &via(&*back));
    assert!(got.is_ok(), "{got:?}");
    let pages = pages.lock().unwrap().clone();
    assert_eq!(pages.len(), 2, "{pages:?}");
    assert!(has(&pages[0], SHARED_SEND) && has(&pages[0], TASKS) && !has(&pages[1], SHARED_SEND) && !has(&pages[1], TASKS), "{pages:?}");
    // An administrator even for that: the sign-in says so (the approval help comes up).
    let (back, open, pages) = answering(&[ADMIN_ONLY]);
    let got = sign_in_refused_via(&page, &oauth(), "ada@example.org", &*open, &AtomicBool::new(false), PATIENCE, &via(&*back));
    assert_eq!(got.map_err(|refused| refused.policy).err(), Some(Policy::Approval));
    assert_eq!(pages.lock().unwrap().len(), 2);
    // The owner's own no on the page (Cancel) is one page, and no more.
    let (back, open, pages) = answering(&["error=access_denied&error_description=AADSTS65004%3A+User+declined+to+consent.", YES]);
    let got = sign_in_refused_via(&page, &oauth(), "ada@example.org", &*open, &AtomicBool::new(false), PATIENCE, &via(&*back));
    assert!(got.is_err());
    assert_eq!(pages.lock().unwrap().len(), 1);
}

#[test]
fn the_one_page_for_an_older_sign_in_asks_for_the_shared_permissions_too_but_not_of_a_personal_account() {
    for mail_write in [false, true] {
        // (An account whose directory isn't known yet is a personal one when its tenant or its
        // address says so: Outlook.com's own.)
        for (directory, tenant, email, shared) in [
            (Directory::Unknown, "", "ada@example.org", true),
            (Directory::Organisation, "", "ada@example.org", true),
            (Directory::Personal, "", "ada@example.org", false),
            (Directory::Unknown, "consumers", "ada@example.org", false),
            (Directory::Unknown, "", "ada@outlook.com", false),
        ] {
            let settings = OAuth { directory, tenant: tenant.into(), ..oauth() };
            let page = sync_endpoints(&settings, email, mail_write);
            for permission in [SHARED_MAIL, SHARED_SEND, SHARED_CALENDARS] {
                assert_eq!(has(&page.consent, permission), shared, "{directory:?}: {}", page.consent);
                // The tokens are never for them: a shared mailbox's job asks for its own.
                assert!(!has(&page.scope, permission));
            }
            assert!(has(&page.scope, TASKS) && has(&page.scope, SETTINGS_WRITE));
            assert!(page.fewer.is_empty() && page.lesser.is_empty());
            assert!(!FAVORITES.iter().any(|p| has(&page.consent, p) || has(&page.scope, p)));
        }
    }
}

#[test]
fn the_one_page_is_asked_again_without_the_shared_permissions_only_when_they_cant_be_had() {
    let page = |server: &FakeHttp| at(server, sync_endpoints(&oauth(), "ada@example.org", true));
    // No such permissions for this account: once more without them.
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let (back, open, pages) = answering(&[NO_SUCH_SCOPE, YES]);
    let got = sync_page_within(&page(&server), &oauth(), "ada@example.org", &*open, &AtomicBool::new(false), PATIENCE, &via(&*back));
    assert!(got.as_ref().is_ok_and(|tokens| tokens.shared_not_given), "tokens of a page asked again without the shared permissions say so: {got:?}");
    let pages = pages.lock().unwrap().clone();
    assert_eq!(pages.len(), 2);
    assert!(has(&pages[0], SHARED_MAIL) && !pages[1].contains("Shared") && has(&pages[1], TASKS), "{pages:?}");
    // Given, they say nothing of the kind.
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let (back, open, _) = answering(&[YES]);
    let got = sync_page_within(&page(&server), &oauth(), "ada@example.org", &*open, &AtomicBool::new(false), PATIENCE, &via(&*back));
    assert!(got.as_ref().is_ok_and(|tokens| !tokens.shared_not_given), "{got:?}");
    // The organisation has no To Do lists or settings to give (refused for its scopes both with the
    // shared permissions and without): the shared ones are had without those, and the tokens say so.
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let (back, open, pages) = answering(&[NO_SUCH_SCOPE, NO_SUCH_SCOPE, YES]);
    let got = sync_page_within(&page(&server), &oauth(), "ada@example.org", &*open, &AtomicBool::new(false), PATIENCE, &via(&*back));
    assert!(got.as_ref().is_ok_and(|tokens| tokens.sync_not_given && !tokens.shared_not_given), "{got:?}");
    let pages = pages.lock().unwrap().clone();
    assert_eq!(pages.len(), 3, "{pages:?}");
    assert!(has(&pages[2], SHARED_MAIL) && has(&pages[2], SHARED_SEND) && !has(&pages[2], TASKS) && !has(&pages[2], SETTINGS_WRITE), "{}", pages[2]);
    // An administrator has to approve: said as such, and not asked again without them (the
    // owner is told what to do next; an approval covers all of it).
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let (back, open, pages) = answering(&[ADMIN_ONLY, YES]);
    let got = sync_page_within(&page(&server), &oauth(), "ada@example.org", &*open, &AtomicBool::new(false), PATIENCE, &via(&*back));
    assert_eq!(got, Err(FavoritesRefusal::Approval));
    assert_eq!(pages.lock().unwrap().len(), 1);
    assert_eq!(server.asked(), 0);
    // The owner's own no: the answer, nothing asked again.
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let (back, open, pages) = answering(&["error=access_denied&error_description=The+user+said+no."]);
    let got = sync_page_within(&page(&server), &oauth(), "ada@example.org", &*open, &AtomicBool::new(false), PATIENCE, &via(&*back));
    assert!(matches!(got, Err(FavoritesRefusal::Other(_))), "{got:?}");
    assert_eq!(pages.lock().unwrap().len(), 1);
}

#[test]
fn sending_from_a_shared_mailbox_still_needs_a_token_that_covers_it() {
    // Whatever the page asked: the token a shared mailbox's mail is sent with is asked for with
    // Mail.Send.Shared, and a sign-in that doesn't cover it gets no token at all.
    let unasked = r#"{"error":"invalid_grant","error_description":"AADSTS65001: The user or administrator has not consented.","error_codes":[65001],"suberror":"consent_required"}"#;
    for send in
        [refresh_graph_shared_send_at as fn(&Endpoints, &OAuth, &str) -> Result<GraphTokens, NetError>, refresh_graph_shared_calendars_send_at]
    {
        let server = FakeHttp::answering(json_answer("400 Bad Request", unasked));
        let page = at(&server, Endpoints::default());
        assert!(send(&page, &oauth(), "re-fresh").unwrap_err().is_consent_needed());
        let form: HashMap<String, String> = http::pairs(server.request(0).split_once("\r\n\r\n").map_or("", |(_, body)| body)).into_iter().collect();
        assert!(has(&form["scope"], SHARED_SEND), "{}", form["scope"]);
    }
}
