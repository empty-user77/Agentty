mod backend;
mod keys;
mod view;

pub use view::{
    classify_screen, path_and_place_at, strip_agent_mark, tool_label, AgentStatus, Clear, Copy, NoticeKind, Paste, SelectAll, SubagentRun,
    TerminalEvent, TerminalView,
};
