# Signature files

On the desktop, select a signature on the left of File > Signatures, then open the **File** tab beside Rich text and HTML. Selecting the tab activates the file source; select **Choose…** to pick its file. The system file picker starts in your home directory. The file appears immediately in Preview; **Refresh Preview** reads any subsequent changes to the file.

The file becomes the source of the selected signature. Assign that named signature to accounts using the existing New messages and Replies and forwards selectors at the bottom of the page. Different accounts can use different file signatures, and new messages and replies can use different signatures. Selecting Rich text or HTML stops using the file and restores the saved signature contents in that format. The path is retained for the next time File is selected; its contents do not overwrite the saved signature.

The file is read again when a new message, reply or forward starts. Editing the file does not change an existing message or replace the signature in a saved draft when it is reopened. Choosing another sender follows the composer's existing rule: replace an unedited signature with the new sender's signature.

Supported files are UTF-8 text (`.txt`, `.text`), HTML (`.html`, `.htm`), and PNG, JPEG or GIF images. Text is limited to 10,000 characters, HTML to 64 KB, and images to 256 KB, matching the existing signature limits. Images are included as inline attachments rather than links to the file. HTML uses MailCraft's existing HTML signature support and its limitations; it does not load other local files or fetch remote resources while reading the signature file.

Plain-text replies remain plain text. If a signature contains pictures, the editor warns that plain-text output omits them and offers **Switch to HTML**. HTML output includes the pictures as inline attachments. Reopened drafts retain their saved images even when the external signature file changes. An empty text file means no signature, without substituting the saved contents.

A plain-text draft retains its pictures for later editing and remembers the chosen text format locally. After reopening, Switch to HTML can still include those saved pictures. Sending in plain text omits the pictures and does not attach the draft's hidden image parts. The warning appears only while the current message still references pictures, not for deleted pictures kept in Undo history.

If the file is missing, unreadable, unsupported, damaged or too large, Preview and the message editor show a warning and use the selected signature's saved contents instead. If those contents are empty, the message starts without a signature.

Paths are stored in this installation's `ui.json`, scoped to the mailbox path and signature name. They are not stored in exported mailbox files. A mailbox opened elsewhere cannot choose a file on that computer as its signature. Renaming a signature keeps its file source; deleting it removes the local file binding. After moving or renaming a mailbox file, select the signature file again. Agents cannot configure signature file paths or open their picker.

Undo and Redo of a signature rename or removal restore its local file binding with that operation. A newly created signature does not inherit the removed signature's binding. A pending file choice is cancelled if its target is renamed, removed or restored by Undo/Redo.

Undoing signature creation detaches its file binding; Redo restores it only with that same signature. The local settings limit reserves slots for bindings Undo or Redo can restore, so restoring a signature cannot make the settings exceed the limit and lose links after restart.

The option is unavailable in the browser build, which cannot retain access to arbitrary local filesystem paths.
