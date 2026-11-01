// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! M4.4: a sign-in on Microsoft's page that an organisation has to approve first. What the
//! wizard says while the browser has the sign-in, the page of its own about an approval, how a
//! sign-in that ended says so, the same page for an account that exists, and the commands for
//! all of it. The host is a stand-in that answers what a test tells it to: nobody is signed in,
//! no browser is opened and nothing leaves this machine.

use std::sync::mpsc::Sender;

use egui_kittest::kittest::NodeT as _;

use super::*;
use crate::approval::{NEEDS_APPROVAL, NO_BROWSER, SIGN_IN_DECLINED, SIGN_IN_TIMED_OUT};
use crate::{AdminConsentAnswer, AdminConsentRequest};

/// The directory of the organisation `example.org` stands for, as a sign-in names it: invented.
const ORGANISATION: &str = "3a1c5e7b-9d2f-4a6c-8e0b-1d3f5a7c9e2b";
/// What the two things to do about an approval are called.
const ADMINISTRATOR: &str = "I'm an administrator: approve it for my organisation";
const COPY: &str = "Copy a message for my administrator";

/// What an approval was asked with: the account (0: the one being set up), the directory, the
/// application, and the flag that stops it.
type Approvals = Log<(Id, String, String, Arc<AtomicBool>)>;

/// A host that has administrators approve: it answers `answer`, or leaves Microsoft's page open.
fn approvals(app: &mut MailApp, answer: Option<AdminConsentAnswer>) -> Approvals {
    let asked: Approvals = Default::default();
    let log = asked.clone();
    let silent: Log<Sender<AdminConsentAnswer>> = Default::default();
    app.services.admin_consent = Some(Box::new(move |req: AdminConsentRequest| {
        log.lock().unwrap().push((req.account, req.tenant.clone(), req.oauth.client_id.clone(), req.cancel.clone()));
        let (tx, rx) = mpsc::channel();
        match &answer {
            Some(answer) => tx.send(answer.clone()).unwrap(),
            None => silent.lock().unwrap().push(tx),
        }
        rx
    }));
    asked
}

/// A host that makes the address an administrator approves at: a stand-in's, which names what
/// it was given. The log is the directory, each time one was made.
fn links(app: &mut MailApp) -> Log<String> {
    let asked: Log<String> = Default::default();
    let log = asked.clone();
    app.services.admin_consent_link = Some(Box::new(move |oauth, tenant| {
        log.lock().unwrap().push(tenant.to_string());
        let directory = if tenant.is_empty() { "organizations" } else { tenant };
        Some(format!("https://login.example.org/{directory}/v2.0/adminconsent?client_id={}", oauth.client_id))
    }));
    asked
}

/// The senders a host that leaves owners in the browser holds: how a test answers later.
type Held = Log<Sender<Result<SignedIn, String>>>;

/// A host that signs owners in when the test says so, and what it was asked: the address, and
/// whether for Microsoft Graph and for the Favorites.
fn sign_ins_held(app: &mut MailApp) -> (Log<(String, bool, bool, Arc<AtomicBool>)>, Held) {
    let asked: Log<(String, bool, bool, Arc<AtomicBool>)> = Default::default();
    let held: Held = Default::default();
    let (log, keep) = (asked.clone(), held.clone());
    app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
        log.lock().unwrap().push((req.email.clone(), req.graph, req.favorites, req.cancel.clone()));
        let (tx, rx) = mpsc::channel();
        keep.lock().unwrap().push(tx);
        rx
    }));
    (asked, held)
}

/// Answer the sign-in that was asked for last.
fn answer(held: &Held, with: Result<SignedIn, String>) {
    held.lock().unwrap().last().expect("a sign-in was asked for").send(with).unwrap();
}

/// A host that could read Outlook's Favorites through Microsoft Graph.
fn reads_favorites(app: &mut MailApp) {
    app.services.graph = Some(Box::new(|request| {
        let (tx, rx) = mpsc::channel();
        tx.send(crate::GraphResult { account: request.account.id, ..Default::default() }).unwrap();
        rx
    }));
}

/// The wizard after Connect on `noor@example.org`, an address at a company's own domain whose
/// mail Microsoft 365 takes, with a host that `host` sets up.
fn work_account(host: impl FnOnce(&mut MailApp)) -> Harness<'static, MailApp> {
    let mut a = app(false);
    a.services.microsoft_client_id = APP_ID.into();
    lookups(&mut a, Some(Ok(vec![MICROSOFT_MX])));
    reads_favorites(&mut a);
    host(&mut a);
    let mut h = open(a, json!({}));
    connect(&mut h, "noor@example.org");
    h.run_steps(4);
    h
}

/// Who signed in, as an organisation's directory names them.
fn signed_in_at_work() -> SignedIn {
    // (Who signed in is left unsaid: these are taken for whichever account asked, #103.)
    SignedIn { tenant: ORGANISATION.into(), email: String::new(), ..signed_in("Noor Haddad") }
}

/// Press one of the things to do: it is known by what it is called, with or without the
/// sentence about it.
fn press_step(h: &mut Harness<'static, MailApp>, title: &str) {
    let named = |node: &egui_kittest::Node<'_>| node.accesskit_node().label().is_some_and(|label| label.starts_with(title));
    h.get_all_by_role(Role::Button).find(named).unwrap_or_else(|| panic!("nothing to press is called {title}")).click();
    h.run_steps(5);
}

/// Where the thing of `kind` was drawn in the last frame, for the account `id` (0: the one being set up).
fn at(h: &Harness<'static, MailApp>, kind: &str, id: Id) -> Option<egui::Rect> {
    h.state().hit.rect(kind, id)
}

/// A click of the mouse at `pos`.
fn click_at(h: &mut Harness<'static, MailApp>, pos: egui::Pos2) {
    h.event(egui::Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(4);
}

/// A click of the mouse on the thing of `kind`, where it was drawn in the last frame.
fn click_kind(h: &mut Harness<'static, MailApp>, kind: &str, id: Id) {
    let rect = at(h, kind, id).unwrap_or_else(|| panic!("no {kind} on screen"));
    click_at(h, rect.center());
}

/// Run frames until one hands something to the clipboard (a few at most): what it was. The
/// window's way to the clipboard is the same on every system, and no clipboard is needed here.
fn copied(h: &mut Harness<'static, MailApp>) -> Option<String> {
    for _ in 0..8 {
        h.step();
        let found = h.output().platform_output.commands.iter().rev().find_map(|command| match command {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        });
        if found.is_some() {
            return found;
        }
    }
    None
}

fn status(h: &Harness<'static, MailApp>) -> String {
    h.state().status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default()
}

fn stopped(flags: &[Arc<AtomicBool>]) -> bool {
    !flags.is_empty() && flags.iter().all(|flag| flag.load(Ordering::Relaxed))
}

/// (1) While the browser has the sign-in, the page says so and, without waiting for anything
/// to come back, what to do if Microsoft's page asks for an administrator.
#[test]
fn the_waiting_page_says_what_to_do_about_an_approval_without_waiting() {
    let mut held = None;
    let mut made = None;
    let mut h = work_account(|a| {
        held = Some(sign_ins_held(a));
        made = Some(links(a));
        approvals(a, None);
    });
    let ((asked, held), made) = (held.unwrap(), made.unwrap());
    assert_eq!((page(&h), wizard(&h).map(|w| w.waiting), h.state().signing_in()), (Some(Page::SignIn), Some(true), true));
    // What is happening, and for whom.
    assert!(says(&h, "Microsoft's sign-in page is open in your browser") && says(&h, "Sign in there as noor@example.org"));
    // Some systems can't say that no browser started, so the page says what to do if none did.
    assert!(says(&h, "If no browser opened, check that this computer has a default browser"));
    // Nothing is said of a step about Outlook's Favorites: there is none.
    assert!(!says(&h, "First you sign in at Microsoft") && !says(&h, "Favorites"));
    // And what to do if that page says no: why, in plain words, and the things to do, in view.
    for said in [
        "If that page says you need admin approval",
        "Your organisation lets only an administrator approve a new program",
        "Nothing is wrong with your account or your password",
        "there is nothing to wait for here",
        ADMINISTRATOR,
        COPY,
        "More about this",
        "Open the page again",
    ] {
        assert!(says(&h, said), "{said}");
    }
    for kind in ["approvalAdministrator", "approvalCopyMessage", "signInApprovalHelp", "signInTryAgain"] {
        assert!(at(&h, kind, 0).is_some(), "{kind}");
    }
    // The page that only waited is no more: what there is to do stands under what is happening.
    let (first, second) = (at(&h, "approvalAdministrator", 0).unwrap(), at(&h, "approvalCopyMessage", 0).unwrap());
    assert!(second.min.y >= first.max.y - 0.5 && first.width() > 300.0);

    // The message can be copied at once, and the sign-in goes on waiting meanwhile.
    h.get_by_role_and_label(Role::Button, COPY).click();
    let message = copied(&mut h).expect("a message on the clipboard");
    h.run_steps(3);
    assert!(message.contains("https://login.example.org/example.org/v2.0/adminconsent?client_id=11111111-2222-3333-4444-555555555555"), "{message}");
    assert!(status(&h).contains("clipboard") && h.state().signing_in() && page(&h) == Some(Page::SignIn));
    assert_eq!(*made.lock().unwrap(), ["example.org".to_string()]);

    // The page about an approval, from its link: the sign-in still waits behind it, for the
    // owner may yet sign in on Microsoft's page (with an administrator's account, say).
    click_kind(&mut h, "signInApprovalHelp", 0);
    assert_eq!(
        (page(&h), wizard(&h).map(|w| (w.waiting, w.approval.said)), h.state().signing_in()),
        (Some(Page::Approval), Some((true, false)), true)
    );
    assert!(says(&h, "Your organisation has to approve MailCraft first") && !says(&h, "Microsoft's page said so"));
    assert_eq!((asked.lock().unwrap().len(), addresses(&h)), (1, vec![String::new()]));
    answer(&held, Ok(signed_in_at_work()));
    h.run_steps(6);
    // Signed in after all: the account is added, and that is all.
    assert_eq!((page(&h), addresses(&h)), (Some(Page::Done), vec!["noor@example.org".to_string()]));

    // "Open the page again", for a page that was closed: the sign-in afresh, the old one stopped.
    let mut held = None;
    let mut h = work_account(|a| held = Some(sign_ins_held(a)));
    let (asked, _) = held.unwrap();
    click(&mut h, "Open the page again");
    let flags: Vec<Arc<AtomicBool>> = asked.lock().unwrap().iter().map(|(.., stop)| stop.clone()).collect();
    assert_eq!((page(&h), wizard(&h).map(|w| w.waiting), flags.len()), (Some(Page::SignIn), Some(true), 2));
    assert!(flags[0].load(Ordering::Relaxed) && !flags[1].load(Ordering::Relaxed) && h.state().signing_in());
    // Cancel is what it was: back to the address, nobody waiting.
    click(&mut h, "Cancel");
    assert_eq!((page(&h), h.state().signing_in(), addresses(&h)), (Some(Page::Address), false, vec![String::new()]));

    // An Outlook.com account is told the same of an approval.
    let mut a = app(false);
    a.services.microsoft_client_id = APP_ID.into();
    reads_favorites(&mut a);
    sign_ins(&mut a, None);
    let mut h = open(a, json!({"email": "noor@example.org"}));
    connect_manually(&mut h);
    click(&mut h, Kind::OutlookCom.title());
    assert!(page(&h) == Some(Page::SignIn) && says(&h, "If that page says you need admin approval"));
}

/// (2) A sign-in that comes back as needing an administrator shows the page of its own; an
/// owner who is one approves from there, and the wizard goes on by itself to the sign-in and the
/// account.
#[test]
fn a_sign_in_refused_for_approval_shows_the_page_and_an_administrator_approves_from_it() {
    let mut asked = None;
    let mut h = work_account(|a| {
        sign_ins(a, Some(Err(NEEDS_APPROVAL.into())));
        asked = Some(approvals(a, None));
        links(a);
    });
    let asked = asked.unwrap();
    assert_eq!(
        (page(&h), wizard(&h).map(|w| (w.waiting, w.approval.said)), h.state().signing_in()),
        (Some(Page::Approval), Some((false, true)), false)
    );
    assert_eq!(addresses(&h), [""]);
    // Why, in plain words, and what to do next, as things to press.
    for said in [
        "Your organisation has to approve MailCraft first",
        "noor@example.org",
        "Microsoft's page said so.",
        "lets only an administrator approve a new program, and none has approved MailCraft yet",
        "Nothing is wrong with your account or your password.",
        "What to do next",
        ADMINISTRATOR,
        "Opens Microsoft's approval page in your browser",
        COPY,
        "MailCraft sends nothing itself",
        "If you have an administrator account of your own, the first choice lets you sign in with it on Microsoft's page.",
        "Once an administrator has approved, try again",
    ] {
        assert!(says(&h, said), "{said}");
    }
    for kind in ["approvalAdministrator", "approvalCopyMessage", "approvalTryAgain", "approvalNotNow"] {
        assert!(at(&h, kind, 0).is_some(), "{kind}");
    }
    assert!(h.query_by_role_and_label(Role::Button, "Try again").is_some() && h.query_by_role_and_label(Role::Button, "Not now").is_some());
    // Enter is nobody's here: nothing opens a page of Microsoft's by a key that was held.
    press(&mut h, Key::Enter);
    assert_eq!((page(&h), asked.lock().unwrap().len(), h.state().signing_in()), (Some(Page::Approval), 0, false));

    // "I'm an administrator": Microsoft's approval page, for this application and the
    // organisation of the address; the page waits for it.
    press_step(&mut h, ADMINISTRATOR);
    {
        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        let (account, directory, application, _) = &asked[0];
        assert_eq!((*account, directory.as_str(), application.as_str()), (0, "example.org", APP_ID));
    }
    assert!(h.state().approving() && !h.state().signing_in() && wizard(&h).is_some_and(|w| w.approval.approving && w.busy()));
    assert!(says(&h, "Microsoft's approval page is open in your browser") && says(&h, "Sign in there with an administrator account and accept"));
    assert!(h.query_by_role_and_label(Role::Button, "Not now").is_none());
    // Cancel there: back to what there is to do, and the host is told to stop.
    click(&mut h, "Cancel");
    assert_eq!((page(&h), h.state().approving(), wizard(&h).map(|w| w.approval.approving)), (Some(Page::Approval), false, Some(false)));
    assert!(stopped(&[asked.lock().unwrap()[0].3.clone()]) && says(&h, "What to do next"));

    // Whoever signed in on Microsoft's page was no administrator: what Microsoft answered is
    // said, with what to do instead, and the page is where it was.
    let refused = AdminConsentAnswer::NotAdministrator("AADSTS90094: Administrator consent is required.".into());
    approvals(h.state_mut(), Some(refused));
    press_step(&mut h, ADMINISTRATOR);
    h.run_steps(4);
    assert_eq!((page(&h), h.state().approving(), addresses(&h)), (Some(Page::Approval), false, vec![String::new()]));
    assert!(
        says(&h, "is not an administrator of your organisation") && says(&h, "Microsoft answered: AADSTS90094: Administrator consent is required.")
    );
    assert!(says(&h, "Copy the message for an administrator instead") && says(&h, "What to do next"));
    for (answer, said) in [
        (AdminConsentAnswer::Declined("AADSTS65004: User declined to consent to access the app.".into()), "came back without the approval"),
        (AdminConsentAnswer::TimedOut, "Nothing came back from Microsoft's approval page"),
        (AdminConsentAnswer::NoBrowser, "couldn't open your browser for Microsoft's approval page"),
        (AdminConsentAnswer::Failed("the approval page came back without an answer".into()), "The approval didn't go through."),
    ] {
        approvals(h.state_mut(), Some(answer));
        press_step(&mut h, ADMINISTRATOR);
        h.run_steps(4);
        assert!(page(&h) == Some(Page::Approval) && says(&h, said) && !says(&h, "is not an administrator"), "{said}");
    }

    // An administrator approves. The wizard says so and goes on to the sign-in by itself…
    approvals(h.state_mut(), Some(AdminConsentAnswer::Approved));
    let (signed, held) = sign_ins_held(h.state_mut());
    let synced = syncs(h.state_mut());
    press_step(&mut h, ADMINISTRATOR);
    h.run_steps(4);
    assert_eq!((page(&h), wizard(&h).map(|w| (w.waiting, w.approved)), h.state().signing_in()), (Some(Page::SignIn), Some((true, true)), true));
    assert!(says(&h, "Your organisation has approved MailCraft. Now sign in with your own account."));
    assert!(says(&h, "Microsoft's sign-in page is open in your browser") && !h.state().approving());
    let asked_for: Vec<String> = signed.lock().unwrap().iter().map(|(address, ..)| address.clone()).collect();
    assert_eq!(asked_for, ["noor@example.org"]);
    // …which now goes through: the account is added and its mail is under way.
    answer(&held, Ok(signed_in_at_work()));
    h.run_steps(6);
    // (Kept in step through Graph, R3: nothing by IMAP.)
    assert_eq!((page(&h), addresses(&h), synced.lock().unwrap().len()), (Some(Page::Done), vec!["noor@example.org".to_string()], 0));
    assert!(says(&h, "noor@example.org is set up") && !h.state().signing_in());

    // Approved, and the sign-in is refused for an approval all the same (another permission,
    // another directory): the page is back, saying so.
    let mut h = work_account(|a| {
        sign_ins(a, Some(Err(NEEDS_APPROVAL.into())));
        approvals(a, Some(AdminConsentAnswer::Approved));
    });
    press_step(&mut h, ADMINISTRATOR);
    h.run_steps(6);
    assert_eq!(
        (page(&h), wizard(&h).map(|w| (w.approval.said, w.approved)), addresses(&h)),
        (Some(Page::Approval), Some((true, false)), vec![String::new()])
    );

    // A build that can't open that page says so, and nothing is given up for it.
    let mut h = work_account(|a| {
        sign_ins(a, None);
    });
    press_step(&mut h, ADMINISTRATOR);
    assert!(page(&h) == Some(Page::SignIn) && h.state().signing_in() && says(&h, "This build can't open Microsoft's approval page"));
}

/// (2, continued) The message for an administrator: the link, what it is for, and nothing of
/// the owner. Nothing is sent, and the status bar says where the message is.
#[test]
fn the_copied_message_has_the_link_and_no_address_of_the_owner() {
    let mut made = None;
    let mut h = work_account(|a| {
        sign_ins(a, Some(Err(NEEDS_APPROVAL.into())));
        made = Some(links(a));
    });
    let made = made.unwrap();
    h.get_all_by_role(Role::Button).find(|node| node.accesskit_node().label().is_some_and(|label| label.starts_with(COPY))).unwrap().click();
    let message = copied(&mut h).expect("a message on the clipboard");
    h.run_steps(3);
    let link = format!("https://login.example.org/example.org/v2.0/adminconsent?client_id={APP_ID}");
    // What the program is, that an approval is asked once for the organisation, the address on a
    // line of its own, and what the administrator will see afterwards.
    for said in [
        "MailCraft is an open-source mail and calendar program",
        "approval once, for the whole organisation",
        "a page that can't be reached",
        "That is expected",
    ] {
        assert!(message.contains(said), "{said}: {message}");
    }
    assert!(message.lines().any(|line| line == link), "{message}");
    // Nothing of the owner: no address, no name of the mailbox, no name.
    for never in ["noor", "Noor", "@", "Haddad"] {
        assert!(!message.contains(never), "{never}: {message}");
    }
    // (The directory in the address is the organisation's own domain, which the address names.)
    assert_eq!(*made.lock().unwrap(), ["example.org".to_string()]);
    // The page is where it was, nothing was asked of anybody, and the status bar says what happened.
    assert_eq!((page(&h), h.state().signing_in(), h.state().approving(), addresses(&h)), (Some(Page::Approval), false, false, vec![String::new()]));
    assert_eq!(status(&h), "The message is on the clipboard: paste it into mail or chat for your organisation's administrator");

    // The command says the same without a click, and answers with what it copied.
    let said = h.state_mut().execute("ui.copyAdminMessage", json!({})).unwrap();
    assert_eq!((said["account"].clone(), said["url"].clone(), said["text"].clone()), (json!(0), json!(link), json!(message)));

    // An address at a provider of personal mail, or a kind of account that has no organisation:
    // no directory is named, and whoever signs in on Microsoft's page says which it is.
    let mut a = app(false);
    a.services.microsoft_client_id = APP_ID.into();
    sign_ins(&mut a, None);
    let made = links(&mut a);
    let mut h = open(a, json!({"email": "noor@example.org"}));
    connect_manually(&mut h);
    click(&mut h, Kind::OutlookCom.title());
    let said = h.state_mut().execute("ui.copyAdminMessage", json!({})).unwrap();
    assert!(said["url"].as_str().unwrap().starts_with("https://login.example.org/organizations/v2.0/adminconsent"));
    assert_eq!(*made.lock().unwrap(), [String::new()]);
    // A build that can make no such address says so on the page.
    h.state_mut().services.admin_consent_link = None;
    assert!(h.state_mut().execute("ui.copyAdminMessage", json!({})).unwrap_err().contains("can't make a link"));
    press_step(&mut h, COPY);
    assert!(says(&h, "This build can't make a link for an administrator.") && page(&h) == Some(Page::SignIn));
}

/// (2, continued) "Try again" starts the sign-in afresh; "Not now" closes the wizard and leaves
/// nothing behind: no account, nobody waiting in the browser, nothing asked of anybody.
#[test]
fn try_again_signs_in_afresh_and_not_now_leaves_nothing_behind() {
    let before = |h: &Harness<'static, MailApp>| (h.state().session.mailbox.accounts.len(), h.state().session.rev());
    let mut signed = None;
    let mut h = work_account(|a| signed = Some(sign_ins(a, Some(Err(NEEDS_APPROVAL.into())))));
    let signed = signed.unwrap();
    assert_eq!((page(&h), signed.lock().unwrap().len()), (Some(Page::Approval), 1));
    let was = before(&h);
    // Try again, and Microsoft still says no: asked once more, and the page is back.
    click(&mut h, "Try again");
    assert_eq!((page(&h), signed.lock().unwrap().len(), before(&h)), (Some(Page::Approval), 2, was));
    // Try again once an administrator has approved: the sign-in goes through.
    sign_ins(h.state_mut(), Some(Ok(signed_in_at_work())));
    click(&mut h, "Try again");
    h.run_steps(4);
    assert_eq!((page(&h), addresses(&h)), (Some(Page::Done), vec!["noor@example.org".to_string()]));

    // Not now, from the page Microsoft's answer brought up.
    let mut h = work_account(|a| {
        sign_ins(a, Some(Err(NEEDS_APPROVAL.into())));
    });
    let was = before(&h);
    click(&mut h, "Not now");
    assert_eq!((page(&h), before(&h), addresses(&h)), (None, was, vec![String::new()]));
    assert!(!h.state().signing_in() && !h.state().approving() && h.state().dialog.is_none());

    // Not now while a sign-in still waits behind the page, and while Microsoft's approval page
    // is open: both are given up, and the host is told to stop.
    for approving in [false, true] {
        let (mut held, mut asked) = (None, None);
        let mut h = work_account(|a| {
            held = Some(sign_ins_held(a));
            asked = Some(approvals(a, None));
        });
        let ((signed, _), asked) = (held.unwrap(), asked.unwrap());
        let was = before(&h);
        click_kind(&mut h, "signInApprovalHelp", 0);
        if approving {
            press_step(&mut h, ADMINISTRATOR);
            assert!(h.state().approving() && !h.state().signing_in());
            // (Escape and the window's own close button are "not now" too.)
            press(&mut h, Key::Escape);
        } else {
            assert!(h.state().signing_in());
            click(&mut h, "Not now");
        }
        assert_eq!((page(&h), before(&h), addresses(&h)), (None, was, vec![String::new()]), "{approving}");
        assert!(!h.state().signing_in() && !h.state().approving(), "{approving}");
        let flags: Vec<Arc<AtomicBool>> = signed.lock().unwrap().iter().map(|(.., stop)| stop.clone()).collect();
        assert!(stopped(&flags), "{approving}");
        let flags: Vec<Arc<AtomicBool>> = asked.lock().unwrap().iter().map(|(.., stop)| stop.clone()).collect();
        assert_eq!((flags.len(), flags.iter().all(|flag| flag.load(Ordering::Relaxed))), (usize::from(approving), true));
        // The next wizard starts clean.
        h.state_mut().execute("ui.dialog", json!({"name": "account"})).unwrap();
        h.run_steps(3);
        assert_eq!((page(&h), h.state().signing_in(), h.state().approving()), (Some(Page::Address), false, false));
    }

    // A script that closes the dialog while Microsoft's approval page is open doesn't tell it:
    // the wait is given up all the same.
    let mut asked = None;
    let mut h = work_account(|a| {
        sign_ins(a, Some(Err(NEEDS_APPROVAL.into())));
        asked = Some(approvals(a, None));
    });
    press_step(&mut h, ADMINISTRATOR);
    assert!(h.state().approving());
    h.state_mut().execute("ui.closeDialog", json!({})).unwrap();
    h.run_steps(3);
    assert!(!h.state().approving() && stopped(&[asked.unwrap().lock().unwrap()[0].3.clone()]) && addresses(&h) == [""]);
}

/// (2, continued) The window's own close button leaves nothing behind either, as "Not now" and
/// Escape: the sign-in that waits, and Microsoft's approval page, are given up at once, and the
/// host is told to stop.
#[test]
fn the_close_button_leaves_nothing_behind() {
    for approving in [false, true] {
        let (mut held, mut asked) = (None, None);
        let mut h = work_account(|a| {
            held = Some(sign_ins_held(a));
            asked = Some(approvals(a, None));
        });
        let ((signed, _), asked) = (held.unwrap(), asked.unwrap());
        click_kind(&mut h, "signInApprovalHelp", 0);
        if approving {
            press_step(&mut h, ADMINISTRATOR);
        }
        assert_eq!((h.state().signing_in(), h.state().approving()), (!approving, approving));
        h.get_by_label("Close window").click();
        h.run_steps(3);
        assert!(h.state().dialog.is_none(), "{approving}");
        assert!(!h.state().signing_in() && !h.state().approving(), "{approving}");
        let flags: Vec<Arc<AtomicBool>> = signed.lock().unwrap().iter().map(|(.., stop)| stop.clone()).collect();
        assert!(stopped(&flags), "{approving}");
        let flags: Vec<Arc<AtomicBool>> = asked.lock().unwrap().iter().map(|(.., stop)| stop.clone()).collect();
        assert_eq!((flags.len(), flags.iter().all(|flag| flag.load(Ordering::Relaxed))), (usize::from(approving), true), "{approving}");
    }
    // The page of an account that exists, with Microsoft's approval page open from it: the
    // close button gives that up too.
    let mut a = app(true);
    a.services.microsoft_client_id = APP_ID.into();
    let work = json!({"email": "ada@lumen.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "outlook.office365.com"}});
    let work = a.execute("account.add", work).unwrap()["id"].as_u64().unwrap();
    let asked = approvals(&mut a, None);
    let mut h = window(a, (1280.0, 800.0));
    h.state_mut().execute("ui.approvalHelp", json!({"account": work})).unwrap();
    h.run_steps(3);
    press_step(&mut h, ADMINISTRATOR);
    assert!(h.state().approving());
    h.get_by_label("Close window").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none() && !h.state().approving());
    assert!(stopped(&[asked.lock().unwrap()[0].3.clone()]));
}

/// (3) The other ways a sign-in ends without tokens: the wizard says which, in a sentence of
/// its own, with "Try again" and the way to the page about an approval.
#[test]
fn a_closed_tab_a_timeout_a_no_and_a_missing_browser_each_say_so() {
    for (how, title, said) in [
        (
            SIGN_IN_TIMED_OUT,
            "Nothing came back from Microsoft's sign-in page",
            "The page was closed, or five minutes went by before the sign-in was finished.",
        ),
        (SIGN_IN_DECLINED, "Microsoft's page came back without signing you in", "You came back from Microsoft's page without having signed in"),
        (NO_BROWSER, "MailCraft couldn't open your browser", "none could be started on this computer"),
    ] {
        let mut signed = None;
        let mut h = work_account(|a| signed = Some(sign_ins(a, Some(Err(how.into())))));
        let signed = signed.unwrap();
        assert_eq!((page(&h), wizard(&h).map(|w| w.waiting), h.state().signing_in()), (Some(Page::SignIn), Some(false), false), "{how}");
        assert!(says(&h, title) && says(&h, said) && says(&h, "noor@example.org"), "{how}");
        // (Said once, in the page's words: not a second time as an error.)
        assert_eq!((wizard(&h).map(|w| w.error.clone()), addresses(&h)), (Some(String::new()), vec![String::new()]), "{how}");
        // A page that was closed or said no may have been the one that asks for an administrator
        // (Microsoft sends the same no for the way back from that page): what to do about an
        // approval is on this page itself then, under what happened. A browser that never
        // opened showed no such page: there, only the way to the page about it.
        let may_have_asked = how != NO_BROWSER;
        assert_eq!(says(&h, "If that page said you need admin approval"), may_have_asked, "{how}");
        assert_eq!((says(&h, ADMINISTRATOR), says(&h, COPY)), (may_have_asked, may_have_asked), "{how}");
        assert_eq!(at(&h, "approvalAdministrator", 0).is_some(), may_have_asked, "{how}");
        assert_eq!(says(&h, "My organisation has to approve MailCraft first: what to do"), !may_have_asked, "{how}");
        assert!(at(&h, "signInTryAgain", 0).is_some() && at(&h, "signInApprovalHelp", 0).is_some(), "{how}");
        if may_have_asked {
            // The message can be copied from here, and the page stays.
            links(h.state_mut());
            press_step(&mut h, COPY);
            assert!(status(&h).contains("clipboard") && page(&h) == Some(Page::SignIn) && says(&h, title), "{how}");
        }
        // Try again: the sign-in once more.
        click(&mut h, "Try again");
        assert_eq!((page(&h), signed.lock().unwrap().len()), (Some(Page::SignIn), 2), "{how}");
        // The way to the page about an approval, where Microsoft itself has said nothing of one.
        click_kind(&mut h, "signInApprovalHelp", 0);
        assert_eq!((page(&h), wizard(&h).map(|w| w.approval.said)), (Some(Page::Approval), Some(false)), "{how}");
        assert!(says(&h, "Your organisation has to approve MailCraft first") && !says(&h, "Microsoft's page said so"), "{how}");
        // And from there the sign-in again, which this time goes through.
        sign_ins(h.state_mut(), Some(Ok(signed_in_at_work())));
        click(&mut h, "Try again");
        h.run_steps(4);
        assert_eq!((page(&h), addresses(&h)), (Some(Page::Done), vec!["noor@example.org".to_string()]), "{how}");
    }

    // Anything else is said in Microsoft's words, as it was, with the same two ways on.
    let h = work_account(|a| {
        sign_ins(a, Some(Err("AADSTS53003: Access has been blocked by Conditional Access policies.".into())));
    });
    assert!(page(&h) == Some(Page::SignIn) && says(&h, "Signing in didn't work") && says(&h, "AADSTS53003: Access has been blocked"));
    assert!(at(&h, "signInApprovalHelp", 0).is_some() && h.query_by_role_and_label(Role::Button, "Set up manually").is_some());
    // What kept an account from being added after a sign-in is not a sign-in that ended: no such way.
    let mut h = work_account(|a| {
        sign_ins(a, Some(Ok(signed_in_at_work())));
        a.execute("account.add", json!({"email": "noor@example.org"})).unwrap();
    });
    assert!(page(&h) == Some(Page::SignIn) && says(&h, "already set up") && at(&h, "signInApprovalHelp", 0).is_none());
    click(&mut h, "Set up manually");
    assert_eq!(page(&h), Some(Page::Choose));
}

/// (6) Everything is a command: the approval help, the message, the administrator's approval
/// and "try again" over the control channel, without a click; and none of them minds junk.
#[test]
fn the_approval_help_is_reached_by_commands_without_clicking() {
    for id in ["ui.approvalHelp", "ui.copyAdminMessage", "ui.adminConsent", "ui.signInAgain"] {
        assert!(crate::UI_COMMANDS.iter().any(|command| command.0 == id), "{id}");
    }
    // Nothing to help with: no account signs in with Microsoft, and none is being set up to.
    let mut plain = app(true);
    for id in ["ui.approvalHelp", "ui.copyAdminMessage", "ui.adminConsent", "ui.signInAgain"] {
        assert!(plain.enabled(id) && plain.execute(id, json!({})).unwrap_err().contains("Microsoft"), "{id}");
        assert!(plain.dialog.is_none());
    }
    // Nor while the wizard is not at a sign-in.
    let mut h = open(app(false), json!({}));
    assert!(h.state_mut().execute("ui.approvalHelp", json!({})).unwrap_err().contains("Microsoft"));
    assert_eq!(page(&h), Some(Page::Address));

    let (mut held, mut asked) = (None, None);
    let mut h = work_account(|a| {
        held = Some(sign_ins_held(a));
        asked = Some(approvals(a, None));
        links(a);
    });
    let ((signed, _), asked) = (held.unwrap(), asked.unwrap());
    let run = |h: &mut Harness<'static, MailApp>, id: &str| {
        let answer = h.state_mut().execute(id, json!({}));
        h.run_steps(3);
        answer
    };
    // Open the help: the page, and the sign-in goes on waiting.
    assert_eq!(run(&mut h, "ui.approvalHelp").unwrap(), json!({"dialog": "account", "page": "approval", "account": 0}));
    assert_eq!((page(&h), h.state().signing_in()), (Some(Page::Approval), true));
    assert_eq!(run(&mut h, "ui.approvalHelp").unwrap()["page"], "approval");
    // Copy the message.
    let said = run(&mut h, "ui.copyAdminMessage").unwrap();
    assert!(said["text"].as_str().unwrap().contains(said["url"].as_str().unwrap()) && said["account"] == 0);
    // Start the administrator's approval: the sign-in gives way to it.
    assert_eq!(run(&mut h, "ui.adminConsent").unwrap(), json!({"pending": "adminConsent", "account": 0}));
    assert!(h.state().approving() && !h.state().signing_in() && wizard(&h).is_some_and(|w| w.approval.approving));
    assert_eq!((asked.lock().unwrap().len(), stopped(&[signed.lock().unwrap()[0].3.clone()])), (1, true));
    // One page of Microsoft's at a time.
    assert!(run(&mut h, "ui.adminConsent").unwrap_err().contains("browser is waiting"));
    // Cancel Sign-In gives the approval up, and the page says what there is to do again.
    assert!(h.state().enabled("ui.cancelSignIn"));
    assert_eq!(run(&mut h, "ui.cancelSignIn").unwrap(), json!({"cancelled": 0}));
    assert!(!h.state().approving() && !h.state().enabled("ui.cancelSignIn") && stopped(&[asked.lock().unwrap()[0].3.clone()]));
    assert_eq!((page(&h), wizard(&h).map(|w| w.approval.approving)), (Some(Page::Approval), Some(false)));
    // Try again: the sign-in afresh.
    assert_eq!(run(&mut h, "ui.signInAgain").unwrap(), json!({"pending": "signIn", "account": 0}));
    assert_eq!((page(&h), h.state().signing_in(), signed.lock().unwrap().len()), (Some(Page::SignIn), true, 2));

    // Junk for parameters, in every state of the wizard: nothing falls over, and no account comes of it.
    let junk = "\u{0}\u{202e}é\n".repeat(5_000);
    for odd in
        [json!(null), json!([]), json!("x"), json!({"account": "x"}), json!({"account": 999_999}), json!({"account": -1}), json!({"account": junk})]
    {
        for id in ["ui.approvalHelp", "ui.copyAdminMessage", "ui.adminConsent", "ui.signInAgain", "ui.cancelSignIn"] {
            let _ = h.state_mut().execute(id, odd.clone());
            h.run_steps(2);
        }
    }
    assert_eq!(addresses(&h), [""]);
    press(&mut h, Key::Escape);
    assert!(page(&h).is_none() && !h.state().signing_in() && !h.state().approving());
}

/// (5) The same help for an account that exists: a Send/Receive the owner asked for that is
/// refused for want of an approval brings the same page up, as a dialog; and what the folder
/// pane says of Outlook's Favorites that wait on an administrator leads to it as well.
#[test]
fn an_account_that_exists_gets_the_same_page_when_its_sign_in_is_refused_for_approval() {
    let mut a = app(true);
    a.services.microsoft_client_id = APP_ID.into();
    let work = json!({"email": "ada@lumen.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "outlook.office365.com"}});
    let work = a.execute("account.add", work).unwrap()["id"].as_u64().unwrap();
    let other =
        a.execute("account.add", json!({"email": "noor@lumen.example", "incoming": {"host": "imap.lumen.example"}})).unwrap()["id"].as_u64().unwrap();
    let accounts = a.session.mailbox.accounts.len();
    let signed = sign_ins(&mut a, Some(Err(NEEDS_APPROVAL.into())));
    let synced = syncs(&mut a);
    let asked = approvals(&mut a, None);
    let made = links(&mut a);
    let mut h = window(a, (1280.0, 800.0));

    // A Send/Receive nobody asked for is refused the same way: said in the status bar and the
    // problems report, and no dialog is put in front of anybody.
    assert_eq!(h.state_mut().execute("ui.sync", json!({"account": work, "quiet": true})).unwrap()["pending"], "signIn");
    h.run_steps(4);
    assert!(h.state().dialog.is_none() && status(&h) == NEEDS_APPROVAL && h.state().has_sync_problems());
    // One the owner asked for: the page that says what to do next comes up, for that account.
    assert_eq!(h.state_mut().execute("ui.sync", json!({"account": work})).unwrap()["pending"], "signIn");
    h.run_steps(4);
    assert_eq!((page(&h), wizard(&h).map(|w| (w.existing, w.title()))), (Some(Page::Approval), Some((work, "Approval Needed".to_string()))));
    for said in ["Your organisation has to approve MailCraft first", "ada@lumen.example", "What to do next", ADMINISTRATOR, COPY, "Not now"] {
        assert!(says(&h, said), "{said}");
    }
    // (It is set up already: nothing is said of steps to come.)
    assert!(at(&h, "approvalAdministrator", work).is_some());

    // The message: its directory is not known (no sign-in of this run named one), so the
    // administrator's own sign-in says which organisation it is.
    press_step(&mut h, COPY);
    assert_eq!((made.lock().unwrap().clone(), status(&h).contains("clipboard")), (vec![String::new()], true));
    // Not now: the dialog goes, and the account stays as it was.
    click(&mut h, "Not now");
    assert_eq!((page(&h), h.state().session.mailbox.accounts.len()), (None, accounts));
    assert!(h.state().session.mailbox.account(work).is_some());

    // The command opens it too, for the account named or the first that signs in with
    // Microsoft; not for one that signs in with a password.
    assert!(h.state_mut().execute("ui.approvalHelp", json!({"account": other})).unwrap_err().contains("Microsoft"));
    assert!(h.state_mut().execute("ui.dialog", json!({"name": "account", "id": other, "page": "approval"})).is_err());
    assert_eq!(h.state_mut().execute("ui.approvalHelp", json!({})).unwrap(), json!({"dialog": "account", "page": "approval", "account": work}));
    h.run_steps(3);
    assert_eq!((page(&h), wizard(&h).map(|w| w.existing)), (Some(Page::Approval), Some(work)));
    // "I'm an administrator", and Microsoft says the account is none: said on the page and in
    // the status bar.
    approvals(h.state_mut(), Some(AdminConsentAnswer::NotAdministrator("AADSTS90094: Administrator consent is required.".into())));
    press_step(&mut h, ADMINISTRATOR);
    h.run_steps(4);
    assert!(
        page(&h) == Some(Page::Approval)
            && says(&h, "is not an administrator of your organisation")
            && status(&h).contains("is not an administrator")
    );
    assert_eq!(asked.lock().unwrap().len(), 0, "(the first stand-in was never asked)");
    // An administrator approves: the dialog goes, and the account signs in again by itself;
    // this time Microsoft lets it, and its Send/Receive starts with the token.
    let approved = approvals(h.state_mut(), Some(AdminConsentAnswer::Approved));
    let before = signed.lock().unwrap().len();
    let signed = sign_ins(h.state_mut(), Some(Ok(signed_in_at_work())));
    press_step(&mut h, ADMINISTRATOR);
    h.run_steps(6);
    {
        let approved = approved.lock().unwrap();
        assert_eq!((approved.len(), approved[0].0, approved[0].1.as_str(), approved[0].2.as_str()), (1, work, "", APP_ID));
    }
    assert_eq!((page(&h), signed.lock().unwrap().len(), before), (None, 1, 2));
    {
        let synced = synced.lock().unwrap();
        assert_eq!((synced.len(), synced[0].0.id, synced[0].1.as_str()), (1, work, "tok-1"));
    }
    assert!(h.state().syncing() && !h.state().approving());

    // "Try again" from the dialog is the account's Send/Receive, and closes it.
    let mut a = app(true);
    let work = json!({"email": "ada@lumen.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "outlook.office365.com"}});
    let work = a.execute("account.add", work).unwrap()["id"].as_u64().unwrap();
    let (signed, _) = sign_ins_held(&mut a);
    syncs(&mut a);
    let mut h = window(a, (1280.0, 800.0));
    h.state_mut().execute("ui.approvalHelp", json!({"account": work})).unwrap();
    h.run_steps(3);
    click(&mut h, "Try again");
    assert_eq!((page(&h), h.state().signing_in()), (None, true));
    // (For its mail, not for Microsoft Graph or the Favorites.)
    let asked_for: Vec<(bool, bool)> = signed.lock().unwrap().iter().map(|(_, graph, favorites, _)| (*graph, *favorites)).collect();
    assert_eq!(asked_for, [(false, false)]);
    h.state_mut().execute("ui.cancelSignIn", json!({})).unwrap();

    // Outlook's Favorites that wait on an administrator: the line under the Favorites heading
    // offers the message and the page; "try again" from there asks for the Favorites again.
    let _ = h.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    h.state_mut().execute("ui.favorites", json!({"account": work, "answer": {"outcome": "approval"}})).unwrap();
    h.run_steps(3);
    let state = h.state_mut().execute("ui.favorites", json!({"account": work})).unwrap();
    assert_eq!((state["line"].clone(), state["steps"].clone()), (json!("approval"), json!(["copyMessage", "help"])));
    let (copy, help) =
        (at(&h, "favoritesCopyMessage", work).expect("the message to copy"), at(&h, "favoritesHelp", work).expect("the way to the page"));
    assert!(help.min.y >= copy.max.y - 0.5);
    links(h.state_mut());
    click_at(&mut h, copy.center());
    assert!(status(&h).contains("The message is on the clipboard") && h.state().dialog.is_none());
    click_at(&mut h, help.center());
    assert_eq!((page(&h), wizard(&h).map(|w| w.existing)), (Some(Page::Approval), Some(work)));
    click(&mut h, "Try again");
    let asked_for: Vec<(bool, bool)> = signed.lock().unwrap().iter().map(|(_, graph, favorites, _)| (*graph, *favorites)).collect();
    assert_eq!((page(&h), asked_for), (None, vec![(false, false), (true, true)]));
}
