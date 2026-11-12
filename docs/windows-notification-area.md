# Windows notification area

The Windows desktop build keeps receiving mail when its main window is closed or minimized. Click the notification-area icon, or choose Open from its menu, to restore the window. Choose Quit MailCraft to exit; the existing questions about unsaved drafts and pending saves still apply. If Windows cannot create the icon, the normal visible-window and close behavior remains available.

`mailcraft --start-in-tray` creates the main window hidden and keeps the icon available. To start this way at login, the owner can create a Windows startup shortcut with that argument. The application does not register itself for startup automatically.

The opt-in control channel can use `ui.minimize` and `ui.closeWindow` to exercise the native window actions, `ui.focus` to restore it, and `app.quit` for an explicit exit. Closing and quitting remain owner-script actions, unavailable to agents.
