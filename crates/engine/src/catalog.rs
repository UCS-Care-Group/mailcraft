// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mail-client feature catalog: the incumbent's ribbon and menu commands (feature names
//! only) mapped to our command ids. `parity()` compares it with the registry; the gap is the
//! work list (`cargo xtask parity` → docs/parity-checklist.md).

use serde_json::{Value, json};

use crate::Registry;

/// `Tab|Group|Feature|command id`
pub const CATALOG: &str = "\
File|Backstage|Info|file.info
File|Backstage|Add Account|account.add
File|Backstage|Account Settings|account.edit
File|Backstage|Remove Account|account.remove
File|Backstage|Open Mailbox File|file.open
File|Backstage|Save Mailbox|file.save
File|Backstage|Save Mailbox As|file.saveAs
File|Backstage|Import|file.import
File|Backstage|Import a Folder of Mailboxes|file.importFolderTree
File|Backstage|Export Folder|file.exportFolder
File|Backstage|Save Message As|file.exportMessage
File|Backstage|Save Calendar|file.exportCalendar
File|Backstage|Export Contacts|file.exportContacts
File|Backstage|Save Attachments|mail.saveAttachment
File|Backstage|Print|file.print
File|Backstage|Options|file.options
File|Backstage|Signatures|signature.set
File|Backstage|Automatic Replies|account.automaticReplies
File|Backstage|Mailbox Cleanup|file.cleanup
File|Backstage|Rules and Alerts|rules.list
File|Backstage|Open Data File|file.openDataFile
File|Backstage|Open Shared Calendar|calendar.openShared
File|Backstage|Office Account|file.account
Quick Access|Toolbar|Send/Receive All Folders|sync.request
Quick Access|Toolbar|Undo|edit.undo
Quick Access|Toolbar|Redo|edit.redo
Search|Search|Search Mailbox|view.search
Search|Search|Search Scope|view.search
Search|Search|Close Search|view.clearSearch
Search|Refine|From|view.search
Search|Refine|Subject|view.search
Search|Refine|Has Attachments|view.search
Search|Refine|Categorized|view.search
Search|Refine|Unread|view.search
Search|Refine|Flagged|view.search
Search|Refine|Important|view.search
Search|Results|More Results on the Server|sync.graphSearchRequest
Search|Options|Recent Searches|view.recentSearches
Search|Options|Search Folders|folder.newSearchFolder
Home|New|New Email|mail.new
Home|New|New Appointment|calendar.new
Home|New|New Meeting|calendar.newMeeting
Home|New|New Contact|people.new
Home|New|New Task|tasks.new
Home|New|New Contact Group|people.newGroup
Home|Delete|Ignore Conversation|mail.ignore
Home|Delete|Clean Up Conversation|mail.cleanUp
Home|Delete|Junk|mail.junk
Home|Delete|Not Junk|mail.notJunk
Home|Delete|Block Sender|junk.blockSender
Home|Delete|Never Block Sender|junk.neverBlockSender
Home|Delete|Never Block Sender's Domain|junk.neverBlockDomain
Home|Delete|Junk E-mail Options|junk.list
Home|Delete|Delete|mail.delete
Home|Delete|Permanently Delete|mail.deletePermanently
Home|Delete|Archive|mail.archive
Home|Respond|Reply|mail.reply
Home|Respond|Reply All|mail.replyAll
Home|Respond|Forward|mail.forward
Home|Respond|Reply with Meeting|calendar.fromMessage
Home|Respond|Forward as Attachment|mail.forwardAsAttachment
Home|Respond|Reply with IM|mail.replyIm
Home|Quick Steps|Move to Folder Quick Step|mail.move
Home|Quick Steps|Reply and Delete|mail.replyAndDelete
Home|Quick Steps|Create Quick Step|quickSteps.new
Home|Quick Steps|Manage Quick Steps|quickSteps.manage
Home|Move|Move|mail.move
Home|Move|Copy to Folder|mail.copy
Home|Move|Create Rule|rules.add
Home|Move|Always Move Messages From|rules.fromMessage
Home|Move|Manage Rules|rules.list
Home|Move|Delete Rule|rules.remove
Home|Move|Enable Rule|rules.enable
Home|Tags|Unread/ Read|mail.toggleRead
Home|Tags|Mark as Read|mail.markRead
Home|Tags|Mark as Unread|mail.markUnread
Home|Tags|Categorize|mail.categorize
Home|Tags|Clear All Categories|mail.clearCategories
Home|Tags|All Categories|category.list
Home|Tags|New Category|category.set
Home|Tags|Rename Category|category.rename
Home|Tags|Delete Category|category.remove
Home|Tags|Follow Up|mail.flag
Home|Tags|Flag Message|mail.toggleFlag
Home|Tags|Mark Complete|mail.complete
Home|Tags|Clear Flag|mail.clearFlag
Home|Tags|Add Reminder|mail.reminder
Home|Tags|Pin|mail.pin
Home|Tags|Add to Tasks|tasks.fromMessage
Home|Find|Search People|people.search
Home|Find|Address Book|people.addressBook
Home|Find|Filter Email|view.filter
Home|Find|Find Related Messages|mail.findRelated
Home|Find|Messages from Sender|mail.findFromSender
Home|Actions|Add Sender to Contacts|people.fromMessage
Home|Actions|View Source|mail.viewSource
Home|Actions|Translate|mail.translate
Home|Actions|Read Aloud|mail.readAloud
Home|Actions|Resend Message|mail.resend
Home|Actions|Recall Message|mail.recall
Home|Actions|Edit Message|mail.edit
Send / Receive|Send & Receive|Send/Receive All Folders|sync.request
Send / Receive|Send & Receive|Update Folder|sync.folder
Send / Receive|Send & Receive|Send All|sync.sendAll
Send / Receive|Send & Receive|Send/Receive Groups|sync.groups
Send / Receive|Download|Show Progress|sync.progress
Send / Receive|Download|Cancel All|sync.cancel
Send / Receive|Preferences|Work Offline|sync.offline
Folder|New|New Folder|folder.new
Folder|New|New Search Folder|folder.newSearchFolder
Folder|Actions|Rename Folder|folder.rename
Folder|Actions|Copy Folder|folder.copy
Folder|Actions|Move Folder|folder.move
Folder|Actions|Delete Folder|folder.delete
Folder|Actions|Go to Folder|folder.select
Folder|Actions|Select All|mail.selectAll
Folder|Clean Up|Mark All as Read|mail.markAllRead
Folder|Clean Up|Run Rules Now|rules.run
Folder|Clean Up|Clean Up Folder|folder.cleanUp
Folder|Clean Up|Empty Folder|folder.empty
Folder|Clean Up|Recover Deleted Items|folder.recover
Folder|Favorites|Add to Favorites|folder.favorite
Folder|Properties|Folder Permissions|folder.permissions
Folder|Properties|Folder Properties|folder.properties
View|Current View|Change View|view.changeView
View|Current View|View Settings|view.settings
View|Current View|Reset View|view.reset
View|Messages|Show as Conversations|view.conversations
View|Messages|Conversation Settings|view.conversationSettings
View|Arrangement|Message Preview|view.previewLines
View|Arrangement|Arrange By|view.sort
View|Arrangement|Reverse Sort|view.reverseSort
View|Arrangement|Add Columns|view.columns
View|Arrangement|Expand/Collapse Groups|view.groups
View|Arrangement|Single Line|view.oneLine
View|Layout|Folder Pane|view.folderPane
View|Layout|Reading Pane|view.readingPane
View|Layout|To-Do Bar|view.todoBar
View|Layout|Use Tighter Spacing|view.density
View|People Pane|People Pane|view.peoplePane
View|Window|Reminders Window|view.reminders
View|Window|Open in New Window|view.newWindow
View|Window|Immersive Reader|view.immersiveReader
View|Window|Zoom|view.zoom
Navigation|Modules|Mail|view.mail
Navigation|Modules|Calendar|view.calendar
Navigation|Modules|People|view.people
Navigation|Modules|Tasks|view.tasks
Navigation|Modules|Notes|view.notes
Message|Clipboard|Paste|edit.paste
Message|Clipboard|Cut|edit.cut
Message|Clipboard|Copy|edit.copy
Message|Clipboard|Format Painter|edit.formatPainter
Message|Basic Text|Font|format.font
Message|Basic Text|Font Size|format.size
Message|Basic Text|Bold|format.bold
Message|Basic Text|Italic|format.italic
Message|Basic Text|Underline|format.underline
Message|Basic Text|Bullets|format.bullets
Message|Basic Text|Numbering|format.numbering
Message|Basic Text|Font Color|format.color
Message|Basic Text|Text Highlight Color|format.highlight
Message|Basic Text|Align Text|format.align
Message|Basic Text|Clear All Formatting|format.clear
Message|Names|Address Book|people.addressBook
Message|Names|Check Names|compose.checkNames
Message|Include|Attach File|compose.attach
Message|Include|Remove Attachment|compose.removeAttachment
Message|Include|Attach Item|compose.attachItem
Message|Include|Signature|compose.signature
Message|Include|Link|compose.link
Message|Tags|Follow Up|compose.followUp
Message|Tags|High Importance|compose.highImportance
Message|Tags|Low Importance|compose.lowImportance
Message|Send|Send|compose.send
Message|Send|Save Draft|compose.saveDraft
Message|Send|Discard|compose.discard
Message|Send|Close|compose.close
Message|Send|Edit Message Fields|compose.set
Message|Send|Open Draft|mail.editDraft
Insert|Include|Item|insert.outlookItem
Insert|Include|Business Card|insert.businessCard
Insert|Include|Calendar|insert.calendar
Insert|Tables|Table|insert.table
Insert|Illustrations|Pictures|insert.picture
Insert|Illustrations|Shapes|insert.shape
Insert|Illustrations|Icons|insert.icon
Insert|Illustrations|3D Models|insert.models3d
Insert|Illustrations|Diagram|insert.diagram
Insert|Illustrations|Chart|insert.chart
Insert|Illustrations|Screenshot|insert.screenshot
Insert|Links|Link|compose.link
Insert|Links|Bookmark|insert.bookmark
Insert|Text|Text Box|insert.textBox
Insert|Text|Quick Parts|insert.quickParts
Insert|Text|Text Effects|insert.textEffects
Insert|Text|Drop Cap|insert.dropCap
Insert|Text|Date & Time|insert.dateTime
Insert|Text|Object|insert.object
Insert|Symbols|Symbol|insert.symbol
Insert|Symbols|Equation|insert.equation
Insert|Symbols|Horizontal Line|insert.rule
Insert|Symbols|Emoji|insert.emoji
Options|Themes|Themes|options.theme
Options|Themes|Page Color|options.pageColor
Options|Show Fields|Bcc|compose.set
Options|Show Fields|From|compose.set
Options|Encrypt|Encrypt|compose.encrypt
Options|Encrypt|Sign|compose.sign
Options|Tracking|Use Voting Buttons|compose.voting
Options|Tracking|Request a Delivery Receipt|compose.deliveryReceipt
Options|Tracking|Request a Read Receipt|compose.readReceipt
Options|More Options|Save Sent Item To|compose.saveSentTo
Options|More Options|Delay Delivery|compose.delay
Options|More Options|Direct Replies To|compose.replyTo
Format Text|Format|HTML|compose.formatHtml
Format Text|Format|Plain Text|compose.formatPlain
Format Text|Format|Rich Text|compose.formatRich
Format Text|Paragraph|Line Spacing|format.lineSpacing
Format Text|Paragraph|Indent|format.indent
Format Text|Styles|Styles|format.styles
Format Text|Editing|Find|edit.find
Format Text|Editing|Replace|edit.replace
Review|Proofing|Spelling & Grammar|review.spelling
Review|Proofing|Thesaurus|review.thesaurus
Review|Proofing|Word Count|review.wordCount
Review|Accessibility|Check Accessibility|review.accessibility
Review|Language|Translate|review.translate
Review|Language|Language|review.language
Attachments|Actions|Open|mail.openAttachment
Attachments|Actions|Save As|mail.saveAttachment
Attachments|Actions|Save All Attachments|mail.saveAllAttachments
Attachments|Actions|Remove Attachment|mail.removeAttachment
Attachments|Actions|Quick Print|mail.printAttachment
Attachments|Selection|Select All|mail.selectAllAttachments
Attachments|Selection|Copy|mail.copyAttachment
Calendar|New|New Appointment|calendar.new
Calendar|New|New Meeting|calendar.newMeeting
Calendar|New|New All Day Event|calendar.new
Calendar|New|New Recurring Appointment|calendar.new
Calendar|Go To|Today|view.today
Calendar|Go To|Next 7 Days|view.next7
Calendar|Go To|Back|view.previous
Calendar|Go To|Forward|view.next
Calendar|Go To|Go to Date|view.goToDate
Calendar|Arrange|Day|view.day
Calendar|Arrange|Work Week|view.workWeek
Calendar|Arrange|Week|view.week
Calendar|Arrange|Month|view.month
Calendar|Arrange|Schedule View|view.schedule
Calendar|Manage Calendars|Add Calendar|calendar.addCalendar
Calendar|Manage Calendars|Delete Calendar|calendar.removeCalendar
Calendar|Manage Calendars|Open Calendar|calendar.openShared
Calendar|Manage Calendars|Show Calendar|calendar.showCalendar
Calendar|Manage Calendars|Calendar Groups|calendar.groups
Calendar|Manage Calendars|Overlay Mode|view.overlay
Calendar|Share|Email Calendar|calendar.email
Calendar|Share|Share Calendar|calendar.share
Calendar|Share|Publish Online|calendar.publish
Calendar|Share|Calendar Permissions|calendar.permissions
Calendar|Find|Search Calendar|calendar.search
Appointment|Actions|Open|calendar.select
Appointment|Actions|Edit|calendar.edit
Appointment|Actions|Move|calendar.move
Appointment|Actions|Delete|calendar.delete
Appointment|Actions|Delete Occurrence|calendar.delete
Appointment|Actions|Forward|calendar.forward
Appointment|Actions|Save as iCalendar|file.exportCalendar
Appointment|Actions|Copy to My Calendar|calendar.copy
Appointment|Show|Scheduling Assistant|calendar.freeBusy
Appointment|Show|Tracking|calendar.tracking
Appointment|Attendees|Invite Attendees|calendar.edit
Appointment|Attendees|Cancel Meeting|calendar.cancel
Appointment|Attendees|Contact Attendees|calendar.contactAttendees
Appointment|Options|Show As|calendar.showAs
Appointment|Options|Reminder|calendar.reminder
Appointment|Options|Recurrence|calendar.edit
Appointment|Options|Time Zones|calendar.timeZones
Appointment|Tags|Categorize|calendar.categorize
Appointment|Tags|Private|calendar.private
Appointment|Tags|High Importance|calendar.importance
Meeting|Respond|Accept|calendar.respond
Meeting|Respond|Tentative|calendar.respond
Meeting|Respond|Decline|calendar.respond
Meeting|Respond|Propose New Time|calendar.propose
Meeting|Respond|Respond with Email|calendar.respondEmail
People|New|New Contact|people.new
People|New|New Contact Group|people.newGroup
People|Delete|Delete|people.delete
People|Communicate|Email|people.email
People|Communicate|Meeting|people.meeting
People|Current View|People|view.people
People|Current View|Business Card|people.viewCards
People|Current View|List|people.viewList
People|Actions|Edit Contact|people.edit
People|Actions|Add to Favorites|people.favorite
People|Actions|Forward Contact|people.forward
People|Actions|Link Contacts|people.link
People|Actions|Merge Duplicates|people.merge
People|Actions|Mail Merge|people.mailMerge
People|Tags|Categorize|people.categorize
People|Tags|Follow Up|people.followUp
People|Tags|Private|people.private
People|Find|Search People|people.search
Tasks|New|New Task|tasks.new
Tasks|Delete|Delete|tasks.delete
Tasks|Manage Task|Mark Complete|tasks.complete
Tasks|Manage Task|Remove from List|tasks.delete
Tasks|Manage Task|Edit Task|tasks.edit
Tasks|Manage Task|Assign Task|tasks.assign
Tasks|Manage Task|Send Status Report|tasks.statusReport
Tasks|Manage Task|Recurrence|tasks.recurrence
Tasks|Follow Up|Today|tasks.edit
Tasks|Follow Up|Tomorrow|tasks.edit
Tasks|Follow Up|This Week|tasks.edit
Tasks|Follow Up|Next Week|tasks.edit
Tasks|Follow Up|No Date|tasks.edit
Tasks|Follow Up|Custom|tasks.edit
Tasks|Current View|Filter Tasks|view.taskFilter
Tasks|Tags|Categorize|tasks.categorize
Tasks|Tags|Private|tasks.private
Tasks|Tags|High Importance|tasks.edit
Tasks|Tags|Low Importance|tasks.edit
Help|Help|Help|help.help
Help|Help|Contact Support|help.support
Help|Help|Feedback|help.feedback
Help|Help|Show Training|help.training
Help|Help|What's New|help.whatsNew
";

/// One catalog line.
pub struct Feature<'a> {
    pub tab: &'a str,
    pub group: &'a str,
    pub name: &'a str,
    pub id: &'a str,
}

pub fn features() -> impl Iterator<Item = Feature<'static>> {
    CATALOG.lines().filter_map(|l| {
        let mut p = l.split('|');
        Some(Feature { tab: p.next()?, group: p.next()?, name: p.next()?, id: p.next()? })
    })
}

/// Compare the catalog with the registry: totals, the percentage implemented, per-tab numbers
/// and what is missing.
pub fn parity(reg: &Registry) -> Value {
    let mut tabs: Vec<(String, usize, usize)> = Vec::new();
    let mut missing = Vec::new();
    let (mut total, mut done) = (0usize, 0usize);
    for f in features() {
        total += 1;
        let have = reg.get(f.id).is_some();
        done += usize::from(have);
        match tabs.iter_mut().find(|t| t.0 == f.tab) {
            Some(t) => {
                t.1 += 1;
                t.2 += usize::from(have);
            }
            None => tabs.push((f.tab.to_string(), 1, usize::from(have))),
        }
        if !have {
            missing.push(json!({"tab": f.tab, "group": f.group, "feature": f.name, "id": f.id}));
        }
    }
    let percent = if total == 0 { 0.0 } else { (done as f64 * 1000.0 / total as f64).round() / 10.0 };
    json!({
        "total": total,
        "implemented": done,
        "percent": percent,
        "commands": reg.all().len(),
        "tabs": tabs.iter().map(|(t, n, d)| json!({"tab": t, "total": n, "implemented": d})).collect::<Vec<_>>(),
        "missing": missing,
    })
}

/// The parity report as Markdown (docs/parity-checklist.md).
pub fn parity_markdown(reg: &Registry) -> String {
    let p = parity(reg);
    let n = |k: &str| p.get(k).and_then(Value::as_u64).unwrap_or(0);
    let mut out = String::from("# Outlook feature parity\n\n");
    out.push_str("> Generated by `cargo xtask parity` from `crates/engine/src/catalog.rs` and the command registry. Do not edit by hand.\n\n");
    out.push_str(&format!(
        "**{} of {} catalog features have a command ({}%).** The registry holds {} commands.\n\n",
        n("implemented"),
        n("total"),
        p.get("percent").and_then(Value::as_f64).unwrap_or(0.0),
        n("commands")
    ));
    out.push_str("A command existing means the behaviour is reachable by id (ribbon, shortcut, CLI, control channel, MCP); how complete it is against the incumbent is tracked in `docs/gaps.md` and the per-area parity checklists.\n\n");
    out.push_str("| Tab | Implemented | Total |\n|---|---|---|\n");
    for t in p.get("tabs").and_then(Value::as_array).into_iter().flatten() {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            t.get("tab").and_then(Value::as_str).unwrap_or(""),
            t.get("implemented").and_then(Value::as_u64).unwrap_or(0),
            t.get("total").and_then(Value::as_u64).unwrap_or(0)
        ));
    }
    out.push_str("\n## Missing\n\n| Tab | Group | Feature | Command id |\n|---|---|---|---|\n");
    for m in p.get("missing").and_then(Value::as_array).into_iter().flatten() {
        let s = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("");
        out.push_str(&format!("| {} | {} | {} | `{}` |\n", s("tab"), s("group"), s("feature"), s("id")));
    }
    out
}
