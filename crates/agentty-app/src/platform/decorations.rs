//! Who draws the window frame, and the resize edges Agentty draws when it is its own frame.
//!
//! macOS draws the traffic lights over Agentty's title bar, and Windows and X11 window managers
//! draw the system frame. Wayland compositors without the decoration protocol (GNOME / mutter,
//! the Ubuntu default) draw no frame at all, and gpui reports server-side decorations there as
//! soon as they were asked for, so the window had no title bar and no edges to move or resize it
//! by. On Wayland Agentty therefore always draws the frame itself.

use gpui::{div, prelude::*, px, AnyElement, CursorStyle, Decorations, MouseButton, ResizeEdge, Window, WindowDecorations};

/// How far into the window an edge picks up a resize drag.
const EDGE: f32 = 5.;
/// The size of the corner squares, which resize in both directions.
const CORNER: f32 = 10.;

/// The decorations to ask for on a window that has Agentty's title bar, for the compositor gpui
/// runs on (`App::compositor_name`).
pub fn requested(compositor: &str) -> Option<WindowDecorations> {
    if cfg!(target_os = "macos") {
        None
    } else if compositor == "Wayland" {
        Some(WindowDecorations::Client)
    } else {
        Some(WindowDecorations::Server)
    }
}

/// True when Agentty draws the window frame itself (title bar, buttons, resize edges).
pub fn client_side(window: &Window) -> bool {
    !cfg!(target_os = "macos") && matches!(window.window_decorations(), Decorations::Client { .. })
}

/// Invisible strips along the window's edges that start a system resize, drawn over everything
/// else; none when the system draws the frame, and none on an edge tiled against the screen
/// (a maximised window has all four tiled).
pub fn resize_edges(window: &Window) -> Vec<AnyElement> {
    let Decorations::Client { tiling } = window.window_decorations() else { return Vec::new() };
    if cfg!(target_os = "macos") {
        return Vec::new();
    }
    let (top, bottom, left, right) = (!tiling.top, !tiling.bottom, !tiling.left, !tiling.right);
    let handle = |id: &'static str, edge: ResizeEdge, cursor: CursorStyle| {
        div().id(id).absolute().occlude().cursor(cursor).on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            window.start_window_resize(edge);
        })
    };
    let mut edges = Vec::new();
    if top {
        edges.push(handle("resize-top", ResizeEdge::Top, CursorStyle::ResizeUpDown).top_0().left_0().w_full().h(px(EDGE)));
    }
    if bottom {
        edges.push(handle("resize-bottom", ResizeEdge::Bottom, CursorStyle::ResizeUpDown).bottom_0().left_0().w_full().h(px(EDGE)));
    }
    if left {
        edges.push(handle("resize-left", ResizeEdge::Left, CursorStyle::ResizeLeftRight).top_0().left_0().h_full().w(px(EDGE)));
    }
    if right {
        edges.push(handle("resize-right", ResizeEdge::Right, CursorStyle::ResizeLeftRight).top_0().right_0().h_full().w(px(EDGE)));
    }
    let corner = |id, edge, cursor| handle(id, edge, cursor).size(px(CORNER));
    if top && left {
        edges.push(corner("resize-top-left", ResizeEdge::TopLeft, CursorStyle::ResizeUpLeftDownRight).top_0().left_0());
    }
    if top && right {
        edges.push(corner("resize-top-right", ResizeEdge::TopRight, CursorStyle::ResizeUpRightDownLeft).top_0().right_0());
    }
    if bottom && left {
        edges.push(corner("resize-bottom-left", ResizeEdge::BottomLeft, CursorStyle::ResizeUpRightDownLeft).bottom_0().left_0());
    }
    if bottom && right {
        edges.push(corner("resize-bottom-right", ResizeEdge::BottomRight, CursorStyle::ResizeUpLeftDownRight).bottom_0().right_0());
    }
    edges.into_iter().map(IntoElement::into_any_element).collect()
}

#[cfg(test)]
mod tests {
    use super::requested;
    use gpui::WindowDecorations;

    #[test]
    fn wayland_draws_its_own_frame() {
        let expected = (!cfg!(target_os = "macos")).then_some(WindowDecorations::Client);
        assert_eq!(requested("Wayland"), expected);
    }

    #[test]
    fn x11_and_windows_keep_the_system_frame() {
        let expected = (!cfg!(target_os = "macos")).then_some(WindowDecorations::Server);
        assert_eq!(requested("X11"), expected);
        assert_eq!(requested(""), expected);
    }
}
