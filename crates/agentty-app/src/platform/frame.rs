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

    /// Same frame with a new height, keeping the top edge fixed.
    pub fn with_height_from_top(self, height: f64) -> Frame {
        Frame { y: self.y + self.height - height, height, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::Frame;

    #[test]
    fn height_change_keeps_the_top_edge_fixed() {
        let frame = Frame { x: 1124., y: 684., width: 300., height: 200. };
        let taller = frame.with_height_from_top(260.);
        assert_eq!(taller.y + taller.height, frame.y + frame.height);
        assert_eq!(taller.x, frame.x);
    }
}
