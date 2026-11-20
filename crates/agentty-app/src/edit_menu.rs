//! The Edit menu's Cut / Copy / Paste / Select All for whatever has the keyboard.
//!
//! The menu's items carry the terminal's actions (their shortcuts are the terminal's). A terminal
//! handles them itself; anything else that has the keyboard — a text field, the file editor, a
//! table, a chat's text, a diff — knows the same commands under its own action, so a command that
//! reaches the window unhandled is passed on under each of those names. Only the view that has the
//! keyboard hears one.

use crate::webview::EditCommand;
use gpui::{Action, App, Window};

/// Runs `command` in the in-app browser page that has the keyboard, else in the focused view.
pub fn perform(command: EditCommand, window: &mut Window, cx: &mut App) {
    if crate::webview::perform_in_page(command) {
        return;
    }
    for action in actions(command) {
        window.dispatch_action(action, cx);
    }
}

fn actions(command: EditCommand) -> Vec<Box<dyn Action>> {
    use crate::text_input as field;
    use crate::workbench::diff_body as diff;
    let mut actions: Vec<Box<dyn Action>> = match command {
        EditCommand::Cut => vec![Box::new(field::Cut)],
        EditCommand::Copy => vec![Box::new(field::Copy), Box::new(diff::DiffCopy), Box::new(crate::workbench::CopyChatSelection)],
        EditCommand::Paste => vec![Box::new(field::Paste), Box::new(crate::workbench::PasteIntoChat)],
        EditCommand::SelectAll => vec![Box::new(field::SelectAll), Box::new(diff::DiffSelectAll)],
    };
    actions.extend(crate::editor::edit_menu_actions(command));
    actions
}
