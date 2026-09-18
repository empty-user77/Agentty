//! Window frames in screen coordinates (origin at the bottom-left of the primary screen).

/// A rectangle in AppKit screen coordinates (origin at the bottom-left of the primary screen).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    #[cfg(target_os = "macos")]
    pub(crate) fn ns(self) -> cocoa::foundation::NSRect {
        use cocoa::foundation::{NSPoint, NSRect, NSSize};
        NSRect::new(NSPoint::new(self.x, self.y), NSSize::new(self.width, self.height))
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn from_ns(rect: cocoa::foundation::NSRect) -> Self {
        Self { x: rect.origin.x, y: rect.origin.y, width: rect.size.width, height: rect.size.height }
    }

    /// A `width`×`height` frame in the top-right corner of `area`, `margin` points from both edges.
    pub fn top_right(area: Frame, width: f64, height: f64, margin: f64) -> Frame {
        Frame { x: area.x + area.width - width - margin, y: area.y + area.height - height - margin, width, height }
    }

    /// Same frame with a new height, keeping the top edge fixed.
    pub fn with_height_from_top(self, height: f64) -> Frame {
        Frame { y: self.y + self.height - height, height, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::Frame;

    #[test]
    fn anchors_to_top_right() {
        let area = Frame { x: 0., y: 80., width: 1440., height: 820. };
        let mini = Frame::top_right(area, 300., 200., 16.);
        assert_eq!((mini.x, mini.y), (1124., 684.));
        let taller = mini.with_height_from_top(260.);
        assert_eq!(taller.y + taller.height, mini.y + mini.height);
    }
}
