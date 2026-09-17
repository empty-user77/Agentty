mod backend;
mod keys;
mod view;

pub use view::{classify_screen, AgentStatus, Clear, Copy, NoticeKind, Paste, SelectAll, TerminalEvent, TerminalView};
