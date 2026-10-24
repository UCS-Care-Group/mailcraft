// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! About ▸ Contributors: everyone who contributed to MailCraft, by GitHub username always, and by
//! a display or real name only when they chose one for themselves (`docs/contributors.md`,
//! craftrules standards/contributors.md). The tables are baked into the binary by `build.rs`
//! from `contributors/`; nothing is read at run time.
//!
//! About ▸ Testers thanks the testers who said yes to being credited, from
//! `contributors/testers.toml`, baked in the same way.
//!
//! There is no About ▸ Models: the owner's commits carry no AI attribution, so it would be empty.

use std::cmp::Ordering;

use egui::RichText;

use crate::theme::Tokens;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contributor {
    pub login: &'static str,
    pub display_name: Option<&'static str>,
    pub real_name: Option<&'static str>,
    pub prs: u64,
    pub commits: u64,
    pub lines_added: u64,
    pub lines_deleted: u64,
    pub binary_added: u64,
    pub binary_deleted: u64,
    /// ISO 8601 UTC; empty for someone credited only through a merged PR.
    pub first_commit: &'static str,
    pub last_commit: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/credits.rs"));

/// A tester who said yes to being credited, in About ▸ Testers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tester {
    pub login: &'static str,
    pub display_name: Option<&'static str>,
    /// The day they became a tester, `YYYY-MM-DD`; empty when not known.
    pub since: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/testers.rs"));

/// Where to sign up to test: the roster issue of the repository the build came from.
pub const TESTERS_SIGN_UP: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/issues/11");

impl Tester {
    /// The display name they chose, else `@username`.
    pub fn name(&self) -> String {
        self.display_name.map_or_else(|| format!("@{}", self.login), str::to_string)
    }

    /// The tester's GitHub profile.
    pub fn url(&self) -> String {
        format!("https://github.com/{}", self.login)
    }
}

/// Which name to show. Display and real names fall back to the `@username` when not given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NameMode {
    #[default]
    Username,
    DisplayName,
    RealName,
}

impl NameMode {
    pub const ALL: [NameMode; 3] = [NameMode::Username, NameMode::DisplayName, NameMode::RealName];

    pub fn label(self) -> &'static str {
        match self {
            NameMode::Username => "Username",
            NameMode::DisplayName => "Display name",
            NameMode::RealName => "Real name",
        }
    }

    /// The next in the toggle: Username → Display name → Real name → Username.
    pub fn next(self) -> NameMode {
        match self {
            NameMode::Username => NameMode::DisplayName,
            NameMode::DisplayName => NameMode::RealName,
            NameMode::RealName => NameMode::Username,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Prs,
    Commits,
    LinesAdded,
    LinesDeleted,
    LinesDelta,
    BinaryAdded,
    BinaryDeleted,
    #[default]
    FirstCommit,
    LastCommit,
}

impl SortKey {
    pub const ALL: [SortKey; 10] = [
        SortKey::Name,
        SortKey::Prs,
        SortKey::Commits,
        SortKey::LinesAdded,
        SortKey::LinesDeleted,
        SortKey::LinesDelta,
        SortKey::BinaryAdded,
        SortKey::BinaryDeleted,
        SortKey::FirstCommit,
        SortKey::LastCommit,
    ];

    /// Menu label and table header.
    pub fn label(self) -> (&'static str, &'static str) {
        match self {
            SortKey::Name => ("Name (A–Z)", "Name"),
            SortKey::Prs => ("Merged PRs", "PRs"),
            SortKey::Commits => ("Commits", "Commits"),
            SortKey::LinesAdded => ("Lines added", "+Lines"),
            SortKey::LinesDeleted => ("Lines deleted", "−Lines"),
            SortKey::LinesDelta => ("Line delta", "ΔLines"),
            SortKey::BinaryAdded => ("Binary assets added", "+Bin"),
            SortKey::BinaryDeleted => ("Binary assets removed", "−Bin"),
            SortKey::FirstCommit => ("First commit", "First"),
            SortKey::LastCommit => ("Last commit", "Last"),
        }
    }

    /// Names and dates read naturally oldest/A first; counts read biggest first.
    pub fn default_ascending(self) -> bool {
        matches!(self, SortKey::Name | SortKey::FirstCommit | SortKey::LastCommit)
    }
}

impl Contributor {
    pub fn name(&self, mode: NameMode) -> String {
        let chosen = match mode {
            NameMode::Username => None,
            NameMode::DisplayName => self.display_name,
            NameMode::RealName => self.real_name,
        };
        chosen.map_or_else(|| format!("@{}", self.login), str::to_string)
    }

    pub fn lines_delta(&self) -> i128 {
        i128::from(self.lines_added) - i128::from(self.lines_deleted)
    }

    /// The contributor's GitHub profile.
    pub fn url(&self) -> String {
        format!("https://github.com/{}", self.login)
    }

    /// One line with everything we know, for the tooltip.
    pub fn summary(&self) -> String {
        crate::i18n::fmt(
            tl!(
                "@{login}: {prs} merged PRs, {commits} commits, +{added} / −{deleted} lines (Δ {delta}), +{binAdded} / −{binDeleted} binary assets, {first} – {last}"
            ),
            &[
                ("login", self.login),
                ("prs", &group(self.prs)),
                ("commits", &group(self.commits)),
                ("added", &group(self.lines_added)),
                ("deleted", &group(self.lines_deleted)),
                ("delta", &signed(self.lines_delta())),
                ("binAdded", &group(self.binary_added)),
                ("binDeleted", &group(self.binary_deleted)),
                ("first", day(self.first_commit)),
                ("last", day(self.last_commit)),
            ],
        )
    }
}

/// Alphabetical key: case-insensitive, ignoring the `@` of usernames.
fn name_key(name: &str) -> String {
    name.trim_start_matches('@').to_lowercase()
}

fn compare(a: &Contributor, b: &Contributor, mode: NameMode, key: SortKey) -> Ordering {
    match key {
        SortKey::Name => name_key(&a.name(mode)).cmp(&name_key(&b.name(mode))),
        SortKey::Prs => a.prs.cmp(&b.prs),
        SortKey::Commits => a.commits.cmp(&b.commits),
        SortKey::LinesAdded => a.lines_added.cmp(&b.lines_added),
        SortKey::LinesDeleted => a.lines_deleted.cmp(&b.lines_deleted),
        SortKey::LinesDelta => a.lines_delta().cmp(&b.lines_delta()),
        SortKey::BinaryAdded => a.binary_added.cmp(&b.binary_added),
        SortKey::BinaryDeleted => a.binary_deleted.cmp(&b.binary_deleted),
        // Someone credited only through a merged PR (no commit date) sorts after everyone with one.
        SortKey::FirstCommit => dated(a.first_commit).cmp(&dated(b.first_commit)),
        SortKey::LastCommit => dated(a.last_commit).cmp(&dated(b.last_commit)),
    }
}

fn dated(d: &str) -> (bool, &str) {
    (d.is_empty(), d)
}

/// Contributors in display order. Ties fall back to the name (A–Z) so the order is stable.
pub fn sorted(list: &[Contributor], mode: NameMode, key: SortKey, ascending: bool) -> Vec<&Contributor> {
    let mut v: Vec<&Contributor> = list.iter().collect();
    v.sort_by(|a, b| {
        let o = compare(a, b, mode, key);
        let o = if ascending { o } else { o.reverse() };
        o.then_with(|| compare(a, b, mode, SortKey::Name)).then_with(|| a.login.cmp(b.login))
    });
    v
}

/// `1234567` as `1,234,567`.
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn signed(n: i128) -> String {
    let g = group(u64::try_from(n.unsigned_abs()).unwrap_or(u64::MAX));
    if n < 0 { format!("−{g}") } else { format!("+{g}") }
}

/// The day of an ISO 8601 date; a dash for none.
fn day(iso: &str) -> &str {
    if iso.is_empty() { "—" } else { iso.get(..10).unwrap_or(iso) }
}

/// How the list is shown; kept in egui's memory while the program runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct View {
    names: NameMode,
    key: SortKey,
    ascending: bool,
    table: bool,
}

impl Default for View {
    fn default() -> Self {
        View { names: NameMode::Username, key: SortKey::FirstCommit, ascending: true, table: false }
    }
}

/// A name that leads to the contributor's GitHub profile, with everything known in its tooltip.
fn name_link(ui: &mut egui::Ui, c: &Contributor, names: NameMode) {
    let url = c.url();
    if ui.link(c.name(names)).on_hover_text(format!("{}\n{url}", c.summary())).clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

/// About ▸ Contributors: a name toggle, a sort, and the list as a grab bag or a table.
pub fn contributors_ui(ui: &mut egui::Ui, t: &Tokens) {
    let id = egui::Id::unique("credits_view");
    let mut v = ui.data_mut(|d| d.get_temp::<View>(id)).unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        if ui.button(tl!(v.names.label())).on_hover_text(tl!("Show usernames, display names or real names")).clicked() {
            v.names = v.names.next();
        }
        ui.separator();
        ui.label(RichText::new(tl!("Sort by")).color(t.text_dim));
        egui::ComboBox::from_id_salt("credits_sort").selected_text(tl!(v.key.label().0)).show_ui(ui, |ui| {
            for k in SortKey::ALL {
                if ui.selectable_label(v.key == k, tl!(k.label().0)).clicked() {
                    v.key = k;
                    v.ascending = k.default_ascending();
                }
            }
        });
        if ui.button(if v.ascending { "▲" } else { "▼" }).on_hover_text(tl!("Reverse the order")).clicked() {
            v.ascending = !v.ascending;
        }
        ui.separator();
        if ui.selectable_label(!v.table, tl!("Grab bag")).clicked() {
            v.table = false;
        }
        if ui.selectable_label(v.table, tl!("Table")).clicked() {
            v.table = true;
        }
    });
    let list = sorted(CONTRIBUTORS, v.names, v.key, v.ascending);
    let summary = crate::i18n::fmt(
        tl!("{contributors} contributors · {commits} commits"),
        &[("contributors", &list.len().to_string()), ("commits", &group(TOTAL_COMMITS))],
    );
    ui.label(RichText::new(summary).color(t.text_dim));
    ui.separator();
    egui::ScrollArea::both().auto_shrink([false, true]).max_height(340.0).show(ui, |ui| {
        if list.is_empty() {
            ui.label(RichText::new(tl!("No contributor credits were built into this copy.")).color(t.text_dim));
        } else if v.table {
            table(ui, &list, &mut v, t);
        } else {
            ui.horizontal_wrapped(|ui| {
                for (i, c) in list.iter().enumerate() {
                    if i > 0 {
                        ui.label(RichText::new("·").color(t.text_dim));
                    }
                    name_link(ui, c, v.names);
                }
            });
        }
    });
    ui.label(RichText::new(tl!("Contributors choose their own names; see docs/contributors.md.")).small().color(t.text_dim));
    ui.data_mut(|d| d.insert_temp(id, v));
}

/// About ▸ Testers: a thank-you, and the testers' names flowing as text in the order they are
/// built in (the earliest tester first). Nothing to sort and no numbers.
pub fn testers_ui(ui: &mut egui::Ui, t: &Tokens) {
    ui.label(
        RichText::new(tl!("MailCraft is tested on real mail, by people who use it every day and tell us what goes wrong. Thank you.")).color(t.text),
    );
    ui.separator();
    egui::ScrollArea::vertical().auto_shrink([false, true]).max_height(340.0).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if TESTERS.is_empty() {
                ui.label(RichText::new(tl!("No testers credited yet.")).color(t.text_dim));
                if ui.link(tl!("Want to be one? Sign up in #11")).on_hover_text(TESTERS_SIGN_UP).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(TESTERS_SIGN_UP));
                }
                return;
            }
            for (i, tester) in TESTERS.iter().enumerate() {
                if i > 0 {
                    ui.label(RichText::new("·").color(t.text_dim));
                }
                let url = tester.url();
                let who = if tester.since.is_empty() {
                    format!("@{}", tester.login)
                } else {
                    crate::i18n::fmt(tl!("@{login}, testing since {since}"), &[("login", tester.login), ("since", tester.since)])
                };
                if ui.link(tester.name()).on_hover_text(format!("{who}\n{url}")).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            }
        });
    });
    ui.label(RichText::new(tl!("Testers are credited when they say so; see docs/contributors.md.")).small().color(t.text_dim));
}

fn table(ui: &mut egui::Ui, list: &[&Contributor], v: &mut View, t: &Tokens) {
    egui::Grid::new("credits_table").striped(true).num_columns(SortKey::ALL.len()).show(ui, |ui| {
        for k in SortKey::ALL {
            let arrow = if v.key == k { if v.ascending { " ▲" } else { " ▼" } } else { "" };
            let head = RichText::new(format!("{}{arrow}", tl!(k.label().1))).strong().color(t.text);
            if ui.add(egui::Button::new(head).frame(false)).on_hover_text(tl!(k.label().0)).clicked() {
                if v.key == k {
                    v.ascending = !v.ascending;
                } else {
                    v.key = k;
                    v.ascending = k.default_ascending();
                }
            }
        }
        ui.end_row();
        for c in list {
            name_link(ui, c, v.names);
            for cell in [
                group(c.prs),
                group(c.commits),
                group(c.lines_added),
                group(c.lines_deleted),
                signed(c.lines_delta()),
                group(c.binary_added),
                group(c.binary_deleted),
                day(c.first_commit).to_string(),
                day(c.last_commit).to_string(),
            ] {
                ui.label(RichText::new(cell).color(t.text));
            }
            ui.end_row();
        }
    });
}

#[cfg(test)]
#[path = "credits_data.rs"]
mod credits_data;

#[cfg(test)]
mod tests {
    use super::credits_data::{MAX_NAME_CHARS, parse, parse_testers, render, render_testers, valid_login};
    use super::*;

    fn c(
        login: &'static str,
        display: Option<&'static str>,
        real: Option<&'static str>,
        added: u64,
        deleted: u64,
        first: &'static str,
    ) -> Contributor {
        Contributor {
            login,
            display_name: display,
            real_name: real,
            prs: 0,
            commits: 1,
            lines_added: added,
            lines_deleted: deleted,
            binary_added: 0,
            binary_deleted: 0,
            first_commit: first,
            last_commit: first,
        }
    }

    fn people() -> [Contributor; 4] {
        [
            c("zed", None, None, 5, 50, "2026-01-02T00:00:00Z"),
            c("Bob", Some("Ann"), None, 10, 0, "2026-01-01T00:00:00Z"),
            c("alice", None, None, 7, 1, ""),
            c("carol", None, Some("Dana"), 1, 0, "2026-01-03T00:00:00Z"),
        ]
    }

    fn logins(v: &[&Contributor]) -> Vec<&'static str> {
        v.iter().map(|c| c.login).collect()
    }

    #[test]
    fn names_fall_back_to_the_username() {
        let p = c("bob", Some("Bobby"), None, 0, 0, "");
        assert_eq!(p.name(NameMode::Username), "@bob");
        assert_eq!(p.name(NameMode::DisplayName), "Bobby");
        assert_eq!(p.name(NameMode::RealName), "@bob");
        let mut m = NameMode::default();
        let order: Vec<NameMode> = (0..4)
            .map(|_| {
                let now = m;
                m = m.next();
                now
            })
            .collect();
        assert_eq!(order, [NameMode::Username, NameMode::DisplayName, NameMode::RealName, NameMode::Username]);
    }

    #[test]
    fn alphabetical_ignores_at_and_case() {
        let all = people();
        let v = sorted(&all, NameMode::Username, SortKey::Name, true);
        assert_eq!(logins(&v), ["alice", "Bob", "carol", "zed"]);
        let v = sorted(&all, NameMode::DisplayName, SortKey::Name, true);
        assert_eq!(logins(&v), ["alice", "Bob", "carol", "zed"]); // "Ann" sorts with the @usernames
        let v = sorted(&all, NameMode::RealName, SortKey::Name, false);
        assert_eq!(logins(&v), ["zed", "carol", "Bob", "alice"]); // "Dana" between bob and zed
    }

    #[test]
    fn counts_delta_and_dates() {
        let all = people();
        let v = sorted(&all, NameMode::Username, SortKey::LinesDelta, false);
        assert_eq!(logins(&v), ["Bob", "alice", "carol", "zed"]);
        let v = sorted(&all, NameMode::Username, SortKey::FirstCommit, true);
        assert_eq!(logins(&v), ["Bob", "zed", "carol", "alice"]); // undated last
        // Equal counts fall back to the name, A–Z, whichever way the count runs.
        let v = sorted(&all, NameMode::Username, SortKey::Commits, false);
        assert_eq!(logins(&v), ["alice", "Bob", "carol", "zed"]);
        for k in SortKey::ALL {
            assert_eq!(sorted(&all, NameMode::RealName, k, true).len(), 4);
        }
    }

    #[test]
    fn formatting() {
        assert_eq!(group(1234567), "1,234,567");
        assert_eq!(group(12), "12");
        assert_eq!(group(u64::MAX), "18,446,744,073,709,551,615");
        assert_eq!(signed(-1000), "−1,000");
        assert_eq!(signed(i128::from(u64::MAX) * -4), format!("−{}", group(u64::MAX)));
        assert_eq!(day("2026-10-07T18:45:23Z"), "2026-10-07");
        assert_eq!(day("2026"), "2026");
        assert_eq!(day(""), "—");
    }

    const JSON: &str = r#"{"schema": 1, "repo": "example/mailcraft", "commits": 9, "contributors": [
        {"login": "ann", "people_entry": true, "github_name": "Ann on GitHub", "prs": 2, "commits": 5,
         "lines_added": 100, "lines_deleted": 7, "binary_added": 1, "binary_deleted": 0,
         "first_commit": "2026-10-01T10:00:00Z", "last_commit": "2026-10-02T10:00:00Z"},
        {"login": "bo-b", "people_entry": false, "commits": 3, "first_commit": "2026-10-03T10:00:00Z"},
        {"login": "cy", "people_entry": true, "github_name": "Cy G", "prs": 1},
        {"login": "ANN", "commits": 99},
        {"login": "-bad"}, {"login": "a/../b"}, {"login": ""}, {"commits": 4}, 7,
        {"login": "dee", "people_entry": true, "first_commit": "<script>"}
    ]}"#;

    const PEOPLE: &str = r#"
        [people.ann]
        real_name = "  Ann Example  "
        sync_github_name = true
        [people.bo-b]
        real_name = "Not verified"
        [people.cy]
        display_name = "Cy"
        sync_github_name = true
        [people.dee]
        real_name = "Dee"
        display_name = "   "
        sync_github_name = false
    "#;

    #[test]
    fn the_files_are_read() {
        let got = parse(JSON, PEOPLE).unwrap();
        assert_eq!(got.total_commits, 9);
        let rows: Vec<(&str, Option<&str>, Option<&str>)> =
            got.contributors.iter().map(|r| (r.login.as_str(), r.display_name.as_deref(), r.real_name.as_deref())).collect();
        assert_eq!(
            rows,
            [
                // A synced GitHub name is the display name when none is chosen.
                ("ann", Some("Ann on GitHub"), Some("Ann Example")),
                // Names only from an entry the script verified.
                ("bo-b", None, None),
                // A chosen display name wins over the GitHub one.
                ("cy", Some("Cy"), None),
                // Blank is no name; the duplicate "ANN" and the impossible usernames are left out.
                ("dee", None, Some("Dee")),
            ]
        );
        let ann = &got.contributors[0];
        assert_eq!((ann.prs, ann.commits, ann.lines_added, ann.lines_deleted, ann.binary_added), (2, 5, 100, 7, 1));
        assert_eq!((ann.first_commit.as_str(), ann.last_commit.as_str()), ("2026-10-01T10:00:00Z", "2026-10-02T10:00:00Z"));
        assert_eq!(got.contributors[3].first_commit, "", "a date that isn't one is no date");
        // Rendered as Rust, every string a literal.
        let src = render(&got);
        assert!(src.starts_with("pub static TOTAL_COMMITS: u64 = 9;"));
        assert!(src.contains(r#"login: "ann", display_name: Some("Ann on GitHub"), real_name: Some("Ann Example")"#));
        assert_eq!(src.matches("Contributor {").count(), 4);
    }

    #[test]
    fn names_are_cleaned_and_clamped() {
        let json = r#"{"schema": 1, "contributors": [{"login": "x", "people_entry": true}]}"#;
        let long = "é".repeat(MAX_NAME_CHARS * 3);
        let people = format!("[people.x]\nreal_name = \"a\\u0000b\\nc\\\"\"\ndisplay_name = \"{long}\"\n");
        let got = parse(json, &people).unwrap();
        let x = &got.contributors[0];
        assert_eq!(x.real_name.as_deref(), Some("abc\""));
        assert_eq!(x.display_name.as_ref().map(|s| s.chars().count()), Some(MAX_NAME_CHARS));
        // A quote or backslash in a name stays inside its literal.
        assert!(render(&got).contains(r#"real_name: Some("abc\"")"#));
        assert!(valid_login("UCS-Care-Group") && !valid_login(&"a".repeat(40)) && !valid_login("a b"));
    }

    #[test]
    fn a_malformed_file_is_an_error_not_a_panic() {
        for (json, people) in [
            ("", ""),
            ("{", ""),
            ("[]", ""),
            (r#"{"schema": 2, "contributors": []}"#, ""),
            (r#"{"schema": "1", "contributors": []}"#, ""),
            (r#"{"schema": 1}"#, ""),
            (r#"{"schema": 1, "contributors": {}}"#, ""),
            (r#"{"schema": 1, "contributors": []}"#, "[people.x"),
            (r#"{"schema": 1, "contributors": []}"#, "people = 3"),
        ] {
            assert!(parse(json, people).is_err(), "{json:?} / {people:?}");
        }
        // Without names, everyone is still credited by username.
        assert_eq!(parse(r#"{"schema": 1, "contributors": [{"login": "x"}]}"#, "").unwrap().contributors.len(), 1);
        // Odd bytes anywhere never panic.
        for n in 0..JSON.len() {
            let cut = JSON.get(..n).unwrap_or("");
            let _ = parse(cut, PEOPLE);
            let _ = parse(JSON, PEOPLE.get(..n.min(PEOPLE.len())).unwrap_or(""));
        }
    }

    /// The repository's own files read cleanly, so a malformed one fails CI instead of quietly
    /// leaving the About window empty.
    #[test]
    fn the_repository_files_are_well_formed() {
        let got = parse(include_str!("../../../contributors/contributors.json"), include_str!("../../../contributors/people.toml")).unwrap();
        assert!(!got.contributors.is_empty());
        assert_eq!(got.contributors.len(), CONTRIBUTORS.len());
        assert!(CONTRIBUTORS.iter().all(|c| valid_login(c.login)));
        // testers.toml likewise, and nobody in it without a recorded yes.
        let testers = parse_testers(include_str!("../../../contributors/testers.toml")).unwrap();
        assert_eq!(testers.len(), TESTERS.len());
        assert!(TESTERS.iter().all(|t| valid_login(t.login)));
    }

    fn testers(src: &str) -> Vec<(String, Option<String>, String)> {
        parse_testers(src).unwrap().into_iter().map(|r| (r.login, r.display_name, r.since)).collect()
    }

    #[test]
    fn testers_are_read_as_hostile() {
        let long = "é".repeat(MAX_NAME_CHARS * 3);
        let src = format!(
            r#"
            [testers.ANN]
            display_name = "  Ann\u0000 Example\n "
            since = 2026-10-19
            consent = 11
            [testers.ann]
            since = 2026-01-01
            consent = 12
            [testers.-bad]
            consent = 11
            [testers."a b"]
            consent = 11
            [testers.{too_long}]
            consent = 11
            [testers.no-yes]
            since = 2026-10-01
            [testers.zero-yes]
            consent = 0
            [testers.text-yes]
            consent = "11"
            [testers.long]
            display_name = "{long}"
            since = "2026-10-02"
            consent = 11
            [testers.bad-day]
            since = "2026-02-30"
            consent = 11
            [testers.timed]
            since = 2026-10-03T10:00:00Z
            consent = 11
            [testers.odd]
            since = "<script>"
            display_name = 7
            consent = 11
            [testers.leap]
            since = "2024-02-29"
            consent = 11
            "#,
            too_long = "a".repeat(40),
        );
        let got = testers(&src);
        let logins: Vec<&str> = got.iter().map(|r| r.0.as_str()).collect();
        // Dated first, earliest first; undated after, by username whatever the case. The second
        // "ann", impossible usernames and entries without a yes are left out.
        assert_eq!(logins, ["leap", "long", "ANN", "bad-day", "odd", "timed"]);
        let ann = got.iter().find(|r| r.0 == "ANN").unwrap();
        assert_eq!((ann.1.as_deref(), ann.2.as_str()), (Some("Ann Example"), "2026-10-19"));
        let long = got.iter().find(|r| r.0 == "long").unwrap();
        assert_eq!(long.1.as_ref().map(|s| s.chars().count()), Some(MAX_NAME_CHARS));
        // A date that isn't a day is no date; a name that isn't text is no name.
        assert!(got.iter().filter(|r| ["bad-day", "timed", "odd"].contains(&r.0.as_str())).all(|r| r.2.is_empty()));
        assert_eq!(got.iter().find(|r| r.0 == "odd").unwrap().1, None);
        // Rendered as Rust, every string a literal, and the consent issue not in it.
        let src = render_testers(&parse_testers(&src).unwrap());
        assert!(src.starts_with("pub static TESTERS: &[Tester] = &["));
        assert!(src.contains(r#"Tester { login: "ANN", display_name: Some("Ann Example"), since: "2026-10-19" }"#));
        assert!(!src.contains("consent"));
        assert_eq!(src.matches("Tester {").count(), 6);
    }

    #[test]
    fn testers_ordering_and_names() {
        let got = testers(
            "[testers.zed]\nconsent = 1\n[testers.Bob]\nconsent = 1\n[testers.amy]\nsince = 2026-11-01\nconsent = 1\n[testers.cy]\nsince = 2026-10-01\nconsent = 1\n",
        );
        let logins: Vec<&str> = got.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(logins, ["cy", "amy", "Bob", "zed"]);
        let t = Tester { login: "bob", display_name: None, since: "" };
        assert_eq!((t.name(), t.url()), ("@bob".to_string(), "https://github.com/bob".to_string()));
        assert_eq!(Tester { display_name: Some("Bobby"), ..t }.name(), "Bobby");
        assert!(TESTERS_SIGN_UP.starts_with("https://github.com/") && TESTERS_SIGN_UP.ends_with("/issues/11"));
    }

    #[test]
    fn a_malformed_testers_file_is_an_error_not_a_panic() {
        for bad in ["[testers.x", "testers = 3", "testers = []"] {
            assert!(parse_testers(bad).is_err(), "{bad:?}");
        }
        assert!(testers("").is_empty());
        assert!(testers("[other]\nx = 1").is_empty());
        assert!(testers("[testers]\nx = 1\ny = \"z\"").is_empty());
        let src = "[testers.ann]\ndisplay_name = \"Ann\"\nsince = 2026-10-19\nconsent = 11\n";
        for n in 0..=src.len() {
            let _ = parse_testers(src.get(..n).unwrap_or(""));
        }
    }
}
