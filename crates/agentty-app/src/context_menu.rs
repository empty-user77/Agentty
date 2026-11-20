//! A right-click menu any view can open: text shown somewhere (an error, a log line, a path, a
//! commit message) is copied from it, and a terminal's Copy / Paste / Select All are there.
//!
//! GPUI's text cannot be selected unless a view builds selection for it. This menu is the way out
//! for everything that does not: `copyable` on an element is one line, needs no state in the view,
//! and works wherever the window's root draws the menu ([`render`]).

use crate::i18n::t;
use crate::ui::{menu_item, popover, TypeScale};
use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, App, ClipboardItem, Global, InteractiveElement, MouseButton, MouseDownEvent, Pixels, Point, SharedString,
    Stateful, Window, WindowId,
};
use std::rc::Rc;

/// What a line of the menu does when it is chosen.
type Run = Rc<dyn Fn(&mut Window, &mut App)>;

/// One line of the menu.
#[derive(Clone)]
pub struct Item {
    label: SharedString,
    enabled: bool,
    run: Run,
}

impl Item {
    pub fn new(label: impl Into<SharedString>, run: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Self { label: label.into(), enabled: true, run: Rc::new(run) }
    }

    /// Shown greyed and without a click while `enabled` is false (Copy with nothing selected).
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// "Copy": puts `text` on the clipboard.
    pub fn copy(text: impl Into<String>, cx: &App) -> Self {
        Self::copy_as(t(cx, "input.copy"), text)
    }

    /// An item that copies `text`, under a label of its own ("Copy all", "Copy path").
    pub fn copy_as(label: impl Into<SharedString>, text: impl Into<String>) -> Self {
        let text: String = text.into();
        Self::new(label, move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
    }
}

struct Open {
    window: WindowId,
    at: Point<Pixels>,
    items: Vec<Item>,
}

#[derive(Default)]
struct State(Option<Open>);

impl Global for State {}

/// Opens the menu at `at` (window coordinates) with `items`; nothing opens for an empty list.
pub fn open(at: Point<Pixels>, items: Vec<Item>, window: &mut Window, cx: &mut App) {
    if items.is_empty() {
        return;
    }
    cx.set_global(State(Some(Open { window: window.window_handle().window_id(), at, items })));
    window.refresh();
}

fn close(window: &mut Window, cx: &mut App) {
    if cx.try_global::<State>().is_some_and(|state| state.0.is_some()) {
        cx.set_global(State(None));
        window.refresh();
    }
}

/// Text that says something worth keeping gets a right-click "Copy": `text` is what is copied
/// (what the element shows, or the whole of what it shows a part of).
pub trait Copyable: Sized {
    fn copyable(self, text: impl Into<SharedString>) -> Self;
    /// [`Copyable::copyable`] with further items under "Copy" ("Copy all").
    fn copyable_with(self, text: impl Into<SharedString>, more: impl Fn(&App) -> Vec<Item> + 'static) -> Self;
}

impl<E: InteractiveElement> Copyable for E {
    fn copyable(self, text: impl Into<SharedString>) -> Self {
        self.copyable_with(text, |_| Vec::new())
    }

    fn copyable_with(self, text: impl Into<SharedString>, more: impl Fn(&App) -> Vec<Item> + 'static) -> Self {
        let text: SharedString = text.into();
        self.on_mouse_down(MouseButton::Right, move |event: &MouseDownEvent, window, cx| {
            if text.trim().is_empty() {
                return;
            }
            cx.stop_propagation();
            let mut items = vec![Item::copy(text.to_string(), cx)];
            items.extend(more(cx));
            open(event.position, items, window, cx);
        })
    }
}

/// The open menu, for the root of `window` to draw over everything else.
pub fn render(window: &Window, cx: &App) -> Option<AnyElement> {
    let open = cx.try_global::<State>()?.0.as_ref()?;
    if open.window != window.window_handle().window_id() {
        return None;
    }
    let mut list = popover().min_w(px(160.)).max_w(px(320.)).on_mouse_down_out(|_, window, cx| close(window, cx));
    for (index, item) in open.items.iter().enumerate() {
        let row: Stateful<gpui::Div> = if item.enabled {
            let run = item.run.clone();
            menu_item(("context-menu-item", index), item.label.clone(), move |_, window, cx| {
                close(window, cx);
                run(window, cx);
            })
        } else {
            div()
                .id(("context-menu-item", index))
                .px_3()
                .py_1()
                .t_body()
                .text_color(crate::theme::hex(crate::theme::Chrome::MUTED))
                .child(item.label.clone())
        };
        list = list.child(row);
    }
    Some(
        gpui::deferred(gpui::anchored().position(open.at).snap_to_window_with_margin(px(8.)).child(list))
            .with_priority(6)
            .into_any_element(),
    )
}
