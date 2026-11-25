// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Sieve script checked here, before any server sees it (#9999): its syntax (RFC 5228 §8),
//! the commands, tests and tags it uses and the shape of their arguments, and whether each
//! extension it uses is named by its `require`. The server checks again (ManageSieve's
//! CHECKSCRIPT); this is the first look, with a line and column for each problem.
//!
//! [`check`] takes a whole script. [`check_block`] takes one block meant to sit inside MailCraft's
//! own script: it has no `require` (MailCraft writes those at the top, from what the block
//! [`Checked::needs`]), uses no extension MailCraft doesn't know, and nothing in it can reach out of
//! it (an `elsif` or `else` at its top would join the rule before it; a `}` too many, or a string
//! or comment left open, would swallow what follows).
//!
//! The extensions known are those of the table [`SPECS`] (RFC 5228's own commands and tests, and
//! fileinto, envelope, imap4flags 5232, vacation 5230 and 6131, body 5173, relational 5231,
//! subaddress 5233, copy 3894, reject and ereject 5429, include 6609, variables 5229, date and
//! index 5260, mailbox and metadata 5490, enotify 5435, editheader 5293, ihave 5463, environment
//! 5183, spamtest and virustest 5235, extlists 6134, duplicate 7352, special-use 8579, fcc 8580,
//! mime and its actions 5703, and regex). A script that requires one not known here has what it
//! adds unchecked: what isn't known is then a warning, not an error.
//!
//! **The script is hostile.** It is read by the bounded tokenizer ([`crate::sieve::lex_or_why`],
//! [`crate::sieve::MAX_SCRIPT`]) within the caps of the reader of the owner's script
//! ([`crate::cmd::sieve_elsewhere`]): how deep it nests, how many commands it has, how many
//! arguments one has and how many tests one list holds. Nothing of it runs, and nothing of it is
//! said as an instruction: a problem names a command, a tag or an extension of it, and where it is.

use std::ops::Range;

use mailcraft_i18n::Text;

use crate::sieve::{LexFault, Tok, lex_or_why};

/// How deep tests and blocks may nest (as [`crate::cmd::sieve_elsewhere`] reads them).
pub const MAX_DEPTH: usize = 16;
/// Commands in one script, all levels together.
pub const MAX_COMMANDS: usize = 2_000;
/// Arguments of one command or test, and tests in one list.
pub const MAX_ARGS: usize = 64;
/// Problems kept from one script.
pub const MAX_PROBLEMS: usize = 100;
/// The longest name (of a command, a tag, an extension) kept in a problem, in bytes.
const MAX_NAME: usize = 64;

/// What is wrong, or worth a warning, in a script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    /// It couldn't be read into tokens.
    Lex(LexFault),
    ExpectedCommand,
    ExpectedTest,
    /// Something other than a string in a `[ ]` list.
    ExpectedString,
    /// A list not followed by `,` or `]`.
    ListEnd,
    /// A test list not followed by `,` or `)`.
    TestListEnd,
    /// A command not ended by `;` (or a block): `at` is where it should be.
    MissingSemicolon,
    /// A `{` with no `}`.
    UnclosedBlock,
    /// A `}` with no `{`.
    StrayClose,
    TooDeep,
    TooManyCommands,
    TooManyArgs,
    UnknownCommand(String),
    UnknownTest(String),
    /// A tag (without its colon) the command or test has no use for.
    UnknownTag {
        tag: String,
        name: String,
    },
    TestAsCommand(String),
    CommandAsTest(String),
    /// What `name` (a command, a test, `:tag`, or a comparator) needs that no `require` names.
    NeedsRequire {
        name: String,
        ext: String,
    },
    /// A `require`d extension MailCraft doesn't know (a warning: what it adds isn't checked).
    UnknownExtension(String),
    /// An extension the server didn't say it has (a warning).
    ServerLacks(String),
    /// A `require` in a block: MailCraft writes them.
    RequireInBlock,
    /// A `require` after another command, or inside a block.
    RequireNotFirst,
    /// An `elsif` or `else` with no `if` before it.
    ElseWithoutIf(String),
    NeedsTest(String),
    NoTest(String),
    /// A command or test that needs a test list in brackets.
    NeedsTestList(String),
    /// One that takes one test, given a list in brackets.
    OneTest(String),
    NeedsBlock(String),
    NoBlock(String),
    TagTwice(String),
    TagClash {
        tag: String,
        other: String,
    },
    TagAfterPositional(String),
    /// A tag whose value is missing or of the wrong kind.
    TagValue {
        tag: String,
        wants: Wants,
    },
    /// A positional argument of the wrong kind.
    WrongArg {
        name: String,
        wants: Wants,
    },
    MissingArg(String),
    ExtraArg(String),
    /// A command or test that needs one of a group of tags (`size` needs `:over` or `:under`).
    NeedsTag {
        name: String,
        tags: String,
    },
    /// A relational match (RFC 5231) other than gt, ge, lt, le, eq and ne.
    BadRelation(String),
    UnknownComparator(String),
}

/// The kind of value an argument should be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wants {
    Str,
    /// A string, or a list of them.
    List,
    Num,
}

/// One problem: what, where (its bytes, on character boundaries, and the line and column it
/// starts at, both from 1, the column counted in characters), and whether it is only a warning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub fault: Fault,
    pub at: Range<usize>,
    pub line: usize,
    pub column: usize,
    pub warning: bool,
}

impl Problem {
    /// What it says, in English or in the interface language.
    pub fn what(&self) -> Text {
        match &self.fault {
            Fault::Lex(f) => match f {
                LexFault::TooLarge => said!("The script is larger than {mb} MB.", mb = crate::sieve::MAX_SCRIPT >> 20),
                LexFault::TooMany => said!("The script has too many parts to check."),
                LexFault::Comment => said!("This comment has no */ to end it."),
                LexFault::String => said!("This string has no closing quote."),
                LexFault::Text => said!("This text: has no line with a lone dot to end it."),
                LexFault::TextStart => said!("Only a comment may follow text: on its line."),
                LexFault::Tag => said!("A name must follow the colon."),
                LexFault::Number => said!("This number is too large."),
                LexFault::Char => said!("This character has no meaning here."),
            },
            Fault::ExpectedCommand => said!("A command was expected here."),
            Fault::ExpectedTest => said!("A test was expected here."),
            Fault::ExpectedString => said!("A list holds only strings."),
            Fault::ListEnd => said!("A comma or ] was expected here."),
            Fault::TestListEnd => said!("A comma or ) was expected here."),
            Fault::MissingSemicolon => said!("A semicolon is missing here."),
            Fault::UnclosedBlock => said!("This { has no } to close it."),
            Fault::StrayClose => said!("This } has no { before it."),
            Fault::TooDeep => said!("Nested more than {n} levels deep.", n = MAX_DEPTH),
            Fault::TooManyCommands => said!("More than {n} commands.", n = MAX_COMMANDS),
            Fault::TooManyArgs => said!("More than {n} arguments or tests.", n = MAX_ARGS),
            Fault::UnknownCommand(name) => said!("Unknown command {name}.", name),
            Fault::UnknownTest(name) => said!("Unknown test {name}.", name),
            Fault::UnknownTag { tag, name } => said!("{name} has no :{tag}.", name, tag),
            Fault::TestAsCommand(name) => said!("{name} is a test; a command belongs here.", name),
            Fault::CommandAsTest(name) => said!("{name} is a command; a test belongs here.", name),
            Fault::NeedsRequire { name, ext } => said!("{name} needs require \"{ext}\".", name, ext),
            Fault::UnknownExtension(ext) => said!("MailCraft doesn't know the extension {ext}; what it adds isn't checked.", ext),
            Fault::ServerLacks(ext) => said!("The server doesn't have the extension {ext}.", ext),
            Fault::RequireInBlock => said!("No require here: MailCraft writes it at the top of its script."),
            Fault::RequireNotFirst => said!("require goes before every other command."),
            Fault::ElseWithoutIf(name) => said!("{name} without an if before it.", name),
            Fault::NeedsTest(name) => said!("{name} needs a test.", name),
            Fault::NoTest(name) => said!("{name} takes no test.", name),
            Fault::NeedsTestList(name) => said!("{name} needs a list of tests in brackets.", name),
            Fault::OneTest(name) => said!("{name} takes one test, not a list.", name),
            Fault::NeedsBlock(name) => said!("{name} needs a block in braces.", name),
            Fault::NoBlock(name) => said!("{name} takes no block; end it with a semicolon.", name),
            Fault::TagTwice(tag) => said!(":{tag} is given twice.", tag),
            Fault::TagClash { tag, other } => said!(":{tag} can't go with :{other}.", tag, other),
            Fault::TagAfterPositional(tag) => said!(":{tag} must come before the other arguments.", tag),
            Fault::TagValue { tag, wants: Wants::Str } => said!(":{tag} needs a string after it.", tag),
            Fault::TagValue { tag, wants: Wants::List } => said!(":{tag} needs a string or a list after it.", tag),
            Fault::TagValue { tag, wants: Wants::Num } => said!(":{tag} needs a number after it.", tag),
            Fault::WrongArg { name, wants: Wants::Str } => said!("{name} needs a string here.", name),
            Fault::WrongArg { name, wants: Wants::List } => said!("{name} needs a string or a list here.", name),
            Fault::WrongArg { name, wants: Wants::Num } => said!("{name} needs a number here.", name),
            Fault::MissingArg(name) => said!("{name} is missing an argument.", name),
            Fault::ExtraArg(name) => said!("{name} has too many arguments.", name),
            Fault::NeedsTag { name, tags } => said!("{name} needs one of {tags}.", name, tags),
            Fault::BadRelation(value) => said!("\"{value}\" isn't a relation (gt, ge, lt, le, eq or ne).", value),
            Fault::UnknownComparator(name) => said!("Unknown comparator \"{name}\".", name),
        }
    }
}

/// A script (or block) with no errors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checked {
    /// The extensions its `require`s name, in their order (none for a block).
    pub requires: Vec<String>,
    /// The extensions what it uses needs, each once, in the order first used: what a block's
    /// `require` should name when it is put in a script.
    pub needs: Vec<String>,
    /// Its warnings, in the order of the script.
    pub warnings: Vec<Problem>,
}

/// Whether `script` is Sieve MailCraft can find nothing wrong with, and if not, its problems
/// (errors and warnings, in the order of the script; at least one is an error). `server` is the
/// extensions the server says it has (ManageSieve's `SIEVE` capability), when known: one the
/// script needs that the server lacks is a warning.
pub fn check(script: &str, server: Option<&[String]>) -> Result<Checked, Vec<Problem>> {
    run(script, server, false)
}

/// [`check`] for one block meant to sit inside MailCraft's script: it may not `require` (what it
/// needs is [`Checked::needs`]), nor use an extension MailCraft doesn't know, nor begin with an
/// `elsif` or `else`.
pub fn check_block(block: &str, server: Option<&[String]>) -> Result<Checked, Vec<Problem>> {
    run(block, server, true)
}

/// The extensions known here, besides any `comparator-<name>` of [`COMPARATORS`].
pub const EXTENSIONS: &[&str] = &[
    "fileinto",
    "envelope",
    "encoded-character",
    "imap4flags",
    "vacation",
    "vacation-seconds",
    "body",
    "relational",
    "subaddress",
    "copy",
    "reject",
    "ereject",
    "include",
    "variables",
    "date",
    "index",
    "mailbox",
    "mboxmetadata",
    "servermetadata",
    "enotify",
    "editheader",
    "ihave",
    "environment",
    "spamtest",
    "spamtestplus",
    "virustest",
    "extlists",
    "duplicate",
    "special-use",
    "fcc",
    "mime",
    "foreverypart",
    "replace",
    "enclose",
    "extracttext",
    "regex",
];

/// The comparators known (RFC 4790, 5051): the first two need no `require`.
pub const COMPARATORS: &[&str] = &["i;octet", "i;ascii-casemap", "i;ascii-numeric", "i;unicode-casemap"];

/// The extensions a command or test needs, or with `tag` (without its colon) the extensions
/// that tag needs on it, from the table: `None` for a name not known, and an empty list for one
/// of RFC 5228's own. (A comparator's extension is `comparator-` and its name.)
pub fn needs_of(name: &str, tag: Option<&str>) -> Option<&'static [&'static str]> {
    let spec = lookup(name)?;
    match tag {
        None => Some(spec.ext),
        Some(tag) => spec.tags.iter().flat_map(|g| g.iter()).find(|t| t.name == tag).map(|t| t.ext),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Control,
    Action,
    Test,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Takes {
    No,
    One,
    List,
}

/// A tag: what follows it (`None` for nothing), what it needs, and its group (tags of one
/// non-zero group exclude each other).
struct TagSpec {
    name: &'static str,
    value: Option<Wants>,
    ext: &'static [&'static str],
    group: u8,
}

/// A command or test of the table.
struct Spec {
    name: &'static str,
    kind: Kind,
    ext: &'static [&'static str],
    tags: &'static [&'static [TagSpec]],
    /// Its positional arguments, in order; the first `lead` of them, or the last `trail`, may be
    /// left out.
    pos: &'static [Wants],
    lead: usize,
    trail: usize,
    tests: Takes,
    block: bool,
    /// A group of tags one of which must be given (0 for none).
    needs_group: u8,
}

const fn tag(name: &'static str, value: Option<Wants>, ext: &'static [&'static str], group: u8) -> TagSpec {
    TagSpec { name, value, ext, group }
}

const S: Option<Wants> = Some(Wants::Str);
const L: Option<Wants> = Some(Wants::List);
const N: Option<Wants> = Some(Wants::Num);
const NONE: &[&str] = &[];

const G_MATCH: u8 = 1;
const G_PART: u8 = 2;
const G_BODY: u8 = 3;
const G_ZONE: u8 = 4;
const G_SIZE: u8 = 5;
const G_DAYS: u8 = 6;
const G_CASE: u8 = 7;
const G_FIRST: u8 = 8;
const G_QUOTE: u8 = 9;
const G_WHERE: u8 = 10;
const G_ID: u8 = 11;
const G_MIME: u8 = 12;

const COMPARATOR: &[TagSpec] = &[tag("comparator", S, NONE, 0)];
const MATCH: &[TagSpec] = &[
    tag("is", None, NONE, G_MATCH),
    tag("contains", None, NONE, G_MATCH),
    tag("matches", None, NONE, G_MATCH),
    tag("regex", None, &["regex"], G_MATCH),
    tag("value", S, &["relational"], G_MATCH),
    tag("count", S, &["relational"], G_MATCH),
    tag("list", None, &["extlists"], G_MATCH),
];
const ADDRESS_PART: &[TagSpec] = &[
    tag("all", None, NONE, G_PART),
    tag("localpart", None, NONE, G_PART),
    tag("domain", None, NONE, G_PART),
    tag("user", None, &["subaddress"], G_PART),
    tag("detail", None, &["subaddress"], G_PART),
];
const INDEX: &[TagSpec] = &[tag("index", N, &["index"], 0), tag("last", None, &["index"], 0)];
const MIME_ANY: &[TagSpec] = &[tag("mime", None, &["mime"], 0), tag("anychild", None, &["mime"], 0)];
const MIME_PARTS: &[TagSpec] = &[
    tag("type", None, &["mime"], G_MIME),
    tag("subtype", None, &["mime"], G_MIME),
    tag("contenttype", None, &["mime"], G_MIME),
    tag("param", L, &["mime"], G_MIME),
];
const BODY: &[TagSpec] = &[tag("raw", None, NONE, G_BODY), tag("text", None, NONE, G_BODY), tag("content", L, NONE, G_BODY)];
const DATE_ZONE: &[TagSpec] = &[tag("zone", S, NONE, G_ZONE), tag("originalzone", None, NONE, G_ZONE)];
const ZONE: &[TagSpec] = &[tag("zone", S, NONE, 0)];
const SIZE: &[TagSpec] = &[tag("over", None, NONE, G_SIZE), tag("under", None, NONE, G_SIZE)];
const COPY: &[TagSpec] = &[tag("copy", None, &["copy"], 0)];
const FLAGS: &[TagSpec] = &[tag("flags", L, &["imap4flags"], 0)];
const FILEINTO: &[TagSpec] = &[tag("create", None, &["mailbox"], 0), tag("specialuse", S, &["special-use"], 0)];
const VACATION: &[TagSpec] = &[
    tag("days", N, NONE, G_DAYS),
    tag("seconds", N, &["vacation-seconds"], G_DAYS),
    tag("subject", S, NONE, 0),
    tag("from", S, NONE, 0),
    tag("addresses", L, NONE, 0),
    tag("mime", None, NONE, 0),
    tag("handle", S, NONE, 0),
];
const FCC: &[TagSpec] = &[tag("fcc", S, &["fcc"], 0)];
const MODIFIERS: &[TagSpec] = &[
    tag("lower", None, NONE, G_CASE),
    tag("upper", None, NONE, G_CASE),
    tag("lowerfirst", None, NONE, G_FIRST),
    tag("upperfirst", None, NONE, G_FIRST),
    tag("quotewildcard", None, NONE, G_QUOTE),
    tag("quoteregex", None, &["regex"], G_QUOTE),
    tag("length", None, NONE, 0),
    tag("encodeurl", None, &["enotify"], 0),
];
const INCLUDE: &[TagSpec] =
    &[tag("personal", None, NONE, G_WHERE), tag("global", None, NONE, G_WHERE), tag("once", None, NONE, 0), tag("optional", None, NONE, 0)];
const NOTIFY: &[TagSpec] = &[tag("from", S, NONE, 0), tag("importance", S, NONE, 0), tag("options", L, NONE, 0), tag("message", S, NONE, 0)];
const DUPLICATE: &[TagSpec] = &[
    tag("handle", S, NONE, 0),
    tag("header", S, NONE, G_ID),
    tag("uniqueid", S, NONE, G_ID),
    tag("seconds", N, NONE, 0),
    tag("last", None, NONE, 0),
];
const LAST: &[TagSpec] = &[tag("last", None, NONE, 0)];
const DELETEHEADER: &[TagSpec] = &[tag("index", N, NONE, 0), tag("last", None, NONE, 0)];
const PERCENT: &[TagSpec] = &[tag("percent", None, &["spamtestplus"], 0)];
const NAME: &[TagSpec] = &[tag("name", S, NONE, 0)];
const REPLACE: &[TagSpec] = &[tag("mime", None, NONE, 0), tag("subject", S, NONE, 0), tag("from", S, NONE, 0)];
const ENCLOSE: &[TagSpec] = &[tag("subject", S, NONE, 0), tag("headers", L, NONE, 0)];
const FIRST: &[TagSpec] = &[tag("first", N, NONE, 0)];

const STR: Wants = Wants::Str;
const LIST: Wants = Wants::List;
const NUM: Wants = Wants::Num;

const fn spec(name: &'static str, kind: Kind, ext: &'static [&'static str], tags: &'static [&'static [TagSpec]], pos: &'static [Wants]) -> Spec {
    Spec { name, kind, ext, tags, pos, lead: 0, trail: 0, tests: Takes::No, block: false, needs_group: 0 }
}

const fn with_test(s: Spec, tests: Takes) -> Spec {
    Spec { tests, ..s }
}

const fn with_block(s: Spec) -> Spec {
    Spec { block: true, ..s }
}

const fn optional(s: Spec, lead: usize, trail: usize) -> Spec {
    Spec { lead, trail, ..s }
}

use Kind::{Action, Control, Test};

/// Every command and test known, with what it takes and needs.
const SPECS: &[Spec] = &[
    // RFC 5228.
    spec("require", Control, NONE, &[], &[LIST]),
    with_block(with_test(spec("if", Control, NONE, &[], &[]), Takes::One)),
    with_block(with_test(spec("elsif", Control, NONE, &[], &[]), Takes::One)),
    with_block(spec("else", Control, NONE, &[], &[])),
    spec("stop", Control, NONE, &[], &[]),
    spec("keep", Action, NONE, &[FLAGS], &[]),
    spec("discard", Action, NONE, &[], &[]),
    spec("redirect", Action, NONE, &[COPY], &[STR]),
    spec("fileinto", Action, &["fileinto"], &[COPY, FLAGS, FILEINTO], &[STR]),
    spec("address", Test, NONE, &[COMPARATOR, MATCH, ADDRESS_PART, INDEX, MIME_ANY], &[LIST, LIST]),
    spec("envelope", Test, &["envelope"], &[COMPARATOR, MATCH, ADDRESS_PART], &[LIST, LIST]),
    spec("header", Test, NONE, &[COMPARATOR, MATCH, INDEX, MIME_ANY, MIME_PARTS], &[LIST, LIST]),
    spec("exists", Test, NONE, &[MIME_ANY], &[LIST]),
    Spec { needs_group: G_SIZE, ..spec("size", Test, NONE, &[SIZE], &[NUM]) },
    with_test(spec("allof", Test, NONE, &[], &[]), Takes::List),
    with_test(spec("anyof", Test, NONE, &[], &[]), Takes::List),
    with_test(spec("not", Test, NONE, &[], &[]), Takes::One),
    spec("true", Test, NONE, &[], &[]),
    spec("false", Test, NONE, &[], &[]),
    // RFC 5232.
    optional(spec("setflag", Action, &["imap4flags"], &[], &[STR, LIST]), 1, 0),
    optional(spec("addflag", Action, &["imap4flags"], &[], &[STR, LIST]), 1, 0),
    optional(spec("removeflag", Action, &["imap4flags"], &[], &[STR, LIST]), 1, 0),
    optional(spec("hasflag", Test, &["imap4flags"], &[COMPARATOR, MATCH], &[LIST, LIST]), 1, 0),
    // RFC 5230, 6131, 8580.
    spec("vacation", Action, &["vacation"], &[VACATION, FCC], &[STR]),
    // RFC 5173.
    spec("body", Test, &["body"], &[COMPARATOR, MATCH, BODY], &[LIST]),
    // RFC 5429.
    spec("reject", Action, &["reject"], &[], &[STR]),
    spec("ereject", Action, &["ereject"], &[], &[STR]),
    // RFC 6609.
    spec("include", Control, &["include"], &[INCLUDE], &[STR]),
    spec("return", Control, &["include"], &[], &[]),
    spec("global", Control, &["include", "variables"], &[], &[LIST]),
    // RFC 5229.
    spec("set", Action, &["variables"], &[MODIFIERS], &[STR, STR]),
    spec("string", Test, &["variables"], &[COMPARATOR, MATCH], &[LIST, LIST]),
    // RFC 5260.
    spec("date", Test, &["date"], &[DATE_ZONE, COMPARATOR, MATCH, INDEX], &[STR, STR, LIST]),
    spec("currentdate", Test, &["date"], &[ZONE, COMPARATOR, MATCH], &[STR, LIST]),
    // RFC 5490.
    spec("mailboxexists", Test, &["mailbox"], &[], &[LIST]),
    spec("metadata", Test, &["mboxmetadata"], &[COMPARATOR, MATCH], &[STR, STR, LIST]),
    spec("metadataexists", Test, &["mboxmetadata"], &[], &[STR, LIST]),
    spec("servermetadata", Test, &["servermetadata"], &[COMPARATOR, MATCH], &[STR, LIST]),
    spec("servermetadataexists", Test, &["servermetadata"], &[], &[LIST]),
    // RFC 5435.
    spec("notify", Action, &["enotify"], &[NOTIFY, FCC], &[STR]),
    spec("valid_notify_method", Test, &["enotify"], &[], &[LIST]),
    spec("notify_method_capability", Test, &["enotify"], &[COMPARATOR, MATCH], &[STR, STR, LIST]),
    // RFC 5293.
    spec("addheader", Action, &["editheader"], &[LAST], &[STR, STR]),
    optional(spec("deleteheader", Action, &["editheader"], &[DELETEHEADER, COMPARATOR, MATCH], &[STR, LIST]), 0, 1),
    // RFC 5463.
    spec("ihave", Test, &["ihave"], &[], &[LIST]),
    spec("error", Control, &["ihave"], &[], &[STR]),
    // RFC 5183.
    spec("environment", Test, &["environment"], &[COMPARATOR, MATCH], &[STR, LIST]),
    // RFC 5235.
    spec("spamtest", Test, &["spamtest"], &[PERCENT, COMPARATOR, MATCH], &[STR]),
    spec("virustest", Test, &["virustest"], &[COMPARATOR, MATCH], &[STR]),
    // RFC 6134.
    spec("valid_ext_list", Test, &["extlists"], &[], &[LIST]),
    // RFC 7352.
    spec("duplicate", Test, &["duplicate"], &[DUPLICATE], &[]),
    // RFC 8579.
    optional(spec("specialuse_exists", Test, &["special-use"], &[], &[STR, LIST]), 1, 0),
    // RFC 5703.
    with_block(spec("foreverypart", Control, &["foreverypart"], &[NAME], &[])),
    spec("break", Control, &["foreverypart"], &[NAME], &[]),
    spec("replace", Action, &["replace"], &[REPLACE], &[STR]),
    spec("enclose", Action, &["enclose"], &[ENCLOSE], &[STR]),
    spec("extracttext", Action, &["extracttext"], &[MODIFIERS, FIRST], &[STR]),
];

fn lookup(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name == name)
}

// ---- Syntax ----------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum Val {
    Str(String),
    List(Vec<String>),
    Num,
    Tag(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Arg {
    val: Val,
    at: Range<usize>,
}

/// A command or a test.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Node {
    name: String,
    name_at: Range<usize>,
    args: Vec<Arg>,
    tests: Vec<Node>,
    /// Its tests are a list in brackets.
    test_list: bool,
    block: Option<Vec<Node>>,
    /// Where its arguments end: a missing `;` goes there.
    args_end: usize,
}

/// A syntax error: the first one ends the reading.
struct SyntaxError {
    fault: Fault,
    at: Range<usize>,
}

struct Parser {
    toks: Vec<(Tok, Range<usize>)>,
    at: usize,
    commands: usize,
    /// The end of the script, where a token missing at the end would be.
    len: usize,
}

type Parsed<T> = Result<T, SyntaxError>;

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at).map(|(t, _)| t)
    }

    /// Where the next token is, or the end of the script.
    fn here(&self) -> Range<usize> {
        self.toks.get(self.at).map_or(self.len..self.len, |(_, r)| r.clone())
    }

    /// Where the token before the next one ends.
    fn prev_end(&self) -> usize {
        self.at.checked_sub(1).and_then(|i| self.toks.get(i)).map_or(0, |(_, r)| r.end)
    }

    fn take(&mut self) -> Option<(Tok, Range<usize>)> {
        let slot = self.toks.get_mut(self.at)?;
        let tok = std::mem::replace(&mut slot.0, Tok::Punct(' '));
        let at = slot.1.clone();
        self.at = self.at.saturating_add(1);
        Some((tok, at))
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Punct(c)) {
            self.at = self.at.saturating_add(1);
            true
        } else {
            false
        }
    }

    fn fail<T>(&self, fault: Fault) -> Parsed<T> {
        Err(SyntaxError { fault, at: self.here() })
    }

    fn args(&mut self) -> Parsed<Vec<Arg>> {
        let mut args = Vec::new();
        loop {
            let start = self.here();
            let val = match self.peek() {
                Some(Tok::Str(_) | Tok::Num(_) | Tok::Tag(_)) => match self.take() {
                    Some((Tok::Str(s), _)) => Val::Str(s),
                    Some((Tok::Tag(t), _)) => Val::Tag(t),
                    _ => Val::Num,
                },
                Some(Tok::Punct('[')) => {
                    self.at = self.at.saturating_add(1);
                    let mut list = Vec::new();
                    loop {
                        match self.peek() {
                            Some(Tok::Str(_)) => {
                                if let Some((Tok::Str(s), _)) = self.take() {
                                    list.push(s);
                                }
                            }
                            _ => return self.fail(Fault::ExpectedString),
                        }
                        if self.eat(']') {
                            break;
                        }
                        if !self.eat(',') {
                            return self.fail(Fault::ListEnd);
                        }
                    }
                    Val::List(list)
                }
                _ => return Ok(args),
            };
            if args.len() >= MAX_ARGS {
                return Err(SyntaxError { fault: Fault::TooManyArgs, at: start });
            }
            args.push(Arg { val, at: start.start..self.prev_end() });
        }
    }

    /// A test's or command's tests: one, a list in brackets, or none.
    fn tests(&mut self, depth: usize) -> Parsed<(Vec<Node>, bool)> {
        if self.eat('(') {
            let mut tests = Vec::new();
            loop {
                if tests.len() >= MAX_ARGS {
                    return self.fail(Fault::TooManyArgs);
                }
                tests.push(self.test(depth.saturating_add(1))?);
                if self.eat(')') {
                    return Ok((tests, true));
                }
                if !self.eat(',') {
                    return self.fail(Fault::TestListEnd);
                }
            }
        } else if matches!(self.peek(), Some(Tok::Ident(_))) {
            Ok((vec![self.test(depth.saturating_add(1))?], false))
        } else {
            Ok((Vec::new(), false))
        }
    }

    fn test(&mut self, depth: usize) -> Parsed<Node> {
        if depth > MAX_DEPTH {
            return self.fail(Fault::TooDeep);
        }
        let (name, name_at) = match self.peek() {
            Some(Tok::Ident(_)) => match self.take() {
                Some((Tok::Ident(n), at)) => (n, at),
                _ => return self.fail(Fault::ExpectedTest),
            },
            _ => return self.fail(Fault::ExpectedTest),
        };
        let args = self.args()?;
        let args_end = self.prev_end();
        let (tests, test_list) = self.tests(depth)?;
        Ok(Node { name, name_at, args, tests, test_list, block: None, args_end })
    }

    /// Commands up to a `}` (left for the caller) or the end.
    fn commands(&mut self, depth: usize) -> Parsed<Vec<Node>> {
        if depth > MAX_DEPTH {
            return self.fail(Fault::TooDeep);
        }
        let mut out = Vec::new();
        while let Some(t) = self.peek() {
            if *t == Tok::Punct('}') {
                break;
            }
            self.commands = self.commands.saturating_add(1);
            if self.commands > MAX_COMMANDS {
                return self.fail(Fault::TooManyCommands);
            }
            let (name, name_at) = match self.peek() {
                Some(Tok::Ident(_)) => match self.take() {
                    Some((Tok::Ident(n), at)) => (n, at),
                    _ => return self.fail(Fault::ExpectedCommand),
                },
                _ => return self.fail(Fault::ExpectedCommand),
            };
            let args = self.args()?;
            let args_end = self.prev_end();
            let (tests, test_list) = self.tests(depth)?;
            let block = if self.peek() == Some(&Tok::Punct('{')) {
                let open = self.here();
                self.at = self.at.saturating_add(1);
                let inner = self.commands(depth.saturating_add(1))?;
                if !self.eat('}') {
                    return Err(SyntaxError { fault: Fault::UnclosedBlock, at: open });
                }
                Some(inner)
            } else if self.eat(';') {
                None
            } else {
                let end = self.prev_end();
                return Err(SyntaxError { fault: Fault::MissingSemicolon, at: end..end });
            };
            out.push(Node { name, name_at, args, tests, test_list, block, args_end });
        }
        Ok(out)
    }
}

/// The script's commands, or its first syntax error.
fn parse(script: &str) -> Parsed<Vec<Node>> {
    let lexed = lex_or_why(script).map_err(|e| SyntaxError { fault: Fault::Lex(e.fault), at: e.at })?;
    let mut p = Parser { toks: lexed.toks, at: 0, commands: 0, len: script.len() };
    let commands = p.commands(0)?;
    if p.at < p.toks.len() {
        return p.fail(Fault::StrayClose);
    }
    Ok(commands)
}

// ---- Meaning ---------------------------------------------------------------------------------

/// A name as a problem keeps it: at most [`MAX_NAME`] bytes, on a character boundary.
fn short(s: &str) -> String {
    let mut end = s.len().min(MAX_NAME);
    while !s.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    s.get(..end).unwrap_or_default().to_string()
}

struct Ctx<'a> {
    block_mode: bool,
    server: Option<&'a [String]>,
    /// The `require`s, and where each is.
    requires: Vec<(String, Range<usize>)>,
    /// Extensions an enclosing `if ihave` names (RFC 5463 §4).
    ihave: Vec<String>,
    /// A `require`d extension isn't known here, so what isn't known is only a warning.
    lenient: bool,
    needs: Vec<(String, Range<usize>)>,
    problems: Vec<(Fault, Range<usize>, bool)>,
}

impl Ctx<'_> {
    fn error(&mut self, fault: Fault, at: Range<usize>) {
        self.problem(fault, at, false);
    }

    fn problem(&mut self, fault: Fault, at: Range<usize>, warning: bool) {
        if self.problems.len() < MAX_PROBLEMS {
            self.problems.push((fault, at, warning));
        }
    }

    /// An unknown thing: an error, or a warning while a required extension (or one an enclosing
    /// `ihave` names) is unknown; always an error in a block.
    fn unknown(&mut self, fault: Fault, at: Range<usize>) {
        // Inside `if ihave` naming an extension not known here, what isn't known may be its.
        let guarded = self.ihave.iter().any(|e| !known(e));
        let warning = (self.lenient || guarded) && !self.block_mode;
        self.problem(fault, at, warning);
    }

    /// Whether `ext` is named by a `require` (or an enclosing `ihave`).
    fn declared(&self, ext: &str) -> bool {
        let by = |r: &str| r == ext || (ext == "spamtest" && r == "spamtestplus") || (ext == "vacation" && r == "vacation-seconds");
        self.requires.iter().any(|(r, _)| by(r)) || self.ihave.iter().any(|r| by(r))
    }

    /// `name` (at `at`) uses `exts`.
    fn uses(&mut self, name: &str, exts: &[&str], at: &Range<usize>) {
        for ext in exts {
            self.use_ext(name, ext, at);
        }
    }

    fn use_ext(&mut self, name: &str, ext: &str, at: &Range<usize>) {
        if !self.needs.iter().any(|(n, _)| n == ext) {
            self.needs.push((ext.to_string(), at.clone()));
        }
        if !self.block_mode && !self.declared(ext) {
            self.error(Fault::NeedsRequire { name: short(name), ext: short(ext) }, at.clone());
        }
    }

    fn commands(&mut self, nodes: &[Node], top: bool) {
        let mut seen_other = false;
        // The command before, for `elsif` and `else`.
        let mut after_if = false;
        for node in nodes {
            let name = node.name.as_str();
            if name == "require" {
                if self.block_mode {
                    self.error(Fault::RequireInBlock, node.name_at.clone());
                } else if !top || seen_other {
                    self.error(Fault::RequireNotFirst, node.name_at.clone());
                }
            } else {
                seen_other = true;
            }
            if (name == "elsif" || name == "else") && !after_if {
                self.error(Fault::ElseWithoutIf(short(name)), node.name_at.clone());
            }
            after_if = name == "if" || name == "elsif";
            self.command(node);
        }
    }

    fn command(&mut self, node: &Node) {
        let Some(spec) = lookup(&node.name) else {
            self.unknown(Fault::UnknownCommand(short(&node.name)), node.name_at.clone());
            for t in &node.tests {
                self.test(t);
            }
            if let Some(block) = &node.block {
                self.commands(block, false);
            }
            return;
        };
        if spec.kind == Kind::Test {
            self.error(Fault::TestAsCommand(short(spec.name)), node.name_at.clone());
        }
        self.uses(spec.name, spec.ext, &node.name_at);
        self.args(spec, node);
        // A command that takes no test, followed by what is a command: its `;` was left out.
        let missing_semicolon =
            spec.tests == Takes::No && !node.test_list && node.tests.first().is_some_and(|t| lookup(&t.name).is_some_and(|s| s.kind != Kind::Test));
        if missing_semicolon {
            self.error(Fault::MissingSemicolon, node.args_end..node.args_end);
            if let Some(block) = &node.block {
                self.commands(block, false);
            }
            return;
        }
        self.takes_tests(spec, node);
        // `if ihave [...]`: what it names may be used in its block.
        let pushed = match (spec.name, node.tests.first()) {
            ("if" | "elsif", Some(t)) if t.name == "ihave" => {
                let before = self.ihave.len();
                for a in &t.args {
                    match &a.val {
                        Val::Str(s) => self.ihave.push(s.clone()),
                        Val::List(l) => self.ihave.extend(l.iter().cloned()),
                        _ => {}
                    }
                }
                Some(before)
            }
            _ => None,
        };
        match (&node.block, spec.block) {
            (Some(block), true) => self.commands(block, false),
            (None, true) => self.error(Fault::NeedsBlock(short(spec.name)), node.name_at.clone()),
            (Some(block), false) => {
                self.error(Fault::NoBlock(short(spec.name)), node.name_at.clone());
                self.commands(block, false);
            }
            (None, false) => {}
        }
        if let Some(before) = pushed {
            self.ihave.truncate(before);
        }
    }

    fn test(&mut self, node: &Node) {
        let Some(spec) = lookup(&node.name) else {
            self.unknown(Fault::UnknownTest(short(&node.name)), node.name_at.clone());
            for t in &node.tests {
                self.test(t);
            }
            return;
        };
        if spec.kind != Kind::Test {
            self.error(Fault::CommandAsTest(short(spec.name)), node.name_at.clone());
            return;
        }
        self.uses(spec.name, spec.ext, &node.name_at);
        self.args(spec, node);
        self.takes_tests(spec, node);
    }

    /// Whether `node` has the tests `spec` takes, and theirs.
    fn takes_tests(&mut self, spec: &Spec, node: &Node) {
        let name = short(spec.name);
        match (spec.tests, node.tests.len(), node.test_list) {
            (Takes::No, 0, false) | (Takes::One, 1, false) | (Takes::List, _, true) => {}
            (Takes::No, _, _) => self.error(Fault::NoTest(name), node.name_at.clone()),
            (Takes::One, 0, _) => self.error(Fault::NeedsTest(name), node.name_at.clone()),
            (Takes::One, _, _) => self.error(Fault::OneTest(name), node.name_at.clone()),
            (Takes::List, _, false) => self.error(Fault::NeedsTestList(name), node.name_at.clone()),
        }
        for t in &node.tests {
            self.test(t);
        }
    }

    /// Whether `node`'s arguments are the ones `spec` takes: its tags first, each known, once,
    /// with its value, then its positional arguments, as many as it takes, each of its kind.
    fn args(&mut self, spec: &Spec, node: &Node) {
        let mut given: Vec<&TagSpec> = Vec::new();
        let mut positional: Vec<&Arg> = Vec::new();
        let mut args = node.args.iter();
        while let Some(arg) = args.next() {
            let Val::Tag(t) = &arg.val else {
                positional.push(arg);
                continue;
            };
            if !positional.is_empty() {
                self.error(Fault::TagAfterPositional(short(t)), arg.at.clone());
            }
            let Some(ts) = spec.tags.iter().flat_map(|g| g.iter()).find(|x| x.name == t) else {
                self.unknown(Fault::UnknownTag { tag: short(t), name: short(spec.name) }, arg.at.clone());
                continue;
            };
            if given.iter().any(|g| g.name == ts.name) {
                self.error(Fault::TagTwice(short(t)), arg.at.clone());
            } else if let Some(other) = given.iter().find(|g| ts.group != 0 && g.group == ts.group) {
                self.error(Fault::TagClash { tag: short(t), other: short(other.name) }, arg.at.clone());
            }
            given.push(ts);
            self.uses(&format!(":{}", ts.name), ts.ext, &arg.at);
            let Some(wants) = ts.value else { continue };
            let value = args.next();
            let fits = value.is_some_and(|v| fits(&v.val, wants));
            if !fits {
                self.error(Fault::TagValue { tag: short(t), wants }, value.map_or(arg.at.clone(), |v| v.at.clone()));
                continue;
            }
            if let Some(Arg { val: Val::Str(v), at }) = value {
                self.tag_value(ts.name, v, at);
            }
        }
        if spec.needs_group != 0 && !given.iter().any(|g| g.group == spec.needs_group) {
            let tags: Vec<String> =
                spec.tags.iter().flat_map(|g| g.iter()).filter(|t| t.group == spec.needs_group).map(|t| format!(":{}", t.name)).collect();
            self.error(Fault::NeedsTag { name: short(spec.name), tags: tags.join(", ") }, node.name_at.clone());
        }
        let most = spec.pos.len();
        let least = most.saturating_sub(spec.lead).saturating_sub(spec.trail);
        let name = short(spec.name);
        if positional.len() < least {
            self.error(Fault::MissingArg(name), node.name_at.clone());
            return;
        }
        if let Some(extra) = positional.get(most) {
            self.error(Fault::ExtraArg(name), extra.at.clone());
            return;
        }
        // Left out: the first ones it may lack, else the last.
        let skip = if spec.lead > 0 { most.saturating_sub(positional.len()) } else { 0 };
        for (arg, wants) in positional.iter().zip(spec.pos.iter().skip(skip)) {
            if !fits(&arg.val, *wants) {
                self.error(Fault::WrongArg { name: name.clone(), wants: *wants }, arg.at.clone());
            }
        }
    }

    /// What a tag's string says: a comparator needs its extension; a relation is one of six.
    fn tag_value(&mut self, tag: &str, value: &str, at: &Range<usize>) {
        match tag {
            "comparator" => {
                if !COMPARATORS.contains(&value) {
                    self.unknown(Fault::UnknownComparator(short(value)), at.clone());
                } else if value != "i;octet" && value != "i;ascii-casemap" {
                    let ext = format!("comparator-{value}");
                    self.use_ext(&format!(":comparator \"{value}\""), &ext, at);
                }
            }
            "value" | "count" if !["gt", "ge", "lt", "le", "eq", "ne"].iter().any(|r| r.eq_ignore_ascii_case(value)) => {
                self.error(Fault::BadRelation(short(value)), at.clone());
            }
            _ => {}
        }
    }
}

/// Whether a value is of the kind wanted (a string is a list of one).
fn fits(val: &Val, wants: Wants) -> bool {
    matches!((val, wants), (Val::Str(_), Wants::Str | Wants::List) | (Val::List(_), Wants::List) | (Val::Num, Wants::Num))
}

/// Whether an extension is known here.
fn known(ext: &str) -> bool {
    EXTENSIONS.contains(&ext) || ext.strip_prefix("comparator-").is_some_and(|c| COMPARATORS.contains(&c))
}

fn run(script: &str, server: Option<&[String]>, block_mode: bool) -> Result<Checked, Vec<Problem>> {
    let nodes = match parse(script) {
        Ok(n) => n,
        Err(e) => return Err(positioned(script, vec![(e.fault, e.at, false)])),
    };
    let mut cx = Ctx { block_mode, server, requires: Vec::new(), ihave: Vec::new(), lenient: false, needs: Vec::new(), problems: Vec::new() };
    if !block_mode {
        for node in nodes.iter().filter(|n| n.name == "require") {
            for arg in &node.args {
                match &arg.val {
                    Val::Str(s) => cx.requires.push((s.clone(), arg.at.clone())),
                    Val::List(l) => cx.requires.extend(l.iter().map(|s| (s.clone(), arg.at.clone()))),
                    _ => {}
                }
            }
        }
        let unknown: Vec<(String, Range<usize>)> = cx.requires.iter().filter(|(r, _)| !known(r)).cloned().collect();
        cx.lenient = !unknown.is_empty();
        for (ext, at) in unknown {
            // A server that has it knows what it adds; else MailCraft can't check that.
            let on_server = cx.server.is_some_and(|s| s.iter().any(|e| e.eq_ignore_ascii_case(&ext)));
            if !on_server {
                cx.problem(Fault::UnknownExtension(short(&ext)), at, true);
            }
        }
    }
    cx.commands(&nodes, true);
    // What the server lacks: each extension required, or (in a block) needed, once.
    if let Some(server) = cx.server {
        let wanted: Vec<(String, Range<usize>)> = if block_mode { cx.needs.clone() } else { cx.requires.clone() };
        let mut said: Vec<&str> = Vec::new();
        let mut lacks = Vec::new();
        for (ext, at) in &wanted {
            if said.contains(&ext.as_str()) || (!known(ext) && !block_mode) {
                continue;
            }
            said.push(ext);
            if !server.iter().any(|s| s.eq_ignore_ascii_case(ext)) {
                lacks.push((Fault::ServerLacks(short(ext)), at.clone(), true));
            }
        }
        for p in lacks {
            cx.problem(p.0, p.1, p.2);
        }
    }
    let requires = cx.requires.iter().map(|(r, _)| r.clone()).collect();
    let needs = cx.needs.iter().map(|(n, _)| n.clone()).collect();
    let errors = cx.problems.iter().any(|(_, _, w)| !w);
    let problems = positioned(script, cx.problems);
    if errors { Err(problems) } else { Ok(Checked { requires, needs, warnings: problems }) }
}

/// The problems in the order of the script, each with its line and column, counted in one pass.
fn positioned(script: &str, mut found: Vec<(Fault, Range<usize>, bool)>) -> Vec<Problem> {
    found.sort_by_key(|(_, at, w)| (at.start, *w));
    let mut out = Vec::with_capacity(found.len());
    let (mut line, mut column, mut done) = (1usize, 1usize, 0usize);
    for (fault, at, warning) in found {
        // A span from outside the script, or off a character boundary, is put right.
        let mut start = at.start.min(script.len());
        while !script.is_char_boundary(start) {
            start = start.saturating_sub(1);
        }
        let mut end = at.end.clamp(start, script.len());
        while !script.is_char_boundary(end) {
            end = end.saturating_add(1).min(script.len());
        }
        for c in script.get(done..start).unwrap_or_default().chars() {
            if c == '\n' {
                line = line.saturating_add(1);
                column = 1;
            } else {
                column = column.saturating_add(1);
            }
        }
        done = done.max(start);
        out.push(Problem { fault, at: start..end, line, column, warning });
    }
    out
}
