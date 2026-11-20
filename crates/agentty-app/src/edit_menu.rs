//! The Edit menu's Cut / Copy / Paste / Select All for whatever has the keyboard.
//!
//! The menu's items carry the terminal's actions (their shortcuts are the terminal's). A terminal
//! handles them itself; anything else that has the keyboard — a text field, the file editor, a
//! table, a chat's text, a diff — knows the same commands under its own action. A command that
//! reaches the window unhandled is passed on under the action of the innermost such view around
//! the focus, and only that one: a field inside a table inside the editor is the field's alone.

use crate::webview::EditCommand;
use gpui::{Action, App, InteractiveElement, KeyContext, Window};

/// Runs `command` in the in-app browser page that has the keyboard, else in the focused view.
pub fn perform(command: EditCommand, window: &mut Window, cx: &mut App) {
    if crate::webview::perform_in_page(command) {
        return;
    }
    let contexts = window.context_stack();
    if let Some(action) = contexts.iter().rev().find_map(|context| action_in(context, command)).flatten() {
        window.dispatch_action(action, cx);
    }
}

/// A window's root takes the menu's commands that nothing in it handled (a terminal handles its
/// own): every window that holds text fields has this on its root.
pub fn handled<E: InteractiveElement>(root: E) -> E {
    use crate::terminal::{Copy, Cut, Paste, SelectAll};
    root.on_action(|_: &Cut, window, cx| perform(EditCommand::Cut, window, cx))
        .on_action(|_: &Copy, window, cx| perform(EditCommand::Copy, window, cx))
        .on_action(|_: &Paste, window, cx| perform(EditCommand::Paste, window, cx))
        .on_action(|_: &SelectAll, window, cx| perform(EditCommand::SelectAll, window, cx))
}

/// `None`: this is not a view that knows the menu's commands, look further out. `Some(None)`: it
/// is one, and has nothing for this command (a chat's text is not cut) — nothing further out acts.
fn action_in(context: &KeyContext, command: EditCommand) -> Option<Option<Box<dyn Action>>> {
    use crate::text_input as field;
    use crate::workbench::diff_body as diff;
    let action: Option<Box<dyn Action>> = if context.contains("TextInput") {
        Some(match command {
            EditCommand::Cut => Box::new(field::Cut),
            EditCommand::Copy => Box::new(field::Copy),
            EditCommand::Paste => Box::new(field::Paste),
            EditCommand::SelectAll => Box::new(field::SelectAll),
        })
    } else if context.contains(crate::editor::TABLE_CONTEXT) {
        Some(crate::editor::edit_menu_action(command, true))
    } else if context.contains(crate::editor::CONTEXT) {
        Some(crate::editor::edit_menu_action(command, false))
    } else if context.contains(diff::CONTEXT) {
        match command {
            EditCommand::Copy => Some(Box::new(diff::DiffCopy)),
            EditCommand::SelectAll => Some(Box::new(diff::DiffSelectAll)),
            EditCommand::Cut | EditCommand::Paste => None,
        }
    } else if context.contains("ChatReading") {
        match command {
            EditCommand::Copy => Some(Box::new(crate::workbench::CopyChatSelection)),
            EditCommand::Paste => Some(Box::new(crate::workbench::PasteIntoChat)),
            EditCommand::Cut | EditCommand::SelectAll => None,
        }
    } else {
        return None;
    };
    Some(action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(name: &'static str) -> KeyContext {
        let mut context = KeyContext::default();
        context.add(name);
        context
    }

    /// A field inside the table inside the editor: the command is the field's, and neither the
    /// table nor the hidden text behind it acts on it too.
    #[test]
    fn the_innermost_view_takes_the_command_alone() {
        let stack = [context("Workbench"), context("CodeEditor"), context("TableGrid"), context("TextInput")];
        let pick = |stack: &[KeyContext], command| stack.iter().rev().find_map(|c| action_in(c, command)).flatten();
        let name = |action: Option<Box<dyn Action>>| action.map(|a| a.name().to_string());
        assert_eq!(name(pick(&stack, EditCommand::Paste)), Some(crate::text_input::Paste.name().to_string()));
        assert_ne!(name(pick(&stack[..3], EditCommand::Paste)), name(pick(&stack[..2], EditCommand::Paste)));
        // A diff has no Paste, and the views around it are not asked.
        let diff = [context("Workbench"), context("CodeEditor"), context(crate::workbench::diff_body::CONTEXT)];
        assert_eq!(name(pick(&diff, EditCommand::Paste)), None);
        assert!(pick(&diff, EditCommand::Copy).is_some());
        // Nothing that knows the commands has the keyboard.
        assert!(pick(&[context("Workbench"), context("Sidebar")], EditCommand::Copy).is_none());
    }
}
