//! Settings → Shortcuts: the preset, every shortcut with the keys it has now, and the dialog
//! that takes a new one from the keyboard. The shortcuts themselves are `crate::shortcuts`.

use super::settings_page::{row, section};
use super::Workbench;
use crate::i18n::{t, tf};
use crate::shortcuts::{self, chord, Preset};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, TypeScale as _};
use gpui::{div, prelude::*, px, App, ClickEvent, Context, Div, Global, SharedString, Subscription, Window};

/// Shortcuts that stay as they are, listed for reading: (group key, [(name key, keys)]). A group
/// that also has changeable shortcuts shows these after them.
const FIXED: &[(&str, &[(&str, &str)])] = &[
    (shortcuts::catalog::NAVIGATE, &[("shortcuts.tab_n", "⌃1 … ⌃9"), ("shortcuts.workspace_n", "⌘1 … ⌘9")]),
    (
        "shortcuts.group.terminal",
        &[
            ("shortcuts.copy_paste", "⌘C  /  ⌘V"),
            ("shortcuts.select_all", "⌘A"),
            ("shortcuts.clear", "⌘K"),
            ("shortcuts.find", "⌘F"),
            ("shortcuts.zoom", "⌘=  /  ⌘-  /  ⌘0"),
            ("shortcuts.open_link", "⌘ Click"),
        ],
    ),
    (
        EDITOR_SHORTCUTS,
        &[
            ("editor.save", "⌘S"),
            ("editor.format", "⇧⌥F"),
            ("editor.shortcut_undo_redo", "⌘Z  /  ⇧⌘Z"),
            ("find.replace", "⌥⌘F"),
            ("editor.shortcut_organize_imports", "⇧⌥O"),
            ("editor.shortcut_comment", "⌘/"),
            ("editor.shortcut_move_lines", "⌥↑  /  ⌥↓"),
            ("editor.shortcut_duplicate_lines", "⇧⌥↑  /  ⇧⌥↓"),
            ("editor.shortcut_delete_lines", "⇧⌘K"),
            ("editor.shortcut_select_line", "⌘L"),
            ("editor.shortcut_next_occurrence", "⌘D  /  ⇧⌘L"),
            ("editor.shortcut_add_cursor", "⌥ Click  /  ⌥⌘↑  /  ⌥⌘↓"),
            ("editor.shortcut_go_to_bracket", "⇧⌘\\"),
            ("editor.shortcut_fold", "⌥⌘[  /  ⌥⌘]"),
            ("editor.shortcut_indent_lines", "⌘]  /  ⌘["),
            ("editor.shortcut_insert_line", "⌘↩  /  ⇧⌘↩"),
            ("editor.shortcut_delete_to_start", "⌘⌫"),
            ("editor.shortcut_delete_word", "⌥⌫  /  ⌥⌦"),
            ("editor.close_file", "⌘W"),
        ],
    ),
];

/// The file editor's group: a text field, so its shortcuts use plain Ctrl on Windows and Linux.
const EDITOR_SHORTCUTS: &str = "editor.shortcuts_group";

/// The dialog that waits for a key press, while it is open. A global of its own, with the
/// keystroke interceptor that feeds it: nothing else in the app knows a shortcut is being taken.
struct Recorder {
    command: &'static shortcuts::Command,
    /// The chord pressed last, as this system reports it.
    chord: Option<String>,
    refusal: Option<chord::Refusal>,
    /// The commands the chord would be taken from.
    losing: Vec<&'static shortcuts::Command>,
    reserved: Vec<String>,
    _keys: Subscription,
}

impl Global for Recorder {}

impl Recorder {
    fn ready(&self) -> bool {
        self.chord.is_some() && self.refusal.is_none()
    }
}

/// "Reset all to default?", while it is asked.
struct ResetAsked;

impl Global for ResetAsked {}

fn close_reset(cx: &mut App) {
    if cx.has_global::<ResetAsked>() {
        cx.remove_global::<ResetAsked>();
        cx.refresh_windows();
    }
}

fn open_recorder(id: &'static str, cx: &mut App) {
    let Some(command) = shortcuts::catalog::command(id) else { return };
    // Every key goes to the dialog while it is open, before any shortcut or field sees it.
    let keys = cx.intercept_keystrokes(|event, _, cx| {
        if !cx.has_global::<Recorder>() {
            return;
        }
        cx.stop_propagation();
        let keystroke = &event.keystroke;
        let plain = !keystroke.modifiers.modified();
        match keystroke.key.as_str() {
            "escape" if plain => return cx.defer(close_recorder),
            "enter" if plain && cx.global::<Recorder>().ready() => return cx.defer(save_recorder),
            _ => {}
        }
        let Some(pressed) = chord::of_keystroke(keystroke) else { return };
        let settings = crate::settings::settings(cx).shortcuts.clone();
        cx.update_global::<Recorder, _>(|recorder, _| {
            recorder.refusal = chord::judge(&keystroke.modifiers, &keystroke.key, &recorder.reserved).err();
            recorder.losing =
                if recorder.refusal.is_none() { shortcuts::conflicts(recorder.command.id, &pressed, &settings) } else { Vec::new() };
            recorder.chord = Some(pressed);
        });
        cx.refresh_windows();
    });
    cx.set_global(Recorder { command, chord: None, refusal: None, losing: Vec::new(), reserved: chord::reserved_here(), _keys: keys });
    cx.refresh_windows();
}

fn close_recorder(cx: &mut App) {
    if cx.has_global::<Recorder>() {
        cx.remove_global::<Recorder>();
        cx.refresh_windows();
    }
}

fn save_recorder(cx: &mut App) {
    let Some(recorder) = cx.try_global::<Recorder>().filter(|recorder| recorder.ready()) else { return };
    let (id, pressed) = (recorder.command.id, recorder.chord.clone().unwrap_or_default());
    cx.remove_global::<Recorder>();
    shortcuts::assign(id, &pressed, cx);
}

/// One key as a keycap.
fn keycap(label: impl Into<SharedString>) -> Div {
    div()
        .px_2()
        .py_0p5()
        .rounded_md()
        .border_1()
        .border_color(hex(Chrome::OVERLAY_BORDER))
        .bg(hex(0x2a2a2a))
        .when(crate::theme::is_light(), crate::ui::outlined)
        .t_small()
        .text_color(hex(Chrome::BRIGHT))
        .child(label.into())
}

/// A changeable shortcut's keys as a button that opens the dialog, and a way back to the
/// preset's keys once the user has changed it.
fn keys_control(command: &'static shortcuts::Command, cx: &App) -> Div {
    let id = command.id;
    let keys = shortcuts::shown(id, cx);
    let changed = crate::settings::settings(cx).shortcuts.custom_for(id).is_some();
    let caps = if keys.is_empty() {
        vec![div().px_2().py_0p5().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "keys.none").to_string())]
    } else {
        keys.into_iter().map(keycap).collect()
    };
    div()
        .flex()
        .items_center()
        .gap_1()
        .when(changed, |d| {
            d.child(
                div()
                    .id(SharedString::from(format!("keys-reset-{id}")))
                    .size(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex_alpha(Chrome::BRIGHT, 0.1)))
                    .tooltip(crate::ui::Tooltip::text(t(cx, "keys.reset").to_string(), None))
                    .on_click(move |_: &ClickEvent, _, cx| shortcuts::reset(id, cx))
                    .child(icon("undo-2", IconSize::INLINE, hex(Chrome::MUTED))),
            )
        })
        .child(
            div()
                .id(SharedString::from(format!("keys-{id}")))
                .flex()
                .items_center()
                .gap_1()
                .p_0p5()
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(hex_alpha(Chrome::ACCENT, 0.25)))
                .on_click(move |_: &ClickEvent, _, cx| open_recorder(id, cx))
                .children(caps),
        )
}

fn fixed_row(group: &str, name: &'static str, keys: &str, cx: &App) -> Div {
    let shown = if group == EDITOR_SHORTCUTS { crate::keymap::display_in_text(keys) } else { crate::keymap::display(keys) };
    row(t(cx, name), div().p_0p5().child(keycap(shown.into_owned())))
}

/// The preset to start from, and the way back to it once something was changed.
fn preset_card(cx: &App) -> Div {
    let settings = &crate::settings::settings(cx).shortcuts;
    let presets = div().flex().gap_1().children(Preset::ALL.into_iter().map(|preset| {
        crate::ui::chip(
            SharedString::from(format!("keys-preset-{}", preset.name())),
            preset.name(),
            settings.preset == preset,
            move |_, _, cx| shortcuts::set_preset(preset, cx),
        )
    }));
    let control = div()
        .flex()
        .items_center()
        .gap_2()
        // Always there, so it can be found before it is needed; it asks first.
        .child(
            crate::ui::action_button_with_icon("keys-reset-all", "undo-2", t(cx, "keys.reset_all").to_string(), |_, _, cx| {
                if !shortcuts::is_default(&crate::settings::settings(cx).shortcuts) {
                    cx.set_global(ResetAsked);
                    cx.refresh_windows();
                }
            })
            .when(shortcuts::is_default(settings), |d| d.opacity(0.4)),
        )
        .child(presets);
    section(t(cx, "keys.preset")).child(row(t(cx, "keys.preset_start"), control))
}

/// The dialog: the command's name, the keys pressed (or the invitation to press some), what
/// saving would take from another command, and the ways out.
fn recorder_dialog(window: &Window, cx: &App) -> Option<impl IntoElement> {
    let recorder = cx.try_global::<Recorder>()?;
    let pressed = match &recorder.chord {
        Some(pressed) => div().flex().justify_center().child(keycap(chord::pressed_label(pressed)).px_3().py_1().t_title()),
        None => div().flex().justify_center().t_body().text_color(hex(Chrome::MUTED)).child(t(cx, "keys.press").to_string()),
    };
    let note = match (recorder.refusal, recorder.losing.first()) {
        (Some(refusal), _) => Some(t(cx, refusal.message()).to_string()),
        (None, Some(_)) => {
            let names: Vec<&str> = recorder.losing.iter().map(|command| t(cx, command.label)).collect();
            Some(tf(cx, "keys.takes_from", &[("name", &names.join(", "))]))
        }
        _ => None,
    };
    let ready = recorder.ready();
    let id = recorder.command.id;
    let changed = crate::settings::settings(cx).shortcuts.custom_for(id).is_some();
    let button = |id: &'static str, label: &str| div().id(id).px_3().py_1p5().rounded_md().t_body().child(label.to_string());
    let dialog = div()
        .id("keys-dialog")
        .w(px(480.))
        .p_5()
        .flex()
        .flex_col()
        .gap_4()
        .rounded_xl()
        .bg(hex(Chrome::OVERLAY))
        .border_1()
        .border_color(hex(Chrome::OVERLAY_BORDER))
        .shadow_lg()
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(
            div()
                .t_title()
                .font_weight(crate::theme::EMPHASIS)
                .text_color(hex(Chrome::BRIGHT))
                .child(t(cx, recorder.command.label).to_string()),
        )
        .child(
            div()
                .h(px(72.))
                .flex()
                .flex_col()
                .justify_center()
                .rounded_md()
                .bg(hex(Chrome::PANEL))
                .border_1()
                .border_color(hex(if ready { Chrome::ACCENT } else { Chrome::BORDER }))
                .child(pressed),
        )
        .when_some(note, |d, note| d.child(div().t_small().text_color(hex(Chrome::WARNING)).child(note)))
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                // No shortcut at all: the way to free a key without giving it to something else.
                .child(
                    button("keys-clear", t(cx, "keys.clear"))
                        .cursor_pointer()
                        .text_color(hex(Chrome::MUTED))
                        .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                        .on_click(move |_: &ClickEvent, _, cx| {
                            cx.remove_global::<Recorder>();
                            shortcuts::assign(id, "", cx);
                        }),
                )
                // Back to the preset's keys for this command.
                .child(
                    button("keys-default", t(cx, "keys.reset_default"))
                        .text_color(hex(Chrome::MUTED))
                        .when(!changed, |d| d.opacity(0.4))
                        .when(changed, |d| {
                            d.cursor_pointer().hover(|s| s.text_color(hex(Chrome::BRIGHT))).on_click(move |_: &ClickEvent, _, cx| {
                                cx.remove_global::<Recorder>();
                                shortcuts::reset(id, cx);
                            })
                        }),
                )
                .child(div().flex_1())
                .child(
                    button("keys-cancel", t(cx, "confirm.cancel"))
                        .cursor_pointer()
                        .bg(hex(0x2d2d30))
                        .when(crate::theme::is_light(), crate::ui::outlined)
                        .text_color(hex(Chrome::BRIGHT))
                        .hover(|s| s.opacity(0.85))
                        .on_click(|_: &ClickEvent, _, cx| close_recorder(cx)),
                )
                .child(
                    button("keys-save", t(cx, "save"))
                        .bg(hex(Chrome::ACCENT))
                        .text_color(hex(Chrome::ON_ACCENT))
                        .when(!ready, |d| d.opacity(0.4))
                        .when(ready, |d| d.cursor_pointer().hover(|s| s.opacity(0.85)).on_click(|_: &ClickEvent, _, cx| save_recorder(cx))),
                ),
        );
    Some(modal(dialog, window))
}

/// A dialog over the whole window, from wherever the settings page is drawn.
fn modal(dialog: impl IntoElement, window: &Window) -> impl IntoElement {
    let viewport = window.viewport_size();
    let overlay = div()
        .id("keys-overlay")
        .w(viewport.width)
        .h(viewport.height)
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::hsla(0., 0., 0., 0.45))
        .occlude()
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(dialog);
    gpui::deferred(gpui::anchored().position(gpui::point(px(0.), px(0.))).child(overlay)).with_priority(4)
}

/// The question before everything goes back to default: it takes every key the user chose.
fn reset_dialog(window: &Window, cx: &App) -> Option<impl IntoElement> {
    cx.try_global::<ResetAsked>()?;
    let button =
        |id: &'static str, label: &str| div().id(id).px_3().py_1p5().rounded_md().t_body().cursor_pointer().child(label.to_string());
    let dialog = div()
        .id("keys-reset-dialog")
        .w(px(400.))
        .p_5()
        .flex()
        .flex_col()
        .gap_3()
        .rounded_xl()
        .bg(hex(Chrome::OVERLAY))
        .border_1()
        .border_color(hex(Chrome::OVERLAY_BORDER))
        .shadow_lg()
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(
            div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "keys.reset_all").to_string()),
        )
        .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(t(cx, "keys.reset_all_body").to_string()))
        .child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    button("keys-reset-cancel", t(cx, "confirm.cancel"))
                        .bg(hex(0x2d2d30))
                        .when(crate::theme::is_light(), crate::ui::outlined)
                        .text_color(hex(Chrome::BRIGHT))
                        .hover(|s| s.opacity(0.85))
                        .on_click(|_: &ClickEvent, _, cx| close_reset(cx)),
                )
                .child(
                    button("keys-reset-confirm", t(cx, "keys.reset_all_confirm"))
                        .bg(hex(Chrome::ACCENT))
                        .text_color(hex(Chrome::ON_ACCENT))
                        .hover(|s| s.opacity(0.85))
                        .on_click(|_: &ClickEvent, _, cx| {
                            cx.remove_global::<ResetAsked>();
                            shortcuts::reset_all(cx);
                        }),
                ),
        );
    Some(modal(dialog, window))
}

impl Workbench {
    pub(super) fn render_shortcut_settings(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let mut list = div().flex().flex_col().child(preset_card(cx));
        let fixed = |group: &str| FIXED.iter().find(|(name, _)| *name == group).map(|(_, rows)| *rows).unwrap_or(&[]);
        for group in shortcuts::GROUPS {
            let mut block = section(t(cx, group));
            for command in shortcuts::COMMANDS.iter().filter(|command| command.group == group && command.available()) {
                block = block.child(row(t(cx, command.label), keys_control(command, cx)));
            }
            for (name, keys) in fixed(group) {
                block = block.child(fixed_row(group, name, keys, cx));
            }
            list = list.child(block);
        }
        for (group, rows) in FIXED.iter().filter(|(group, _)| !shortcuts::GROUPS.contains(group)) {
            let mut block = section(t(cx, group));
            for (name, keys) in rows.iter() {
                block = block.child(fixed_row(group, name, keys, cx));
            }
            list = list.child(block);
        }
        list.children(recorder_dialog(window, cx)).children(reset_dialog(window, cx))
    }
}
