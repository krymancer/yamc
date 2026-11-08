//! The macOS menu bar (and its shortcuts). Window-level actions are handled by `app::Yamc`.

use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::*;

actions!(
    yamc,
    [
        Quit,
        Hide,
        HideOthers,
        ShowAll,
        NewMessage,
        OpenSettings,
        FocusSearch,
        GoInbox,
        GoDrafts,
        GoSent,
        Archive,
        Trash,
        MarkUnread,
        MarkRead,
        Reply,
        ReplyAll,
        ToggleSidebar,
        ShowKeys,
        Minimize,
        Zoom,
        CheckForUpdates,
    ]
);

pub fn install(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-n", NewMessage, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-backspace", Trash, None),
        KeyBinding::new("cmd-m", Minimize, None),
    ]);
    cx.set_menus([
        Menu::new("yamc").items([
            MenuItem::action("Check for Updates…", CheckForUpdates),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Hide yamc", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit yamc", Quit),
        ]),
        Menu::new("File").items([MenuItem::action("New Message", NewMessage)]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            MenuItem::action("Find", FocusSearch),
        ]),
        Menu::new("Message").items([
            MenuItem::action("Reply", Reply),
            MenuItem::action("Reply All", ReplyAll),
            MenuItem::separator(),
            MenuItem::action("Archive", Archive),
            MenuItem::action("Move to Trash", Trash),
            MenuItem::separator(),
            MenuItem::action("Mark as Unread", MarkUnread),
            MenuItem::action("Mark as Read", MarkRead),
        ]),
        Menu::new("View").items([
            MenuItem::action("Inbox", GoInbox),
            MenuItem::action("Sent", GoSent),
            MenuItem::action("Drafts", GoDrafts),
            MenuItem::separator(),
            MenuItem::action("Toggle Sidebar", ToggleSidebar),
            MenuItem::action("Keyboard Shortcuts", ShowKeys),
        ]),
        Menu::new("Window").items([MenuItem::action("Minimize", Minimize), MenuItem::action("Zoom", Zoom)]),
    ]);
}
