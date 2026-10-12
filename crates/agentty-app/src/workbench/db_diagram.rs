//! The database page's relationship diagram: every table as a card with its comment and its
//! columns (key, type, comment), joined by lines from a referencing column to the column it
//! points at. Foreign keys the database declares are solid; references read off the names
//! (`user_settings.user_id` → `users.id`, see [`agentty_db::schema::relations`]) are dashed and in
//! another colour, so a guess never passes for a constraint.
//!
//! Geometry is fixed, not measured: a card's rows sit at known heights under its heading, so where
//! a line starts and ends is known before anything is laid out, and the lines are painted under
//! the cards in one pass. Tables are placed by [`layout`] — referenced tables left of the tables
//! that point at them — and can be dragged anywhere after.

use super::plugin_ui::{MONO, RAISED};
use super::Workbench;
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, Tooltip, TypeScale};
use agentty_db::engine::Session;
use agentty_db::schema::{relations, Relation, Schema, TableShape};
use gpui::{
    canvas, div, point, prelude::*, px, AnyElement, AppContext, Bounds, ClickEvent, Context, PathBuilder, Pixels, Point, ScrollHandle,
    SharedString, Window,
};
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

const CARD_WIDTH: f32 = 330.;
/// Room on every side of the tables (the diagram's own pixels): the board is the tables with this
/// much around them, and the view opens on its middle.
const MARGIN: f32 = 1600.;
/// The coloured strip and the table's name.
const HEADING: f32 = 34.;
/// The table's comment, under its name, when it has one.
const NOTE: f32 = 20.;
const ROW: f32 = 22.;
/// Space above the first column row and below the last.
const ROW_PAD: f32 = 4.;
/// Columns shown per card; the rest fold into one "+N more" row.
const MAX_ROWS: usize = 40;
const GAP_X: f32 = 130.;
const GAP_Y: f32 = 40.;
const GRID_STEP: f32 = 24.;
/// Diagrams with more tables than this build only the cards near the view.
const CULL_FROM: usize = 80;
const CANVAS: u32 = 0x161618;
const ZOOM_RANGE: (f32, f32) = (0.25, 2.);
const ZOOM_STEP: f32 = 1.2;
const FIT_MARGIN: f32 = 40.;
/// Strips at the top of the cards, so neighbouring tables tell apart at a glance.
const STRIPS: [u32; 6] = [Chrome::BLUE, Chrome::GREEN, Chrome::PURPLE, Chrome::ORANGE, Chrome::CLAUDE, Chrome::ATTENTION];
const DECLARED: u32 = Chrome::BLUE;
const INFERRED: u32 = Chrome::ORANGE;

/// Which view of a connection the page shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum DbView {
    #[default]
    Data,
    Diagram,
}

/// A connection's diagram: its shape as read, where each table sits, and how the user looks at it.
pub(super) struct Diagram {
    pub(super) connection: String,
    pub(super) schema: Schema,
    pub(super) relations: Vec<Relation>,
    /// Table index by name.
    index: HashMap<String, usize>,
    /// Top-left corner of each table's card, in the diagram's own pixels.
    pub(super) positions: Vec<(f32, f32)>,
    pub(super) zoom: Rc<Cell<f32>>,
    pub(super) show_inferred: bool,
    /// The table whose lines stand out (clicked, or picked in the tables list).
    pub(super) focus: Option<usize>,
    viewport: Rc<Cell<(f32, f32)>>,
    scroll: ScrollHandle,
    /// Dragging the canvas: where the pointer went down and the scroll position then.
    pan: Option<(Point<Pixels>, Point<Pixels>)>,
    /// A card's heading pressed: the pointer then and where the card was. A drag only starts once
    /// the pointer has moved a little; the card follows from the press, not from there.
    press: Option<(Point<Pixels>, (f32, f32))>,
    /// The view was put on the middle of the tables (once the place it is shown in is known).
    centred: Rc<Cell<bool>>,
}

/// What is worked out off the UI thread: the shape, its relations and where each table goes.
pub(super) struct Shape {
    schema: Schema,
    relations: Vec<Relation>,
    index: HashMap<String, usize>,
    positions: Vec<(f32, f32)>,
}

impl Shape {
    pub(super) fn new(schema: Schema) -> Shape {
        let relations = relations(&schema);
        let index: HashMap<String, usize> = schema.tables.iter().enumerate().map(|(i, t)| (t.name.clone(), i)).collect();
        let heights: Vec<f32> = schema.tables.iter().map(card_height).collect();
        let links: Vec<(usize, usize)> =
            relations.iter().filter_map(|r| Some((*index.get(&r.from_table)?, *index.get(&r.to_table)?))).collect();
        let positions = layout(&heights, &links);
        Shape { schema, relations, index, positions }
    }
}

impl Diagram {
    pub(super) fn new(connection: String, shape: Shape) -> Diagram {
        let Shape { schema, relations, index, positions } = shape;
        Diagram {
            connection,
            schema,
            relations,
            index,
            positions,
            zoom: Rc::new(Cell::new(1.)),
            show_inferred: true,
            focus: None,
            viewport: Rc::new(Cell::new((0., 0.))),
            scroll: ScrollHandle::new(),
            pan: None,
            press: None,
            centred: Rc::new(Cell::new(false)),
        }
    }

    /// The box the cards take, in the diagram's own pixels: left, top, right, bottom.
    fn bounds(&self) -> (f32, f32, f32, f32) {
        let mut out = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (table, p) in self.schema.tables.iter().zip(&self.positions) {
            out = (out.0.min(p.0), out.1.min(p.1), out.2.max(p.0 + CARD_WIDTH), out.3.max(p.1 + card_height(table)));
        }
        if out.0 > out.2 {
            (0., 0., 0., 0.)
        } else {
            out
        }
    }

    fn fit_zoom(&self) -> f32 {
        let (left, top, right, bottom) = self.bounds();
        let (w, h) = (right - left, bottom - top);
        let (vw, vh) = self.viewport.get();
        if w <= 0. || h <= 0. || vw <= 0. || vh <= 0. {
            return 1.;
        }
        clamp_zoom(((vw - FIT_MARGIN) / w).min((vh - FIT_MARGIN) / h).min(1.))
    }

    /// Scrolls so the diagram's point `at` sits in the middle of the view.
    fn look_at(&self, at: (f32, f32)) {
        let z = self.zoom.get();
        let (vw, vh) = self.viewport.get();
        let x = (at.0 + MARGIN) * z - vw / 2.;
        let y = (at.1 + MARGIN) * z - vh / 2.;
        self.scroll.set_offset(point(px(-x.max(0.)), px(-y.max(0.))));
    }

    /// The middle of the tables, in the middle of the view.
    fn centre(&self) {
        let (left, top, right, bottom) = self.bounds();
        self.look_at(((left + right) / 2., (top + bottom) / 2.));
    }

    /// Zooms to `zoom`, keeping what is in the middle of the view there.
    fn zoom_to(&self, zoom: f32) {
        let middle = view_middle(&self.scroll, self.viewport.get(), self.zoom.get());
        self.zoom.set(clamp_zoom(zoom));
        self.look_at(middle);
    }

    /// Scrolls so `table` sits in the middle of the view, and makes its lines stand out.
    pub(super) fn reveal(&mut self, table: &str) {
        let Some(&i) = self.index.get(table) else { return };
        self.focus = Some(i);
        let (x, y) = self.positions[i];
        let (_, vh) = self.viewport.get();
        let z = self.zoom.get();
        // Its top a little above the middle, so a tall table is read from its name down.
        self.look_at((x + CARD_WIDTH / 2., y + (vh / 4.) / z));
    }

    pub(super) fn debug_state(&self) -> serde_json::Value {
        serde_json::json!({
            "connection": self.connection,
            "tables": self.schema.tables.len(),
            "truncated": self.schema.truncated,
            "relations": self.relations.iter().map(|r| format!(
                "{}.{} -> {}.{}{}", r.from_table, r.from_column, r.to_table, r.to_column, if r.declared { "" } else { " (inferred)" }
            )).collect::<Vec<_>>(),
            "zoom": self.zoom.get(),
            "show_inferred": self.show_inferred,
            "focus": self.focus.map(|i| self.schema.tables[i].name.clone()),
        })
    }
}

/// The diagram's point in the middle of the view.
fn view_middle(scroll: &ScrollHandle, (vw, vh): (f32, f32), z: f32) -> (f32, f32) {
    let offset = scroll.offset();
    ((-f32::from(offset.x) + vw / 2.) / z - MARGIN, (-f32::from(offset.y) + vh / 2.) / z - MARGIN)
}

fn clamp_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() {
        zoom.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1)
    } else {
        1.
    }
}

fn shown_rows(table: &TableShape) -> usize {
    if table.columns.len() > MAX_ROWS {
        MAX_ROWS + 1
    } else {
        table.columns.len().max(1)
    }
}

fn card_height(table: &TableShape) -> f32 {
    HEADING + if table.comment.is_empty() { 0. } else { NOTE } + ROW_PAD * 2. + shown_rows(table) as f32 * ROW
}

/// The middle of column `column`'s row, from the card's top. A column folded into "+N more" is
/// drawn to that row.
fn row_middle(table: &TableShape, column: usize) -> f32 {
    let note = if table.comment.is_empty() { 0. } else { NOTE };
    HEADING + note + ROW_PAD + column.min(MAX_ROWS) as f32 * ROW + ROW / 2.
}

/// Where each table goes. Tables joined by references form groups, the largest first, one under
/// the other; inside a group a table stands one column right of every table it points at, so
/// lines run left to right from the referencing table to the referenced one's key (cycles are cut
/// where they close). Tables nothing joins fill a grid underneath.
pub(super) fn layout(heights: &[f32], links: &[(usize, usize)]) -> Vec<(f32, f32)> {
    let n = heights.len();
    let mut positions = vec![(0., 0.); n];
    if n == 0 {
        return positions;
    }
    // What each table points at, and the groups the links make.
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut neighbours: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(from, to) in links {
        if from != to && from < n && to < n {
            if !parents[from].contains(&to) {
                parents[from].push(to);
            }
            neighbours[from].push(to);
            neighbours[to].push(from);
        }
    }
    let mut group = vec![usize::MAX; n];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for start in 0..n {
        if group[start] != usize::MAX {
            continue;
        }
        let id = groups.len();
        let mut members = vec![start];
        group[start] = id;
        let mut next = 0;
        while next < members.len() {
            let at = members[next];
            next += 1;
            for &other in &neighbours[at] {
                if group[other] == usize::MAX {
                    group[other] = id;
                    members.push(other);
                }
            }
        }
        groups.push(members);
    }

    // Column of each table: one right of the farthest table it points at. A link back into a
    // table still being worked out closes a cycle and is left out.
    fn rank(at: usize, parents: &[Vec<usize>], ranks: &mut [Option<usize>], visiting: &mut [bool]) -> usize {
        if let Some(r) = ranks[at] {
            return r;
        }
        visiting[at] = true;
        let mut r = 0;
        for &p in &parents[at] {
            if !visiting[p] {
                r = r.max(rank(p, parents, ranks, visiting) + 1);
            }
        }
        visiting[at] = false;
        ranks[at] = Some(r);
        r
    }
    let mut ranks = vec![None; n];
    let mut visiting = vec![false; n];
    for i in 0..n {
        rank(i, &parents, &mut ranks, &mut visiting);
    }

    let (joined, alone): (Vec<Vec<usize>>, Vec<Vec<usize>>) = groups.into_iter().partition(|g| g.len() > 1);
    let mut joined = joined;
    joined.sort_by_key(|g| std::cmp::Reverse(g.len()));
    let mut top = 0.;
    let mut widest = 0usize;
    for members in &joined {
        let deepest = members.iter().map(|&i| ranks[i].unwrap_or(0)).max().unwrap_or(0);
        widest = widest.max(deepest + 1);
        // Order inside a column: next to what it is joined to in the columns already placed.
        let mut order: HashMap<usize, f32> = HashMap::new();
        let mut bottom: f32 = top;
        for column in 0..=deepest {
            let mut tables: Vec<usize> = members.iter().copied().filter(|&i| ranks[i] == Some(column)).collect();
            let key = |i: usize, order: &HashMap<usize, f32>| {
                let placed: Vec<f32> = neighbours[i].iter().filter_map(|o| order.get(o).copied()).collect();
                if placed.is_empty() {
                    f32::MAX - neighbours[i].len() as f32
                } else {
                    placed.iter().sum::<f32>() / placed.len() as f32
                }
            };
            tables.sort_by(|&a, &b| key(a, &order).total_cmp(&key(b, &order)).then(a.cmp(&b)));
            let mut y = top;
            for (slot, &i) in tables.iter().enumerate() {
                positions[i] = (column as f32 * (CARD_WIDTH + GAP_X), y);
                order.insert(i, slot as f32);
                y += heights[i] + GAP_Y;
            }
            bottom = bottom.max(y);
        }
        top = bottom + GAP_Y;
    }

    // The rest, in rows as wide as the widest group (four cards at least).
    let per_row = widest.max(4);
    let alone: Vec<usize> = alone.into_iter().flatten().collect();
    for row in alone.chunks(per_row) {
        let tallest = row.iter().map(|&i| heights[i]).fold(0., f32::max);
        for (slot, &i) in row.iter().enumerate() {
            positions[i] = (slot as f32 * (CARD_WIDTH + GAP_X / 2.), top);
        }
        top += tallest + GAP_Y;
    }
    positions
}

/// Dragging a card by its heading. The card follows how far the pointer went since the press
/// ([`Diagram::press`]), whatever the canvas was scrolled to.
#[derive(Clone)]
pub struct DbCardDrag {
    table: usize,
}

impl gpui::Render for DbCardDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A line as painted: its two ends (the canvas's pixels), the way each end leaves its card, and
/// how it is drawn.
struct Line {
    from: (f32, f32),
    to: (f32, f32),
    /// +1 leaves (arrives) rightwards, -1 leftwards.
    out: f32,
    into: f32,
    declared: bool,
    lit: bool,
    dim: bool,
}

/// The two ends of a line between cards at `a` and `b` (top-left corners): from the facing sides,
/// or both from the right when the cards stand one above the other.
fn ends(a: (f32, f32), ay: f32, b: (f32, f32), by: f32) -> ((f32, f32), (f32, f32), f32, f32) {
    let (ac, bc) = (a.0 + CARD_WIDTH / 2., b.0 + CARD_WIDTH / 2.);
    if (bc - ac).abs() < CARD_WIDTH * 0.75 {
        ((a.0 + CARD_WIDTH, a.1 + ay), (b.0 + CARD_WIDTH, b.1 + by), 1., -1.)
    } else if bc > ac {
        ((a.0 + CARD_WIDTH, a.1 + ay), (b.0, b.1 + by), 1., 1.)
    } else {
        ((a.0, a.1 + ay), (b.0 + CARD_WIDTH, b.1 + by), -1., -1.)
    }
}

fn line_path(line: &Line, origin: Point<Pixels>, z: f32) -> Option<gpui::Path<Pixels>> {
    let a = point(origin.x + px(line.from.0), origin.y + px(line.from.1));
    let b = point(origin.x + px(line.to.0), origin.y + px(line.to.1));
    let reach = ((line.to.0 - line.from.0).abs() / 2.).max(50. * z);
    let width = if line.lit { 2.2 } else { 1.4 } * z;
    let mut path = PathBuilder::stroke(px(width.max(1.)));
    if !line.declared {
        path = path.dash_array(&[px(6. * z), px(4. * z)]);
    }
    path.move_to(a);
    // The arriving end comes in from outside its card: from the right when `into` is -1.
    path.cubic_bezier_to(b, point(a.x + px(reach * line.out), a.y), point(b.x - px(reach * line.into), b.y));
    path.build().ok()
}

impl Workbench {
    /// Shows the rows of a table or the diagram of the connection, reading the diagram the first
    /// time it is shown for that connection.
    pub(super) fn set_db_view(&mut self, view: DbView, cx: &mut Context<Self>) {
        self.db.view = view;
        let current = self.db.diagram.as_ref().map(|d| d.connection.as_str());
        if view == DbView::Diagram && (current.is_none() || current != self.db.selected.as_deref()) && !self.db.diagram_loading {
            self.load_db_diagram(cx);
        }
        cx.notify();
    }

    /// Reads the connection's shape again (after a migration, say).
    pub(super) fn load_db_diagram(&mut self, cx: &mut Context<Self>) {
        let Some(connection) = self.selected_connection().cloned() else { return };
        let id = connection.id.clone();
        self.db.diagram = None;
        self.db.diagram_loading = true;
        self.db.message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let config = connection.config.clone();
            let shape = cx
                .background_spawn(async move {
                    let schema = Session::connect(&config).and_then(|mut s| s.schema())?;
                    anyhow::Ok(Shape::new(schema))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.db.selected.as_deref() != Some(connection.id.as_str()) {
                    return;
                }
                this.db.diagram_loading = false;
                match shape {
                    Ok(shape) => this.db.diagram = Some(Diagram::new(id.clone(), shape)),
                    Err(err) => this.db.message = Some((format!("{err:#}"), true)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Data / Diagram, above the page's main area.
    pub(super) fn render_db_view_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let tab = |id: &'static str, view: DbView, glyph: &'static str, label: &'static str| {
            let active = self.db.view == view;
            div()
                .id(id)
                .px_2p5()
                .py_1()
                .flex()
                .items_center()
                .gap_1p5()
                .rounded_md()
                .cursor_pointer()
                .t_small()
                .text_color(hex(if active { Chrome::BRIGHT } else { Chrome::MUTED }))
                .when(active, |d| d.bg(hex(Chrome::SELECTED)))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(icon(glyph, 13., hex(if active { Chrome::BRIGHT } else { Chrome::MUTED })))
                .child(t(cx, label))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_db_view(view, cx)))
        };
        div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1p5()
            .flex_shrink_0()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .child(tab("db-view-data", DbView::Data, "rows-2", "db.view_data"))
            .child(tab("db-view-diagram", DbView::Diagram, "network", "db.view_diagram"))
            .into_any_element()
    }

    pub(super) fn render_db_diagram(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(diagram) = self.db.diagram.as_ref() else {
            let note = if self.db.diagram_loading {
                crate::ui::loading_row(t(cx, "db.diagram_loading")).into_any_element()
            } else {
                div().p_4().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "db.pick_connection")).into_any_element()
            };
            return div().flex_1().min_h_0().child(note).into_any_element();
        };
        if diagram.schema.tables.is_empty() {
            return div().flex_1().p_4().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "db.diagram_empty")).into_any_element();
        }
        // Opened: every table in view (never above 100%), in the middle — once the place it is
        // shown in has been measured.
        if diagram.viewport.get().0 > 0. && !diagram.centred.get() {
            diagram.centred.set(true);
            diagram.zoom.set(diagram.fit_zoom());
            diagram.centre();
        }
        let z = diagram.zoom.get();
        let tables = &diagram.schema.tables;
        let (_, _, right, bottom) = diagram.bounds();
        let (vw, vh) = diagram.viewport.get();
        // The tables in the middle of the board, with room all round (and past a card dragged out).
        let content_w = ((right.max(0.) + 2. * MARGIN) * z).max(vw);
        let content_h = ((bottom.max(0.) + 2. * MARGIN) * z).max(vh);
        let board = |p: (f32, f32)| ((p.0 + MARGIN) * z, (p.1 + MARGIN) * z);

        // The lines, from the geometry alone.
        let column_of = |table: &TableShape, name: &str| table.columns.iter().position(|c| c.name == name).unwrap_or(0);
        let mut lines = Vec::new();
        for relation in diagram.relations.iter().filter(|r| r.declared || diagram.show_inferred) {
            let (Some(&a), Some(&b)) = (diagram.index.get(&relation.from_table), diagram.index.get(&relation.to_table)) else { continue };
            let ay = row_middle(&tables[a], column_of(&tables[a], &relation.from_column));
            let by = row_middle(&tables[b], column_of(&tables[b], &relation.to_column));
            let (from, to, out, into) = ends(diagram.positions[a], ay, diagram.positions[b], by);
            let lit = diagram.focus.is_some_and(|f| f == a || f == b);
            lines.push(Line {
                from: board(from),
                to: board(to),
                out,
                into,
                declared: relation.declared,
                lit,
                dim: diagram.focus.is_some() && !lit,
            });
        }
        // Lit lines last, so they are drawn over the others.
        lines.sort_by_key(|l| l.lit);
        let step = GRID_STEP * z;
        let backdrop = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let o = bounds.origin;
                let mask = window.content_mask().bounds;
                let dot = hex_alpha(0xffffff, 0.06);
                let first = |o: Pixels, m: Pixels| ((f32::from(m - o) / step).floor().max(1.)) * step;
                let (x0, y0) = (first(o.x, mask.origin.x), first(o.y, mask.origin.y));
                let (x1, y1) = (f32::from(mask.origin.x + mask.size.width - o.x), f32::from(mask.origin.y + mask.size.height - o.y));
                let mut y = y0;
                while y < y1 {
                    let mut x = x0;
                    while x < x1 {
                        window.paint_quad(gpui::fill(Bounds::new(point(o.x + px(x), o.y + px(y)), gpui::size(px(1.5), px(1.5))), dot));
                        x += step;
                    }
                    y += step;
                }
                for line in &lines {
                    let color = if line.declared { DECLARED } else { INFERRED };
                    let alpha = if line.dim {
                        0.18
                    } else if line.lit {
                        1.
                    } else {
                        0.75
                    };
                    if let Some(path) = line_path(line, o, z) {
                        window.paint_path(path, hex_alpha(color, alpha));
                    }
                    // The referenced end (the key) gets a dot; the referencing end a short bar.
                    let r = (3.5 * z).max(2.);
                    let to = point(o.x + px(line.to.0), o.y + px(line.to.1));
                    window.paint_quad(
                        gpui::fill(
                            Bounds::new(point(to.x - px(r), to.y - px(r)), gpui::size(px(r * 2.), px(r * 2.))),
                            hex_alpha(color, alpha),
                        )
                        .corner_radii(px(r)),
                    );
                    let from = point(o.x + px(line.from.0), o.y + px(line.from.1));
                    let bar = (2. * z).max(1.5);
                    window.paint_quad(gpui::fill(
                        Bounds::new(point(from.x - px(bar / 2.), from.y - px(r * 1.4)), gpui::size(px(bar), px(r * 2.8))),
                        hex_alpha(color, alpha),
                    ));
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        // Only the cards in view are built: a database can have hundreds of tables.
        let offset = diagram.scroll.offset();
        let view = Bounds::new(point(-f32::from(offset.x), -f32::from(offset.y)), gpui::size(vw.max(1.), vh.max(1.)));
        let mut area = div().relative().w(px(content_w)).h(px(content_h)).child(backdrop);
        for (i, table) in tables.iter().enumerate() {
            let (x, y) = board(diagram.positions[i]);
            let card = Bounds::new(point(x, y), gpui::size(CARD_WIDTH * z, card_height(table) * z));
            let margin = vw.max(vh).max(400.);
            let near = card.origin.x < view.origin.x + view.size.width + margin
                && card.origin.x + card.size.width > view.origin.x - margin
                && card.origin.y < view.origin.y + view.size.height + margin
                && card.origin.y + card.size.height > view.origin.y - margin;
            if near || vw <= 0. || tables.len() <= CULL_FROM {
                area = area.child(self.render_db_card(diagram, i, z, cx));
            }
        }
        area = area.on_drag_move(cx.listener(move |this, event: &gpui::DragMoveEvent<DbCardDrag>, _, cx| {
            let drag = event.drag(cx).clone();
            let Some(diagram) = this.db.diagram.as_mut() else { return };
            let z = diagram.zoom.get();
            let Some((pointer, from)) = diagram.press else { return };
            let moved = event.event.position - pointer;
            // Never off the board's top or left edge.
            let to = ((from.0 + f32::from(moved.x) / z).max(-MARGIN), (from.1 + f32::from(moved.y) / z).max(-MARGIN));
            if let Some(position) = diagram.positions.get_mut(drag.table) {
                *position = to;
                cx.notify();
            }
        }));

        // The place the diagram is shown in: its size (for fit and culling), and ⌘ or Ctrl with
        // the wheel to zoom.
        let (viewport, zoom, scroll) = (diagram.viewport.clone(), diagram.zoom.clone(), diagram.scroll.clone());
        let measured = viewport.clone();
        let measure = canvas(
            move |bounds, window, _| {
                let size = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                if measured.get() != size {
                    measured.set(size);
                    // Culling and centring need the size: draw again with it.
                    window.refresh();
                }
                bounds
            },
            move |bounds, _, window, _| {
                let (zoom, scroll, viewport) = (zoom.clone(), scroll.clone(), viewport.clone());
                window.on_mouse_event(move |event: &gpui::ScrollWheelEvent, phase, window, cx| {
                    let zooming = event.modifiers.platform || event.modifiers.control;
                    if phase != gpui::DispatchPhase::Capture || !zooming || !bounds.contains(&event.position) {
                        return;
                    }
                    let dy = f32::from(event.delta.pixel_delta(px(16.)).y);
                    if dy != 0. {
                        let middle = view_middle(&scroll, viewport.get(), zoom.get());
                        zoom.set(clamp_zoom(zoom.get() * (1. + dy * 0.004)));
                        let z = zoom.get();
                        let (vw, vh) = viewport.get();
                        let at = ((middle.0 + MARGIN) * z - vw / 2., (middle.1 + MARGIN) * z - vh / 2.);
                        scroll.set_offset(point(px(-at.0.max(0.)), px(-at.1.max(0.))));
                        window.refresh();
                    }
                    cx.stop_propagation();
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .bg(hex(CANVAS))
            .overflow_hidden()
            .child(measure)
            .child(
                div()
                    .id("db-diagram")
                    .size_full()
                    .overflow_scroll()
                    .track_scroll(&diagram.scroll)
                    // Scrolling brings other cards into view: build them.
                    .on_scroll_wheel(cx.listener(|_, _: &gpui::ScrollWheelEvent, _, cx| cx.notify()))
                    // Dragging the empty canvas moves the view (a card's heading moves the card).
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            if let Some(diagram) = this.db.diagram.as_mut() {
                                diagram.pan = Some((event.position, diagram.scroll.offset()));
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                        let Some(diagram) = this.db.diagram.as_mut() else { return };
                        let Some((from, offset)) = diagram.pan else { return };
                        if event.pressed_button != Some(gpui::MouseButton::Left) {
                            diagram.pan = None;
                            return;
                        }
                        let max = diagram.scroll.max_offset();
                        let to = offset + (event.position - from);
                        diagram.scroll.set_offset(point(to.x.clamp(-max.width, px(0.)), to.y.clamp(-max.height, px(0.))));
                        cx.notify();
                    }))
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                            if let Some(diagram) = this.db.diagram.as_mut() {
                                diagram.pan = None;
                            }
                            cx.notify();
                        }),
                    )
                    .when(diagram.pan.is_some(), |d| d.cursor(gpui::CursorStyle::ClosedHand))
                    .child(area),
            )
            .child(self.render_db_diagram_bar(diagram, cx))
            .child(self.render_db_diagram_zoom(diagram, cx))
            .into_any_element()
    }

    fn render_db_card(&self, diagram: &Diagram, i: usize, z: f32, cx: &mut Context<Self>) -> AnyElement {
        let table = &diagram.schema.tables[i];
        let (x, y) = diagram.positions[i];
        let focused = diagram.focus == Some(i);
        // Which columns point elsewhere, and how sure that is.
        let mut refs: HashMap<&str, bool> = HashMap::new();
        for r in diagram.relations.iter().filter(|r| r.from_table == table.name && (r.declared || diagram.show_inferred)) {
            let declared = refs.entry(r.from_column.as_str()).or_insert(false);
            *declared |= r.declared;
        }
        let strip = STRIPS[i % STRIPS.len()];
        let (title, caption, small) = (px(12.5 * z), px(11. * z), px(10.5 * z));

        let name = table.name.clone();
        let drag = DbCardDrag { table: i };
        let heading = div()
            .id(SharedString::from(format!("db-erd-head-{i}")))
            .h(px(HEADING * z))
            .px(px(10. * z))
            .flex()
            .items_center()
            .gap(px(7. * z))
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(0x202024))
            .cursor_grab()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    if let Some(diagram) = this.db.diagram.as_mut() {
                        diagram.press = Some((event.position, diagram.positions[i]));
                    }
                    cx.stop_propagation();
                }),
            )
            .child(div().size(px(8. * z)).flex_shrink_0().rounded_full().bg(hex(strip)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(title)
                    .font_weight(crate::theme::EMPHASIS)
                    .text_color(hex(Chrome::BRIGHT))
                    .child(table.name.clone()),
            )
            .when(table.kind == "view", |d| {
                d.child(div().flex_shrink_0().text_size(small).text_color(hex(Chrome::MUTED)).child(t(cx, "db.diagram_view")))
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if event.click_count() >= 2 {
                    // Its rows, on the data side of the page.
                    this.db.view = DbView::Data;
                    this.open_db_table(name.clone(), cx);
                    return;
                }
                if let Some(diagram) = this.db.diagram.as_mut() {
                    diagram.focus = if diagram.focus == Some(i) { None } else { Some(i) };
                }
                cx.notify();
            }))
            .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()));

        let note = (!table.comment.is_empty()).then(|| {
            div()
                .h(px(NOTE * z))
                .px(px(10. * z))
                .flex()
                .items_center()
                .border_b_1()
                .border_color(hex(Chrome::BORDER))
                .child(div().min_w_0().truncate().text_size(caption).text_color(hex(Chrome::MUTED)).child(table.comment.clone()))
        });

        let mut rows = div().py(px(ROW_PAD * z)).flex().flex_col();
        for (c, column) in table.columns.iter().take(MAX_ROWS).enumerate() {
            let mark = if column.primary {
                Some(icon("key-round", 11. * z, hex(Chrome::FAVORITE)))
            } else {
                refs.get(column.name.as_str()).map(|&declared| icon("link", 11. * z, hex(if declared { DECLARED } else { INFERRED })))
            };
            let mut tip = format!("{} · {}", column.name, column.data_type);
            if !column.nullable {
                tip.push_str(" · NOT NULL");
            }
            if !column.comment.is_empty() {
                tip.push_str(&format!("\n{}", column.comment));
            }
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("db-erd-col-{i}-{c}")))
                    .h(px(ROW * z))
                    .px(px(10. * z))
                    .flex()
                    .items_center()
                    .gap(px(6. * z))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .tooltip(Tooltip::text(SharedString::from(tip), None))
                    .child(div().w(px(12. * z)).flex_shrink_0().flex().justify_center().children(mark))
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(px(CARD_WIDTH * 0.38 * z))
                            .truncate()
                            .text_size(caption)
                            .text_color(hex(if column.primary { Chrome::BRIGHT } else { Chrome::FOREGROUND }))
                            .child(column.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(small)
                            .text_color(hex_alpha(Chrome::MUTED, 0.9))
                            .child(column.comment.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(px(CARD_WIDTH * 0.36 * z))
                            .truncate()
                            .text_size(small)
                            .font_family(MONO)
                            .text_color(hex(Chrome::MUTED))
                            .child(column.data_type.clone()),
                    ),
            );
        }
        if table.columns.len() > MAX_ROWS {
            let more = (table.columns.len() - MAX_ROWS).to_string();
            rows = rows.child(
                div().h(px(ROW * z)).px(px(10. * z)).flex().items_center().text_size(small).text_color(hex(Chrome::MUTED)).child(tf(
                    cx,
                    "db.diagram_more",
                    &[("n", &more)],
                )),
            );
        }
        if table.columns.is_empty() {
            rows = rows.child(div().h(px(ROW * z)).px(px(10. * z)).text_size(small).text_color(hex(Chrome::MUTED)).child("—"));
        }

        div()
            .absolute()
            .left(px((x + MARGIN) * z))
            .top(px((y + MARGIN) * z))
            .w(px(CARD_WIDTH * z))
            .flex()
            .flex_col()
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(if focused { hex(Chrome::ACCENT) } else { hex(Chrome::BORDER) })
            .bg(hex(RAISED))
            .shadow_lg()
            .child(div().h(px(3. * z)).bg(hex(strip)))
            .child(heading)
            .children(note)
            .child(rows)
            .into_any_element()
    }

    /// What the diagram shows, the legend (the inferred kind can be hidden), and reading it again.
    fn render_db_diagram_bar(&self, diagram: &Diagram, cx: &mut Context<Self>) -> AnyElement {
        let declared = diagram.relations.iter().filter(|r| r.declared).count();
        let inferred = diagram.relations.len() - declared;
        let swatch = |color: u32, dashed: bool| {
            let piece = |w: f32| div().w(px(w)).h(px(2.)).rounded_sm().bg(hex(color));
            if dashed {
                div().flex().gap(px(3.)).child(piece(5.)).child(piece(5.)).child(piece(5.))
            } else {
                div().flex().child(piece(21.))
            }
        };
        let summary = tf(
            cx,
            "db.diagram_summary",
            &[("tables", &diagram.schema.tables.len().to_string()), ("relations", &diagram.relations.len().to_string())],
        );
        div()
            .absolute()
            .top(px(10.))
            .left(px(10.))
            .px_2p5()
            .py_1p5()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_3()
            .rounded_md()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex_alpha(RAISED, 0.94))
            .shadow_lg()
            .occlude()
            .t_caption()
            .child(div().text_color(hex(Chrome::FOREGROUND)).child(summary))
            .when(diagram.schema.truncated, |d| {
                d.child(div().text_color(hex(Chrome::WARNING)).child(tf(
                    cx,
                    "db.diagram_truncated",
                    &[("n", &agentty_db::schema::MAX_TABLES.to_string())],
                )))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_color(hex(Chrome::MUTED))
                    .child(swatch(DECLARED, false))
                    .child(format!("{} {declared}", t(cx, "db.diagram_declared"))),
            )
            .child(
                div()
                    .id("db-diagram-inferred")
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(hex(Chrome::MUTED))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .when(!diagram.show_inferred, |d| d.opacity(0.45))
                    .tooltip(Tooltip::text(SharedString::from(t(cx, "db.diagram_inferred_hint")), None))
                    .child(icon(if diagram.show_inferred { "check" } else { "square" }, 12., hex(Chrome::MUTED)))
                    .child(swatch(INFERRED, true))
                    .child(format!("{} {inferred}", t(cx, "db.diagram_inferred")))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        if let Some(diagram) = this.db.diagram.as_mut() {
                            diagram.show_inferred = !diagram.show_inferred;
                        }
                        cx.notify();
                    })),
            )
            .child(
                crate::ui::icon_only(
                    "db-diagram-reload",
                    "refresh-cw",
                    cx.listener(|this, _: &ClickEvent, _, cx| this.load_db_diagram(cx)),
                )
                .tooltip(Tooltip::text(SharedString::from(t(cx, "db.diagram_reload")), None)),
            )
            .into_any_element()
    }

    /// − 100% + and fit, at the bottom right corner.
    fn render_db_diagram_zoom(&self, diagram: &Diagram, cx: &mut Context<Self>) -> AnyElement {
        // Zooms around the middle of the view; fitting also puts the tables in the middle.
        let set = |zoom: fn(&Diagram) -> f32, fit: bool| {
            move |this: &mut Self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                if let Some(diagram) = this.db.diagram.as_ref() {
                    diagram.zoom_to(zoom(diagram));
                    if fit {
                        diagram.centre();
                    }
                }
                cx.notify();
            }
        };
        div()
            .absolute()
            .bottom(px(10.))
            .right(px(10.))
            .px_1()
            .py_0p5()
            .flex()
            .items_center()
            .gap_0p5()
            .rounded_md()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex_alpha(RAISED, 0.94))
            .shadow_lg()
            .occlude()
            .child(
                crate::ui::icon_only("db-diagram-zoom-out", "minus", cx.listener(set(|d| d.zoom.get() / ZOOM_STEP, false)))
                    .tooltip(Tooltip::text(SharedString::from(t(cx, "plugins.graph.zoom_out")), None)),
            )
            .child(
                div()
                    .id("db-diagram-zoom-reset")
                    .w(px(44.))
                    .text_center()
                    .t_caption()
                    .font_family(MONO)
                    .text_color(hex(Chrome::FOREGROUND))
                    .cursor_pointer()
                    .tooltip(Tooltip::text(SharedString::from(t(cx, "plugins.graph.zoom_reset")), None))
                    .on_click(cx.listener(set(|_| 1., false)))
                    .child(format!("{}%", (diagram.zoom.get() * 100.).round())),
            )
            .child(
                crate::ui::icon_only("db-diagram-zoom-in", "plus", cx.listener(set(|d| d.zoom.get() * ZOOM_STEP, false)))
                    .tooltip(Tooltip::text(SharedString::from(t(cx, "plugins.graph.zoom_in")), None)),
            )
            .child(
                crate::ui::icon_only("db-diagram-fit", "maximize-2", cx.listener(set(Diagram::fit_zoom, true)))
                    .tooltip(Tooltip::text(SharedString::from(t(cx, "plugins.graph.zoom_fit")), None)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referenced_tables_stand_left_of_the_tables_pointing_at_them() {
        // 0 users, 1 orders → users, 2 order_items → orders, 3 settings → users, 4 alone.
        let heights = [100., 100., 100., 100., 60.];
        let positions = layout(&heights, &[(1, 0), (2, 1), (3, 0)]);
        assert_eq!(positions[0].0, 0.);
        assert_eq!(positions[1].0, CARD_WIDTH + GAP_X);
        assert_eq!(positions[3].0, CARD_WIDTH + GAP_X);
        assert_eq!(positions[2].0, 2. * (CARD_WIDTH + GAP_X));
        // Two cards in one column do not overlap; the lone table goes under the group.
        assert!((positions[1].1 - positions[3].1).abs() >= 100. + GAP_Y);
        assert!(positions[4].1 >= positions[1].1.max(positions[3].1) + 100.);
    }

    #[test]
    fn a_cycle_or_a_self_reference_still_places_every_table() {
        let positions = layout(&[80., 80., 80.], &[(0, 1), (1, 0), (2, 2), (1, 2)]);
        assert_eq!(positions.len(), 3);
        let mut seen: Vec<(i32, i32)> = positions.iter().map(|p| (p.0 as i32, p.1 as i32)).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 3, "{positions:?}");
        assert!(layout(&[], &[]).is_empty());
    }

    #[test]
    fn rows_sit_under_the_heading_and_the_note() {
        let mut table = TableShape { name: "t".into(), kind: "table".into(), ..Default::default() };
        table.columns = (0..50).map(|i| agentty_db::schema::ColumnShape { name: format!("c{i}"), ..Default::default() }).collect();
        assert_eq!(row_middle(&table, 0), HEADING + ROW_PAD + ROW / 2.);
        // Folded columns are drawn to the "+N more" row.
        assert_eq!(row_middle(&table, 45), row_middle(&table, MAX_ROWS));
        assert_eq!(card_height(&table), HEADING + ROW_PAD * 2. + (MAX_ROWS + 1) as f32 * ROW);
        table.comment = "x".into();
        assert_eq!(row_middle(&table, 0), HEADING + NOTE + ROW_PAD + ROW / 2.);
    }

    #[test]
    fn lines_leave_from_the_facing_sides() {
        let (from, to, out, into) = ends((0., 0.), 10., (500., 0.), 20.);
        assert_eq!((from, to, out, into), ((CARD_WIDTH, 10.), (500., 20.), 1., 1.));
        let (from, to, ..) = ends((500., 0.), 10., (0., 0.), 20.);
        assert_eq!((from, to), ((500., 10.), (CARD_WIDTH, 20.)));
        // One above the other: both ends on the right.
        let (from, to, out, into) = ends((0., 0.), 10., (0., 300.), 20.);
        assert_eq!((from.0, to.0, out, into), (CARD_WIDTH, CARD_WIDTH, 1., -1.));
    }
}
