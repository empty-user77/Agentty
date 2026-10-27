//! The subagents view as an office: each running subagent sits at a desk and says what it is doing
//! in a speech bubble; finished ones wait in the break room with the first line of their report;
//! the session's own agent works at the console at the bottom. More than a floor's worth of
//! subagents stack up as more floors. Clicking someone opens their log beside the office.
//!
//! The pixel art is drawn from small character maps on a canvas; words, buttons and bubbles are
//! ordinary text on top, so they stay sharp and translated. The office keeps its own colours in
//! both themes, like a picture would.

use super::agent_panel::AgentPanel;
use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::ui::{icon, TypeScale};
use agentty_bridge::claude::SubagentFile;
use agentty_bridge::subagent_trace::{ToolKind, Trace};
use gpui::{canvas, div, point, prelude::*, px, AnyElement, Bounds, ClickEvent, Context, Hsla, Pixels, Point, SharedString, Window};

/// Screen pixels per sprite pixel.
const PX: f32 = 3.;
/// Desks on a floor (two rows of four).
const DESKS_PER_FLOOR: usize = 8;
/// Desks in a row.
const ROW: usize = 4;
/// A desk's drawing, in sprite pixels.
const DESK_W: f32 = 44.;
const DESK_H: f32 = 34.;
/// A standing agent in the break room, in sprite pixels.
const STAND_W: f32 = 14.;
const STAND_H: f32 = 18.;

fn rgb(value: u32) -> Hsla {
    gpui::rgb(value).into()
}

// The office's own colours.
const WALL: u32 = 0x2b2d3a;
const WALL_TRIM: u32 = 0x3a3d4f;
const FLOOR_A: u32 = 0x262833;
const FLOOR_B: u32 = 0x2d303c;
const LOUNGE_FLOOR: u32 = 0x3b3040;
const INK: u32 = 0x14151c;
const PAPER: u32 = 0xf4f1e8;
const PAPER_INK: u32 = 0x1c1d26;
const LABEL: u32 = 0xc9cbd6;
const LABEL_DIM: u32 = 0x8a8d9c;
const WOOD: u32 = 0x9a6a3e;
const WOOD_TOP: u32 = 0xb98552;
const WOOD_DARK: u32 = 0x6d4727;
const METAL: u32 = 0x4a4f5e;
const METAL_DARK: u32 = 0x343845;
const CHAIR: u32 = 0x2f3340;
const SCREEN_OFF: u32 = 0x1d2430;
const LED: u32 = 0x4ade80;
const CLAUDE: u32 = 0xd97757;
const STOP: u32 = 0xf85149;

/// Colours agents are told apart by.
const AGENT_COLORS: [u32; 8] = [0x4f8ef7, 0x3fb950, 0xa371f7, 0xf0883e, 0xdb61a2, 0x39c5bb, 0xe3b341, 0x8b9cf7];

fn agent_color(id: &str) -> u32 {
    let hash = id.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    AGENT_COLORS[hash as usize % AGENT_COLORS.len()]
}

/// What a monitor shows for the kind of work going on.
fn screen_color(kind: Option<ToolKind>) -> u32 {
    match kind {
        Some(ToolKind::Read) => 0x3b82f6,
        Some(ToolKind::Edit) => 0x22c55e,
        Some(ToolKind::Run) => 0xf59e0b,
        Some(ToolKind::Web) => 0xa855f7,
        Some(ToolKind::Agent) => 0xec4899,
        _ => 0x64748b,
    }
}

/// Darker by `amount` (0..1), for shading.
fn shade(color: u32, amount: f32) -> u32 {
    let channel = |shift: u32| (((color >> shift) & 0xff) as f32 * (1. - amount)) as u32;
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

/// An agent seated at a desk, seen over it. `b` body, `l` its light, `B` its shade, `k` outline,
/// `h` headset band, `c` ear cups, `w`/`e` eyes, `p` cheeks, `m` mouth.
const SEATED: [&str; 15] = [
    ".....hhhhhh.....",
    "....h......h....",
    "...h.kkkkkk.h...",
    "..ckkbbbbbbkkc..",
    "..ckbllbbbbbkc..",
    "..ckbwwbbwwbkc..",
    "..ckbwebbwebkc..",
    "...kbpbbbbpbk...",
    "...kbbbmmbbbk...",
    "....kbbbbbbk....",
    "...kBBBBBBBBk...",
    "..kBBBBBBBBBBk..",
    "..kBBBBBBBBBBk..",
    "..kBBBBBBBBBBk..",
    "..kBBBBBBBBBBk..",
];

/// The same agent standing with a mug, for the break room.
const STANDING: [&str; 18] = [
    "...kkkkkkkk...",
    "..kbbbbbbbbk..",
    "..kbllbbbbbk..",
    "..kbwwbbwwbk..",
    "..kbwebbwebk..",
    "..kpbbbbbbpk..",
    "..kbbbmmbbbk..",
    "...kbbbbbbk...",
    "..kBBBBBBBBk..",
    ".kBBBBBBBBBBk.",
    ".kBBBBBBBBBBuu",
    ".kBBBBBBBBBBuu",
    ".kBBBBBBBBBBk.",
    "..kBBBBBBBBk..",
    "..kBBk..kBBk..",
    "..kBBk..kBBk..",
    "..kkkk..kkkk..",
    "..............",
];

/// Paints `rows` at (`x`, `y`) in sprite pixels, each character through `color`.
fn paint_rows(window: &mut Window, origin: Point<Pixels>, x: f32, y: f32, rows: &[&str], color: &dyn Fn(char) -> Option<Hsla>) {
    for (r, row) in rows.iter().enumerate() {
        let chars: Vec<char> = row.chars().collect();
        let mut c = 0;
        while c < chars.len() {
            let (ch, start) = (chars[c], c);
            while c < chars.len() && chars[c] == ch {
                c += 1;
            }
            if let Some(fill) = color(ch) {
                rect(window, origin, x + start as f32, y + r as f32, (c - start) as f32, 1., fill);
            }
        }
    }
}

fn rect(window: &mut Window, origin: Point<Pixels>, x: f32, y: f32, w: f32, h: f32, color: Hsla) {
    window.paint_quad(gpui::fill(
        Bounds::new(point(origin.x + px(x * PX), origin.y + px(y * PX)), gpui::size(px(w * PX), px(h * PX))),
        color,
    ));
}

/// A see-through black, for shadows.
fn shadow(alpha: f32) -> Hsla {
    Hsla { h: 0., s: 0., l: 0., a: alpha }
}

/// A colour at `alpha`, for glows.
fn glow(color: u32, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..rgb(color) }
}

/// Lighter by `amount` (0..1), for highlights.
fn light(color: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let c = ((color >> shift) & 0xff) as f32;
        (c + (255. - c) * amount) as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn agent_palette(body: u32) -> impl Fn(char) -> Option<Hsla> {
    move |ch| match ch {
        'b' => Some(rgb(body)),
        'l' => Some(rgb(light(body, 0.45))),
        'B' => Some(rgb(shade(body, 0.3))),
        'k' => Some(rgb(INK)),
        'h' => Some(rgb(0x1f2029)),
        'c' => Some(rgb(0x3b3e4c)),
        'w' => Some(rgb(0xffffff)),
        'e' => Some(rgb(INK)),
        'p' => Some(rgb(0xff8aa3)),
        'm' => Some(rgb(shade(body, 0.6))),
        'u' => Some(rgb(0xece6d4)),
        _ => None,
    }
}

/// Checkered tiles filling `width` × `height` screen pixels.
fn paint_tiles(window: &mut Window, origin: Point<Pixels>, width: f32, height: f32, a: u32, b: u32) {
    let tile = 8.;
    let (columns, rows) = ((width / PX / tile).ceil() as i32, (height / PX / tile).ceil() as i32);
    rect(window, origin, 0., 0., width / PX, height / PX, rgb(a));
    for row in 0..rows {
        for column in 0..columns {
            if (row + column) % 2 == 0 {
                rect(window, origin, column as f32 * tile, row as f32 * tile, tile, tile, rgb(b));
            }
        }
    }
}

/// One desk on a floor, as painted.
#[derive(Clone)]
struct DeskScene {
    /// The agent at it: body colour, the kind of work on its screen, whether it is being stopped.
    agent: Option<(u32, Option<ToolKind>, bool)>,
    /// Animation frame (moves the agent and the lines on its screen).
    frame: u64,
    /// Which desk this is, so desks are not all alike.
    seat: usize,
}

fn paint_desk(window: &mut Window, origin: Point<Pixels>, scene: &DeskScene) {
    let working = scene.agent.is_some_and(|(_, _, stopping)| !stopping);
    let bob = working && !scene.frame.is_multiple_of(2);
    // Shadow on the floor.
    rect(window, origin, 3., 31., 40., 3., shadow(0.35));
    // The chair, behind whoever sits there.
    rect(window, origin, 18., 8., 10., 14., rgb(CHAIR));
    rect(window, origin, 19., 7., 8., 1., rgb(CHAIR));
    rect(window, origin, 19., 9., 1., 11., rgb(light(CHAIR, 0.15)));
    if let Some((body, _, _)) = scene.agent {
        let palette = agent_palette(body);
        paint_rows(window, origin, 15., 6. + if bob { 1. } else { 0. }, &SEATED, &palette);
    }
    // The desk: a lit edge, the top, the front with two drawers.
    rect(window, origin, 1., 20., 42., 1., rgb(light(WOOD_TOP, 0.25)));
    rect(window, origin, 1., 21., 42., 2., rgb(WOOD_TOP));
    rect(window, origin, 1., 23., 42., 8., rgb(WOOD));
    rect(window, origin, 1., 23., 42., 1., rgb(shade(WOOD, 0.25)));
    for x in [4., 30.] {
        rect(window, origin, x, 25., 10., 5., rgb(shade(WOOD, 0.12)));
        rect(window, origin, x, 25., 10., 1., rgb(light(WOOD, 0.12)));
        rect(window, origin, x + 4., 27., 2., 1., rgb(0xd9c08a));
    }
    rect(window, origin, 1., 31., 2., 3., rgb(WOOD_DARK));
    rect(window, origin, 41., 31., 2., 3., rgb(WOOD_DARK));
    // A keyboard, and hands typing on it while it works.
    rect(window, origin, 17., 19., 11., 2., rgb(0x5b6070));
    rect(window, origin, 17., 19., 11., 1., rgb(0x7a8091));
    if let Some((body, _, _)) = scene.agent {
        let lift = |up: bool| if up && working { 18. } else { 19. };
        rect(window, origin, 18., lift(scene.frame.is_multiple_of(2)), 2., 1., rgb(body));
        rect(window, origin, 25., lift(!scene.frame.is_multiple_of(2)), 2., 1., rgb(body));
    }
    // The monitor, to the left: a lit bezel, a glowing picture, a stand.
    let screen = match scene.agent {
        Some((_, kind, false)) => screen_color(kind),
        Some((_, _, true)) => STOP,
        None => SCREEN_OFF,
    };
    if scene.agent.is_some() {
        rect(window, origin, 1., 7., 16., 14., glow(screen, 0.12));
    }
    rect(window, origin, 2., 8., 14., 11., rgb(METAL));
    rect(window, origin, 2., 8., 14., 1., rgb(light(METAL, 0.25)));
    rect(window, origin, 3., 9., 12., 9., rgb(shade(screen, 0.7)));
    if scene.agent.is_some() {
        // Lines of "code" in two colours, moving with the frame.
        for line in 0..4u64 {
            let indent = ((line + scene.frame) % 3) as f32;
            let len = 2. + ((scene.frame + line * 5) % 6) as f32;
            let color = if line.is_multiple_of(2) { screen } else { 0xd7dbe6 };
            rect(window, origin, 4. + indent, 10. + line as f32 * 2., len.min(9. - indent), 1., rgb(color));
        }
    }
    rect(window, origin, 8., 19., 2., 1., rgb(METAL_DARK));
    rect(window, origin, 6., 20., 6., 1., rgb(METAL_DARK));
    // Something different on each desk.
    match scene.seat % 4 {
        0 => {
            // A desk lamp.
            rect(window, origin, 38., 20., 4., 1., rgb(METAL_DARK));
            rect(window, origin, 39., 14., 1., 6., rgb(METAL_DARK));
            rect(window, origin, 36., 12., 5., 2., rgb(0x2f9e8f));
            if scene.agent.is_some() {
                rect(window, origin, 35., 14., 7., 3., glow(0xffe08a, 0.18));
            }
        }
        1 => {
            // A stack of books.
            rect(window, origin, 34., 18., 7., 2., rgb(0xb4443f));
            rect(window, origin, 35., 16., 6., 2., rgb(0x3d6fb4));
            rect(window, origin, 34., 14., 7., 2., rgb(0xd9a441));
        }
        2 => {
            // A plant.
            rect(window, origin, 35., 17., 5., 4., rgb(0x8a5a3b));
            rect(window, origin, 34., 13., 2., 4., rgb(0x57b36a));
            rect(window, origin, 37., 11., 2., 6., rgb(0x3f8f4f));
            rect(window, origin, 39., 13., 2., 4., rgb(0x6cc47f));
        }
        _ => {
            // Coffee and a sticky note.
            rect(window, origin, 34., 17., 3., 4., rgb(0xece6d4));
            rect(window, origin, 37., 18., 1., 2., rgb(0xece6d4));
            rect(window, origin, 34., 17., 3., 1., rgb(0x6b4226));
            rect(window, origin, 38., 20., 4., 1., rgb(0xf6d860));
        }
    }
}

/// The top wall: wooden panels below, a skirting board, an elevator at the left end.
fn paint_wall(window: &mut Window, origin: Point<Pixels>, width: f32, height: f32) {
    let (w, h) = (width / PX, height / PX);
    rect(window, origin, 0., 0., w, h, rgb(WALL));
    rect(window, origin, 0., 0., w, 1., rgb(light(WALL, 0.08)));
    let panel = (h * 0.4).round();
    rect(window, origin, 0., h - panel, w, panel, rgb(0x4a3526));
    rect(window, origin, 0., h - panel, w, 1., rgb(0x6d4f37));
    let mut x = 6.;
    while x < w {
        rect(window, origin, x, h - panel + 2., 1., panel - 4., rgb(0x3d2b1f));
        x += 14.;
    }
    rect(window, origin, 0., h - 2., w, 2., rgb(0x2a1e16));
    rect(window, origin, 4., h - 30., 18., 28., rgb(0x6b7180));
    rect(window, origin, 5., h - 29., 8., 27., rgb(0x9aa1b2));
    rect(window, origin, 13., h - 29., 8., 27., rgb(0x8d94a5));
    rect(window, origin, 12.5, h - 29., 1., 27., rgb(0x5b6070));
    rect(window, origin, 23., h - 18., 2., 3., rgb(0xf6d860));
}

/// The break room: carpet, a rug, a sofa with cushions, a vending machine, a water cooler, a plant.
fn paint_lounge(window: &mut Window, origin: Point<Pixels>, width: f32, height: f32) {
    let (w, h) = (width / PX, height / PX);
    rect(window, origin, 0., 0., w, h, rgb(LOUNGE_FLOOR));
    rect(window, origin, w * 0.2, h - 22., w * 0.6, 16., rgb(0x5a3a4a));
    rect(window, origin, w * 0.2 + 2., h - 20., w * 0.6 - 4., 12., rgb(0x6e4a5c));
    let s = 4.;
    rect(window, origin, s, h - 26., 34., 3., shadow(0.3));
    rect(window, origin, s, h - 40., 34., 8., rgb(0x93466a));
    rect(window, origin, s, h - 32., 34., 7., rgb(0x7c3a52));
    rect(window, origin, s + 2., h - 38., 9., 6., rgb(0xb05d84));
    rect(window, origin, s + 23., h - 38., 9., 6., rgb(0xb05d84));
    rect(window, origin, s - 2., h - 36., 3., 11., rgb(0x6a3046));
    rect(window, origin, s + 33., h - 36., 3., 11., rgb(0x6a3046));
    let v = w - 48.;
    rect(window, origin, v, h - 44., 14., 36., rgb(0xc0392b));
    rect(window, origin, v + 2., h - 42., 7., 24., rgb(0x1d2430));
    for (row, colors) in [[0x3b82f6, 0xf6d860], [0x22c55e, 0xec4899], [0xf59e0b, 0x39c5bb]].iter().enumerate() {
        for (i, color) in colors.iter().enumerate() {
            rect(window, origin, v + 3. + i as f32 * 3., h - 40. + row as f32 * 7., 2., 4., rgb(*color));
        }
    }
    rect(window, origin, v + 10., h - 40., 3., 4., rgb(0x111111));
    rect(window, origin, v + 2., h - 15., 7., 3., rgb(0x111111));
    let c = w - 26.;
    rect(window, origin, c + 1., h - 42., 6., 7., rgb(0x7cc6f2));
    rect(window, origin, c + 2., h - 41., 1., 5., rgb(light(0x7cc6f2, 0.5)));
    rect(window, origin, c, h - 35., 8., 27., rgb(0xd8dde6));
    rect(window, origin, c + 1., h - 30., 2., 2., rgb(0x3b82f6));
    rect(window, origin, c + 5., h - 30., 2., 2., rgb(0xef4444));
    let p = w - 10.;
    rect(window, origin, p, h - 16., 6., 8., rgb(0x8a5a3b));
    rect(window, origin, p - 1., h - 26., 3., 10., rgb(0x57b36a));
    rect(window, origin, p + 3., h - 28., 3., 12., rgb(0x3f8f4f));
    rect(window, origin, p + 1., h - 22., 3., 6., rgb(0x6cc47f));
}

/// The session's own agent at the console: a long desk with screens on both sides.
fn paint_console(window: &mut Window, origin: Point<Pixels>, working: bool, frame: u64) {
    rect(window, origin, 2., 28., 56., 3., shadow(0.35));
    rect(window, origin, 25., 8., 10., 14., rgb(CHAIR));
    let palette = agent_palette(CLAUDE);
    paint_rows(window, origin, 22., 6. + if working && !frame.is_multiple_of(2) { 1. } else { 0. }, &SEATED, &palette);
    for (i, x) in [3., 41.].into_iter().enumerate() {
        let screen = if working { 0x22c55e } else { SCREEN_OFF };
        if working {
            rect(window, origin, x - 1., 3., 18., 16., glow(screen, 0.12));
        }
        rect(window, origin, x, 4., 16., 13., rgb(METAL));
        rect(window, origin, x, 4., 16., 1., rgb(light(METAL, 0.25)));
        rect(window, origin, x + 1., 5., 14., 11., rgb(shade(screen, 0.7)));
        if working {
            for line in 0..5u64 {
                let len = 3. + ((frame + line * 2 + i as u64) % 9) as f32;
                let color = if line.is_multiple_of(2) { screen } else { 0xd7dbe6 };
                rect(window, origin, x + 2., 6. + line as f32 * 2., len.min(12.), 1., rgb(color));
            }
        }
        rect(window, origin, x + 7., 17., 2., 3., rgb(METAL_DARK));
    }
    rect(window, origin, 0., 20., 60., 1., rgb(light(0x5b6070, 0.25)));
    rect(window, origin, 0., 21., 60., 2., rgb(0x5b6070));
    rect(window, origin, 0., 23., 60., 6., rgb(METAL_DARK));
    for (x, color) in [(6., 0x22c55e), (9., 0xf6d860), (12., 0xef4444)] {
        rect(window, origin, x, 25., 2., 1., rgb(if working { color } else { shade(color, 0.6) }));
    }
    rect(window, origin, 24., 19., 12., 1., rgb(0x7a8090));
}

/// A speech bubble: the agent's words in dark ink on paper, its tail pointing down.
fn bubble(text: String, width: f32) -> gpui::Div {
    div()
        .w(px(width))
        .flex()
        .flex_col()
        .items_center()
        .child(
            div()
                .max_w_full()
                .px_2()
                .py_1()
                .rounded(px(3.))
                .border_2()
                .border_color(rgb(INK))
                .bg(rgb(PAPER))
                .text_color(rgb(PAPER_INK))
                .t_caption()
                .line_clamp(3)
                // A hard, pixel-art shadow.
                .shadow(vec![gpui::BoxShadow {
                    color: shadow(0.45),
                    offset: point(px(3.), px(3.)),
                    blur_radius: px(0.),
                    spread_radius: px(0.),
                }])
                .child(text),
        )
        // The tail: a step down to the speaker.
        .child(div().ml(px(-6.)).w(px(8.)).h(px(3.)).bg(rgb(INK)))
        .child(div().ml(px(-10.)).w(px(4.)).h(px(3.)).bg(rgb(INK)))
}

/// The name on an agent's desk: its task, or its type when it was given none.
fn name_of(file: &SubagentFile) -> String {
    if file.description.trim().is_empty() {
        file.agent_type.clone()
    } else {
        file.description.clone()
    }
}

/// What the office needs of one subagent.
struct Worker<'a> {
    file: &'a SubagentFile,
    trace: Option<&'a Trace>,
    /// Finished, by its transcript or by Claude Code's own word (`SubagentStop`), whichever came first.
    done: bool,
}

impl Workbench {
    pub(super) fn render_office<'a>(
        &self,
        panel: &AgentPanel,
        agents: &[&'a SubagentFile],
        trace_of: &impl Fn(&str) -> Option<&'a Trace>,
        now: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let frame = now / 500;
        let pane = self.all_panes().into_iter().find(|p| p.entity_id() == panel.pane);
        let stopping = pane.as_ref().map(|p| p.read(cx).stopping_subagents.clone()).unwrap_or_default();
        let ended: std::collections::HashSet<String> = pane
            .as_ref()
            .map(|p| p.read(cx).subagents.iter().filter(|r| r.finished.is_some()).map(|r| r.id.clone()).collect())
            .unwrap_or_default();
        let workers: Vec<Worker<'a>> = agents
            .iter()
            .map(|file| {
                let trace = trace_of(&file.id);
                Worker { file, trace, done: super::agent_speech::finished(trace, ended.contains(&file.id), now) }
            })
            .collect();
        // At a desk while it runs (and while its log is still being read); in the break room once done.
        let (at_desks, resting): (Vec<&Worker>, Vec<&Worker>) = workers.iter().partition(|w| !w.done);
        // Same seat for the same agent while it works: in the order they started.
        let mut at_desks = at_desks;
        at_desks.sort_by_key(|w| w.trace.and_then(|t| t.started_ms).unwrap_or(u64::MAX));
        let floors = at_desks.len().div_ceil(DESKS_PER_FLOOR).max(1);

        // Measured as drawn, so the window can fit it.
        let measured = self.agent_window.office_height.clone();
        let mut building = div()
            .relative()
            .flex()
            .flex_col()
            .gap_0()
            .child(canvas(|_, _, _| {}, move |bounds, _, _, _| measured.set(f32::from(bounds.size.height))).absolute().size_full());
        building = building.child(self.render_office_wall(&workers, now, cx));
        // The top floor first, as a building is drawn.
        for floor in (0..floors).rev() {
            // A full floor below the top; the top one only as many rows as it needs.
            let here = (at_desks.len() - floor * DESKS_PER_FLOOR).min(DESKS_PER_FLOOR);
            let desks = if floor + 1 < floors { DESKS_PER_FLOOR } else { here.div_ceil(ROW).max(1) * ROW };
            let seats: Vec<Option<&Worker>> = (0..desks).map(|i| at_desks.get(floor * DESKS_PER_FLOOR + i).copied()).collect();
            // Four to a row: as wide as four desks and the gaps between them.
            let mut grid =
                div().mx_auto().w(px(ROW as f32 * DESK_W * PX + (ROW - 1) as f32 * 12.)).flex().flex_wrap().gap_x_3().gap_y_2().py_2();
            for (seat, worker) in seats.into_iter().enumerate() {
                grid = grid.child(self.render_desk(panel, pane.as_ref(), worker, floor * DESKS_PER_FLOOR + seat, &stopping, frame, cx));
            }
            building = building.child(
                div()
                    .relative()
                    .border_t_2()
                    .border_color(rgb(WALL_TRIM))
                    .child(
                        canvas(
                            |_, _, _| {},
                            |bounds, _, window, _| {
                                paint_tiles(
                                    window,
                                    bounds.origin,
                                    f32::from(bounds.size.width),
                                    f32::from(bounds.size.height),
                                    FLOOR_A,
                                    FLOOR_B,
                                )
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(6.))
                            .left(px(8.))
                            .px_1p5()
                            .rounded_sm()
                            .bg(rgb(INK))
                            .text_color(rgb(LED))
                            .t_caption()
                            .font_family("JetBrains Mono")
                            .child(format!("{}F", floor + 1)),
                    )
                    .child(grid),
            );
        }
        building = building.child(self.render_lounge(panel, pane.as_ref(), &resting, cx));
        building = building.child(self.render_console(pane.as_ref(), frame, cx));
        div().id("office").size_full().overflow_y_scroll().bg(rgb(FLOOR_A)).child(building).into_any_element()
    }

    /// The top wall: a window, a clock and the to-do board with every subagent's task.
    fn render_office_wall(&self, workers: &[Worker], now: u64, cx: &mut Context<Self>) -> AnyElement {
        let clock = {
            use chrono::TimeZone;
            chrono::Local.timestamp_millis_opt(now as i64).single().map(|at| at.format("%H:%M").to_string()).unwrap_or_default()
        };
        let mut board = div()
            .w(px(260.))
            .p_1p5()
            .rounded_sm()
            .border_2()
            .border_color(rgb(METAL))
            .bg(rgb(PAPER))
            .text_color(rgb(PAPER_INK))
            .t_caption()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(div().font_weight(crate::theme::EMPHASIS).child(t(cx, "office.todo")));
        for (listed, worker) in workers.iter().filter(|w| !w.done).chain(workers.iter().filter(|w| w.done)).enumerate() {
            if listed == 6 {
                board =
                    board.child(div().text_color(rgb(LABEL_DIM)).child(tf(cx, "office.more", &[("n", &(workers.len() - 6).to_string())])));
                break;
            }
            let done = worker.done;
            board = board.child(div().flex().gap_1().child(div().flex_shrink_0().child(if done { "✓" } else { "▶" })).child(
                div().truncate().when(done, |d| d.text_color(rgb(LABEL_DIM)).line_through()).child(worker.file.description.clone()),
            ));
        }
        if workers.is_empty() {
            board = board.child(div().text_color(rgb(LABEL_DIM)).child(t(cx, "collab.no_agents")));
        }
        let window = canvas(
            |_, _, _| {},
            |bounds, _, window, _| {
                let o = bounds.origin;
                rect(window, o, 0., 0., 40., 22., rgb(METAL));
                rect(window, o, 1., 1., 38., 20., rgb(0x6aa7e8));
                // A skyline.
                for (x, h) in [(2., 10.), (8., 14.), (13., 8.), (18., 16.), (25., 11.), (31., 13.)] {
                    rect(window, o, x, 21. - h, 5., h, rgb(0x2b3a55));
                    rect(window, o, x + 1., 22. - h, 1., 1., rgb(0xf6d860));
                    rect(window, o, x + 3., 24. - h, 1., 1., rgb(0xf6d860));
                }
                rect(window, o, 19.5, 1., 1., 20., rgb(METAL));
            },
        )
        .w(px(40. * PX))
        .h(px(22. * PX));
        let working = workers.iter().filter(|w| !w.done).count();
        let sign = |text: String, color: u32| {
            div()
                .px_2()
                .py_0p5()
                .rounded_sm()
                .border_2()
                .border_color(rgb(METAL))
                .bg(rgb(INK))
                .text_color(rgb(color))
                .font_family("JetBrains Mono")
                .shadow(vec![gpui::BoxShadow {
                    color: glow(color, 0.25),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(8.),
                    spread_radius: px(0.),
                }])
                .child(text)
        };
        div()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .gap_6()
            .pl(px(30. * PX))
            .pr_4()
            .pt_3()
            .pb(px(16.))
            .child(
                canvas(
                    |_, _, _| {},
                    |bounds, _, window, _| paint_wall(window, bounds.origin, f32::from(bounds.size.width), f32::from(bounds.size.height)),
                )
                .absolute()
                .size_full(),
            )
            .child(window)
            .child(div().flex().flex_col().items_center().gap_2().child(sign(clock, LED).t_body()).child(
                sign(tf(cx, "office.working", &[("n", &working.to_string())]), if working > 0 { 0xf6d860 } else { LABEL_DIM }).t_caption(),
            ))
            .child(board.shadow(vec![gpui::BoxShadow {
                color: shadow(0.4),
                offset: point(px(3.), px(3.)),
                blur_radius: px(0.),
                spread_radius: px(0.),
            }]))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_desk(
        &self,
        panel: &AgentPanel,
        pane: Option<&Pane>,
        worker: Option<&Worker>,
        seat: usize,
        stopping: &std::collections::HashSet<String>,
        frame: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let width = DESK_W * PX;
        let Some(worker) = worker else {
            let scene = DeskScene { agent: None, frame, seat };
            return div()
                .w(px(width))
                .flex()
                .flex_col()
                .items_center()
                .child(div().h(px(52.)))
                .child(
                    canvas(|_, _, _| {}, move |bounds, _, window, _| paint_desk(window, bounds.origin, &scene))
                        .w(px(width))
                        .h(px(DESK_H * PX)),
                )
                .child(div().h(px(18.)))
                .into_any_element();
        };
        let id = worker.file.id.clone();
        let is_stopping = stopping.contains(&id);
        let kind = worker.trace.and_then(|t| t.current()).and_then(|s| s.kind());
        let scene = DeskScene { agent: Some((agent_color(&id), kind, is_stopping)), frame, seat };
        let words = if is_stopping {
            t(cx, "collab.stopping").to_string()
        } else {
            worker.trace.map(|t| super::agent_speech::speech(t, false, cx)).unwrap_or_else(|| t(cx, "collab.thinking").to_string())
        };
        let selected = panel.selected.as_deref() == Some(id.as_str());
        let group = SharedString::from(format!("desk-{seat}"));
        let select = id.clone();
        div()
            .id(("office-desk", seat))
            .group(group.clone())
            .w(px(width))
            .flex()
            .flex_col()
            .items_center()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_subagent(&select, cx)))
            .child(div().h(px(52.)).flex().flex_col().justify_end().child(bubble(words, width + 24.)))
            .child(
                canvas(|_, _, _| {}, move |bounds, _, window, _| paint_desk(window, bounds.origin, &scene)).w(px(width)).h(px(DESK_H * PX)),
            )
            .child(
                div()
                    .h(px(18.))
                    .max_w(px(width))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .rounded_sm()
                    .when(selected, |d| d.bg(rgb(agent_color(&id))).text_color(rgb(INK)))
                    .when(!selected, |d| d.text_color(rgb(LABEL)))
                    .t_caption()
                    // What it was given to do tells agents apart better than their type.
                    .child(div().truncate().child(name_of(worker.file)))
                    .children(pane.filter(|_| !is_stopping && worker.trace.is_some()).map(|pane| {
                        let (target, stop) = (pane.clone(), id.clone());
                        div()
                            .id(("office-stop", seat))
                            .invisible()
                            .group_hover(group.clone(), |s| s.visible())
                            .flex_shrink_0()
                            .child(icon("circle-x", 12., rgb(STOP)))
                            .tooltip(crate::ui::Tooltip::text(t(cx, "collab.stop"), None))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.stop_subagent(&target, &stop, cx);
                            }))
                    })),
            )
            .into_any_element()
    }

    /// Finished subagents, standing about with their report's first line, each one dismissable.
    fn render_lounge(&self, panel: &AgentPanel, pane: Option<&Pane>, resting: &[&Worker], cx: &mut Context<Self>) -> AnyElement {
        let furniture = canvas(
            |_, _, _| {},
            |bounds, _, window, _| paint_lounge(window, bounds.origin, f32::from(bounds.size.width), f32::from(bounds.size.height)),
        )
        .absolute()
        .size_full();
        // Between the sofa on the left and the machines on the right.
        let mut people = div().flex().flex_wrap().justify_center().gap_x_4().gap_y_2().pt_2().pb(px(8. * PX)).px(px(44. * PX));
        for (index, worker) in resting.iter().enumerate() {
            let id = worker.file.id.clone();
            let color = agent_color(&id);
            let words = worker.trace.map(|t| super::agent_speech::speech(t, true, cx)).unwrap_or_else(|| t(cx, "narrate.done").to_string());
            let selected = panel.selected.as_deref() == Some(id.as_str());
            let group = SharedString::from(format!("lounge-{index}"));
            let (select, dismiss) = (id.clone(), id.clone());
            let target = pane.cloned();
            people = people.child(
                div()
                    .id(("office-rest", index))
                    .group(group.clone())
                    .w(px(150.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_subagent(&select, cx)))
                    .child(bubble(words, 150.))
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let palette = agent_palette(color);
                                paint_rows(window, bounds.origin, 0., 0., &STANDING, &palette);
                            },
                        )
                        .w(px(STAND_W * PX))
                        .h(px(STAND_H * PX)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_1()
                            .rounded_sm()
                            .t_caption()
                            .when(selected, |d| d.bg(rgb(color)).text_color(rgb(INK)))
                            .when(!selected, |d| d.text_color(rgb(LABEL)))
                            .child(div().truncate().child(name_of(worker.file)))
                            .children(target.map(|target| {
                                div()
                                    .id(("office-dismiss", index))
                                    .invisible()
                                    .group_hover(group.clone(), |s| s.visible())
                                    .child(icon("x", 12., rgb(LABEL)))
                                    .tooltip(crate::ui::Tooltip::text(t(cx, "office.dismiss"), None))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.dismiss_subagents(&target, std::slice::from_ref(&dismiss), cx);
                                    }))
                            })),
                    ),
            );
        }
        let ids: Vec<String> = resting.iter().map(|w| w.file.id.clone()).collect();
        let clear_all = pane.cloned().filter(|_| ids.len() > 1).map(|target| {
            div()
                .id("office-dismiss-all")
                .px_1p5()
                .rounded_sm()
                .cursor_pointer()
                .text_color(rgb(LABEL))
                .hover(|s| s.bg(rgb(WALL_TRIM)))
                .child(t(cx, "office.dismiss_all"))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.dismiss_subagents(&target, &ids, cx)))
        });
        div()
            .relative()
            .border_t_2()
            .border_color(rgb(WALL_TRIM))
            .min_h(px(52. * PX))
            .child(furniture)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .pt_1p5()
                    .t_caption()
                    .child(div().px_1p5().rounded_sm().bg(rgb(INK)).text_color(rgb(0xf6d860)).child(t(cx, "office.lounge")))
                    .child(div().text_color(rgb(LABEL_DIM)).child(tf(cx, "office.resting", &[("n", &resting.len().to_string())])))
                    .child(div().flex_1())
                    .children(clear_all),
            )
            .child(people)
            .into_any_element()
    }

    /// The session's own agent at the console, saying what it is doing.
    fn render_console(&self, pane: Option<&Pane>, frame: u64, cx: &mut Context<Self>) -> AnyElement {
        let (working, words, name): (bool, String, &str) = match pane {
            Some(pane) => {
                let view = pane.read(cx);
                let working = view.status.in_turn();
                let words = match (&view.last_tool, working) {
                    (Some((tool, target)), true) => crate::terminal::tool_label(tool, target.as_deref()),
                    _ => super::status_label(view, cx).0,
                };
                let name = match view.agent_kind() {
                    Some(crate::launch::PaneKind::Codex) => "Codex",
                    _ => "Claude",
                };
                (working, words, name)
            }
            None => (false, String::new(), ""),
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .py_3()
            .border_t_2()
            .border_color(rgb(WALL_TRIM))
            .bg(rgb(0x3a1f22))
            .child(bubble(words, 260.))
            .child(
                canvas(|_, _, _| {}, move |bounds, _, window, _| paint_console(window, bounds.origin, working, frame))
                    .w(px(60. * PX))
                    .h(px(31. * PX)),
            )
            .child(div().t_caption().text_color(rgb(CLAUDE)).font_weight(crate::theme::EMPHASIS).child(name))
            .into_any_element()
    }

    /// Opens someone's log beside the office, or closes it when it is theirs already.
    fn toggle_subagent(&mut self, id: &str, cx: &mut Context<Self>) {
        let open = self.subagents_shown().is_some_and(|p| p.selected.as_deref() == Some(id));
        if open {
            if let Some(panel) = self.subagents_shown() {
                panel.selected = None;
            }
            return cx.notify();
        }
        self.select_subagent(id, cx);
    }

    /// Takes finished subagents out of the view (their transcripts stay where they are).
    pub(super) fn dismiss_subagents(&mut self, pane: &Pane, ids: &[String], cx: &mut Context<Self>) {
        pane.update(cx, |view, _| view.dismissed_subagents.extend(ids.iter().cloned()));
        if let Some(panel) = self.subagents_shown() {
            if panel.selected.as_ref().is_some_and(|s| ids.contains(s)) {
                panel.selected = None;
            }
        }
        cx.notify();
    }

    /// Keeps the office moving while it is on screen.
    pub(super) fn animate_office(&mut self, cx: &mut Context<Self>) {
        if self.agent_window.ticking {
            return;
        }
        self.agent_window.ticking = true;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
            let alive = this.update(cx, |this, cx| {
                let shown = this.agent_window.office && this.subagents_shown().is_some();
                if shown {
                    cx.notify();
                } else {
                    this.agent_window.ticking = false;
                }
                shown
            });
            if !alive.unwrap_or(false) {
                break;
            }
        })
        .detach();
    }
}
