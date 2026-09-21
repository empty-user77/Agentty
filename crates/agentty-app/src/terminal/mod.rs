mod backend;
mod keys;
mod view;

pub use view::{
    classify_screen, strip_agent_mark, tool_label, AgentStatus, Clear, Copy, NoticeKind, Paste, SelectAll, TerminalEvent, TerminalView,
};
