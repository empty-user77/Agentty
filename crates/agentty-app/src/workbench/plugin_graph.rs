//! Two elements of a plugin's panel that are drawn rather than laid out: `image`, a picture from
//! the plugin's own folder (and a place to drop one), and `graph`, node cards on a canvas joined by
//! wires — the way an image pipeline is shown.
//!
//! A graph's geometry is fixed by the tree, not by layout: a node's ports sit in rows of
//! [`PORT_ROW`] under its heading, so where a wire starts and ends is known before anything is
//! measured, and the wires are painted under the cards in one pass.

use super::plugin_ui::{emit, tone_color, RAISED, SUNKEN};
use super::Workbench;
use crate::i18n::t;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, Tooltip, TypeScale};
use agentty_bridge::plugins::ui::{FlowState, GraphNode, ImageFit, Node, Tone, UiEvent, GRAPH_HEIGHT, IMAGE_HEIGHT};
use gpui::{
    canvas, div, point, prelude::*, px, AnyElement, Bounds, ClickEvent, Context, ObjectFit, PathBuilder, Pixels, Point, SharedString,
    Window,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// A node's heading.
const NODE_HEADER: f32 = 32.;
/// Space above the first port row and below the last.
const PORT_PAD: f32 = 4.;
/// One port row: a port on each side.
const PORT_ROW: f32 = 22.;
const PORT_DOT: f32 = 10.;
/// The dots of the canvas, this far apart.
const GRID_STEP: f32 = 24.;
/// Room kept right of and under the farthest node, so it can be dragged further.
const CANVAS_ROOM: f32 = 320.;
const WIRE_WIDTH: f32 = 2.;
/// The canvas behind the nodes: a step darker than the panel.
const CANVAS: u32 = 0x161618;
/// How far a graph zooms out and in, and the step of its buttons.
const ZOOM_RANGE: (f32, f32) = (0.3, 2.);
const ZOOM_STEP: f32 = 1.2;
/// Below this the cards show their pictures only: controls that small could not be used.
const COMPACT_ZOOM: f32 = 0.75;
/// Room kept around the nodes when zooming to fit.
const FIT_MARGIN: f32 = 24.;

/// How the user looks at one graph: its zoom, the size of the place it is shown in, and how far
/// its nodes reach (the graph's own pixels) as last drawn — once with the cards whole and once as
/// the overview, which is shorter. Kept per `plugin/graph`, not sent to the plugin: it is the
/// user's view, like a scroll position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphView {
    pub zoom: f32,
    pub viewport: (f32, f32),
    pub extent: (f32, f32),
    pub overview: (f32, f32),
}

impl Default for GraphView {
    fn default() -> Self {
        GraphView { zoom: 1., viewport: (0., 0.), extent: (0., 0.), overview: (0., 0.) }
    }
}

pub type GraphViews = Rc<RefCell<HashMap<String, GraphView>>>;

fn clamp_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() {
        zoom.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1)
    } else {
        1.
    }
}

/// The zoom that shows every node in the place the graph has. Whole cards are taller than the
/// overview's, so the size that counts is the one of the mode the answer lands in.
fn fit_zoom(view: GraphView) -> f32 {
    let fit = |(w, h): (f32, f32)| {
        (w > 0. && h > 0. && view.viewport.0 > 0. && view.viewport.1 > 0.)
            .then(|| ((view.viewport.0 - FIT_MARGIN) / w).min((view.viewport.1 - FIT_MARGIN) / h))
    };
    match (fit(view.extent), fit(view.overview)) {
        (Some(whole), _) if whole >= COMPACT_ZOOM => whole,
        // Too small for whole cards: the overview, which never shows them.
        (_, Some(overview)) => overview.min(COMPACT_ZOOM - 0.01),
        (Some(whole), None) => whole,
        (None, None) => 1.,
    }
}

/// Files dropped from the Finder go to the drop zone under the pointer: where each one was drawn
/// in the last frame, with the plugin and element it belongs to.
#[derive(Clone)]
pub struct PluginDropZone {
    pub bounds: Bounds<Pixels>,
    pub plugin: String,
    pub element: String,
    pub into: String,
}

pub type PluginDropZones = Rc<RefCell<Vec<PluginDropZone>>>;

/// Plugin pictures, read and checked, by path as of the file's change time and size. A picture is
/// drawn only when its bytes say PNG, JPEG, WebP or GIF — never an SVG, a document the renderer
/// would open the addresses of — and it is drawn from the very bytes that were checked: handed to
/// GPUI by path, the file would be read again later and decoded by its contents, so a plugin could
/// swap in an SVG between the check and the read.
///
/// Files are read off the UI thread (a picture can be 64 MB): a picture not read yet draws as the
/// placeholder, or as the version before it while a changed file is read, and the panel is drawn
/// again when it arrives.
pub type ImageChecks = RefCell<PictureCache>;

/// A file's change time and size: when either moves, the picture is read again.
type Stamp = (SystemTime, u64);
/// Where a picture's `src` led, and the file's stamp there.
type Found = (PathBuf, Stamp);

#[derive(Default)]
pub struct PictureCache {
    entries: HashMap<PathBuf, Picture>,
    /// Reads under way, so a file is read once however many frames ask for it meanwhile.
    loading: HashSet<(PathBuf, Stamp)>,
    /// Bytes of the pictures in `entries`.
    bytes: u64,
    /// Where a plugin's `src` led and the file's stamp there, and when that was looked up: finding
    /// the file takes a lock and a look at every folder on the way, too much for every frame.
    found: HashMap<(String, String), (Instant, Option<Found>)>,
}

struct Picture {
    stamp: Stamp,
    /// `None`: the file is not a picture (or was let go, see `dropped`).
    image: Option<Arc<gpui::Image>>,
    bytes: u64,
    used: Instant,
    /// Let go while on screen, when the cache was over its hard cap, and when: drawn as the
    /// placeholder instead of being read again at once, until [`RETRY_DROPPED`] has passed.
    dropped: Option<Instant>,
}

/// What the cache has for a file as it is now.
enum Cached {
    /// Read from the file as it is.
    Fresh(Option<Arc<gpui::Image>>),
    /// Read from an earlier version of the file, or not read at all: a read is due.
    Stale(Option<Arc<gpui::Image>>),
}

/// A picture drawn this recently is on screen and is not let go, however full the cache: letting
/// one go would read it again at once, push out the next, and go round for as long as a panel
/// shows more than fits.
const ON_SCREEN: Duration = Duration::from_secs(2);
/// How long where a picture's `src` leads is trusted before it is looked up again.
const LOOKUP_EVERY: Duration = Duration::from_secs(1);
/// A picture let go while on screen is tried again after this long.
const RETRY_DROPPED: Duration = Duration::from_secs(30);

impl PictureCache {
    /// Where `src` of `plugin` leads, and the file's stamp: looked up with `look` at most once
    /// every [`LOOKUP_EVERY`].
    fn lookup(
        &mut self,
        plugin: &str,
        src: &str,
        now: Instant,
        look: impl FnOnce() -> Option<(PathBuf, Stamp)>,
    ) -> Option<(PathBuf, Stamp)> {
        if let Some((at, found)) = self.found.get(&(plugin.to_string(), src.to_string())) {
            if now.saturating_duration_since(*at) < LOOKUP_EVERY {
                return found.clone();
            }
        }
        // Pictures a panel no longer shows are forgotten, so the list stays as long as a screenful.
        if self.found.len() >= 1024 {
            self.found.retain(|_, (at, _)| now.saturating_duration_since(*at) < LOOKUP_EVERY);
        }
        let found = look();
        self.found.insert((plugin.to_string(), src.to_string()), (now, found.clone()));
        found
    }

    fn get(&mut self, path: &Path, stamp: Stamp, now: Instant) -> Cached {
        match self.entries.get_mut(path) {
            Some(picture) => {
                picture.used = now;
                let retry = picture.dropped.is_some_and(|at| now.saturating_duration_since(at) >= RETRY_DROPPED);
                if picture.stamp == stamp && !retry {
                    Cached::Fresh(picture.image.clone())
                } else {
                    Cached::Stale(picture.image.clone())
                }
            }
            None => Cached::Stale(None),
        }
    }

    /// Whether a read of `path` as of `stamp` should start: not when one already is under way.
    fn start_loading(&mut self, path: &Path, stamp: Stamp) -> bool {
        self.loading.insert((path.to_path_buf(), stamp))
    }

    /// Keeps what a read found, then lets go of the pictures used longest ago until the rest fit
    /// in `limit` — never one that is on screen ([`ON_SCREEN`]), which the one just read is. Past
    /// twice `limit` (a panel showing more than that) the oldest on screen go as well: they are
    /// drawn as placeholders, and tried again only after [`RETRY_DROPPED`].
    fn finish_loading(&mut self, path: PathBuf, stamp: Stamp, image: Option<Arc<gpui::Image>>, limit: u64, now: Instant) {
        self.loading.remove(&(path.clone(), stamp));
        let bytes = image.as_ref().map_or(0, |image| image.bytes().len() as u64);
        if let Some(old) = self.entries.insert(path, Picture { stamp, image, bytes, used: now, dropped: None }) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        while self.bytes > limit {
            let Some(oldest) = self
                .entries
                .iter()
                .filter(|(_, picture)| now.saturating_duration_since(picture.used) >= ON_SCREEN)
                .min_by_key(|(_, picture)| picture.used)
                .map(|(kept, _)| kept.clone())
            else {
                break;
            };
            if let Some(gone) = self.entries.remove(&oldest) {
                self.bytes -= gone.bytes;
            }
        }
        let hard_limit = limit.saturating_mul(2);
        while self.bytes > hard_limit {
            let Some(oldest) = self.entries.values_mut().filter(|picture| picture.bytes > 0).min_by_key(|picture| picture.used) else {
                break;
            };
            self.bytes -= oldest.bytes;
            oldest.bytes = 0;
            oldest.image = None;
            oldest.dropped = Some(now);
        }
    }
}

/// Reads the picture at `path`: at most [`MAX_PICTURE_BYTES`], and a picture by its bytes.
fn read_picture(path: &Path) -> Option<Arc<gpui::Image>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(MAX_PICTURE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_PICTURE_BYTES {
        return None;
    }
    raster_format(&bytes).map(|format| Arc::new(gpui::Image::from_bytes(format, bytes)))
}

/// The bytes of pictures kept read at once; past this the ones used longest ago are let go (a
/// panel shows a handful, and what is on screen stays whatever it adds up to).
const PICTURE_CACHE_BYTES: u64 = 256 * 1024 * 1024;
/// The largest picture read for drawing.
const MAX_PICTURE_BYTES: u64 = 64 * 1024 * 1024;

/// Nodes the user dragged, kept until the plugin's tree catches up: (`plugin/graph/node`) → the
/// place the tree had and the place it was dropped.
pub type GraphMoves = RefCell<HashMap<String, ((f32, f32), (f32, f32))>>;

/// One wire to paint: from an output, to an input (canvas pixels), its color, and whether it is idle.
struct Wire {
    from: (f32, f32),
    to: (f32, f32),
    color: u32,
    idle: bool,
}

/// A graph node being dragged by its heading: its graph, id and title, and where in the heading it
/// was taken (filled in when the drag starts).
#[derive(Clone)]
pub struct PluginGraphDrag {
    graph: String,
    node: String,
    title: String,
    width: f32,
    grab: Rc<Cell<Point<Pixels>>>,
}

impl gpui::Render for PluginGraphDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let grab = self.grab.get();
        div()
            // The ghost sits where the card was taken: under the pointer at the same spot.
            .ml(-grab.x)
            .mt(-grab.y)
            .w(px(self.width))
            .h(px(NODE_HEADER))
            .px_2()
            .flex()
            .items_center()
            .rounded_lg()
            .border_1()
            .border_color(hex(Chrome::ACCENT))
            .bg(hex_alpha(RAISED, 0.92))
            .t_small()
            .text_color(hex(Chrome::BRIGHT))
            .child(self.title.clone())
    }
}

/// What `bytes` (the start of a file) are, when they are a PNG, JPEG, WebP or GIF.
pub fn raster_format(bytes: &[u8]) -> Option<gpui::ImageFormat> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some(gpui::ImageFormat::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(gpui::ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(gpui::ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(gpui::ImageFormat::Webp)
    } else {
        None
    }
}

pub fn is_raster(bytes: &[u8]) -> bool {
    raster_format(bytes).is_some()
}

/// Whether the file at `path` is a picture by its first bytes.
pub fn file_is_raster(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 16];
    let Ok(mut file) = std::fs::File::open(path) else { return false };
    let read = file.read(&mut head).unwrap_or(0);
    is_raster(&head[..read])
}

/// Where a port's dot sits on its node, from the node's top-left corner.
fn port_offset(index: usize) -> f32 {
    NODE_HEADER + PORT_PAD + index as f32 * PORT_ROW + PORT_ROW / 2.
}

/// The wire between two ports: leaving the output to the right, arriving at the input from the
/// left, however the two are placed.
fn wire(from: Point<Pixels>, to: Point<Pixels>, zoom: f32) -> Option<gpui::Path<Pixels>> {
    let reach = px((f32::from(to.x - from.x).abs() / 2.).max(60. * zoom));
    let mut path = PathBuilder::stroke(px((WIRE_WIDTH * zoom).max(1.)));
    path.move_to(from);
    path.cubic_bezier_to(to, point(from.x + reach, from.y), point(to.x - reach, to.y));
    path.build().ok()
}

impl Workbench {
    /// A plugin's picture, when it is one: inside the plugin's folder (no way out of it, no link),
    /// there, and a picture by its bytes — read once per change of the file, in the background.
    fn plugin_picture(&self, plugin: &str, src: &str, cx: &mut Context<Self>) -> Option<Arc<gpui::Image>> {
        let mut cache = self.plugin_image_checks.borrow_mut();
        let (path, stamp) = cache.lookup(plugin, src, Instant::now(), || {
            let path = agentty_bridge::plugins::files::resolve(plugin, src).ok()?;
            let meta = std::fs::metadata(&path).ok().filter(|m| m.is_file() && m.len() <= MAX_PICTURE_BYTES)?;
            Some((path, (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len())))
        })?;
        let shown = match cache.get(&path, stamp, Instant::now()) {
            Cached::Fresh(image) => return image,
            Cached::Stale(image) => image,
        };
        if cache.start_loading(&path, stamp) {
            let read = path.clone();
            let task = cx.background_executor().spawn(async move { read_picture(&read) });
            cx.spawn(async move |this, cx| {
                let image = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.plugin_image_checks.borrow_mut().finish_loading(path, stamp, image, PICTURE_CACHE_BYTES, Instant::now());
                    cx.notify();
                });
            })
            .detach();
        }
        shown
    }

    /// Whether the plugin may take files dropped on it: the same permission `files/pick` needs.
    fn plugin_takes_files(&self, plugin: &str, cx: &gpui::App) -> bool {
        crate::plugins::plugin(cx, plugin).and_then(|p| p.manifest.as_ref()).is_some_and(|m| m.permissions.iter().any(|p| p == "files"))
    }

    pub(super) fn render_plugin_image(&self, plugin: &str, node: &Node, path: &[usize], cx: &mut Context<Self>) -> AnyElement {
        let Node::Image { src, alt, width, height, fit, caption, placeholder, id, drop, into } = node else {
            return div().into_any_element();
        };
        let file = src.as_deref().and_then(|src| self.plugin_picture(plugin, src, cx));
        let takes_drop = *drop && id.is_some() && self.plugin_takes_files(plugin, cx);
        let picture = match &file {
            Some(picture) => gpui::img(picture.clone())
                .size_full()
                .object_fit(match fit {
                    ImageFit::Contain => ObjectFit::Contain,
                    ImageFit::Cover => ObjectFit::Cover,
                })
                .into_any_element(),
            None => {
                let words = placeholder.clone().or_else(|| alt.clone()).unwrap_or_else(|| {
                    if takes_drop {
                        t(cx, "plugins.image.drop").to_string()
                    } else {
                        String::new()
                    }
                });
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_1p5()
                    .p_3()
                    .child(icon("image", IconSize::BUTTON, hex(Chrome::MUTED)))
                    .when(!words.is_empty(), |d| d.child(div().t_small().text_color(hex(Chrome::MUTED)).text_center().child(words)))
                    .into_any_element()
            }
        };
        let element_id = SharedString::from(format!("plugin-image-{plugin}-{}-{path:?}", id.as_deref().unwrap_or("")));
        // Inside a zoomed graph node, the picture follows the zoom.
        let scale = self.plugin_graph_scale.get();
        let mut frame = div()
            .id(element_id)
            .relative()
            .when_some(*width, |d, w| d.w(px(w * scale)))
            .when(width.is_none(), |d| d.w_full())
            .h(px(height.unwrap_or(IMAGE_HEIGHT) * scale))
            .flex_shrink_0()
            .rounded_md()
            .overflow_hidden()
            .border_1()
            .border_color(hex(if file.is_some() { Chrome::BORDER } else { 0x3a3a40 }))
            .bg(hex(SUNKEN))
            .child(picture);
        if let Some(element) = id.clone() {
            frame = frame.cursor_pointer().hover(|s| s.border_color(hex(Chrome::ACCENT))).on_click(cx.listener(emit(
                plugin.to_string(),
                element,
                "click",
                None,
                None,
            )));
        }
        if takes_drop {
            // Where it was drawn, for a drop from the Finder (see `drop_files_at`).
            let zones = self.plugin_drop_zones.clone();
            let zone = PluginDropZone {
                bounds: Bounds::default(),
                plugin: plugin.to_string(),
                element: id.clone().unwrap_or_default(),
                into: into.clone().unwrap_or_else(|| "dropped".into()),
            };
            frame = frame.child(
                canvas(move |bounds, _, _| zones.borrow_mut().push(PluginDropZone { bounds, ..zone.clone() }), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_1()
            .min_w_0()
            .child(frame)
            .children(caption.clone().map(|c| {
                div().t_caption().font_family(super::plugin_ui::MONO).text_color(hex(Chrome::MUTED)).text_center().truncate().child(c)
            }))
            .into_any_element()
    }

    /// Files dropped from the Finder at `point`: a plugin's picture slot there takes the pictures
    /// among them, copied into its folder, and hears `drop` with `[{ path, name, size }]`.
    pub(super) fn drop_on_plugin(&mut self, at: Point<Pixels>, paths: &[PathBuf], cx: &mut Context<Self>) -> bool {
        // The last drawn is the one on top.
        let Some(zone) = self.plugin_drop_zones.borrow().iter().rev().find(|z| z.bounds.contains(&at)).cloned() else {
            return false;
        };
        let pictures: Vec<PathBuf> = paths.iter().filter(|p| p.is_file() && file_is_raster(p)).cloned().collect();
        if pictures.is_empty() {
            return true;
        }
        let copied = agentty_bridge::plugins::files::resolve(&zone.plugin, &zone.into)
            .and_then(|folder| crate::plugins::copy_picked(&pictures, &folder, &zone.into));
        match copied {
            Ok(files) => {
                let event = UiEvent { element: zone.element, event: "drop".into(), value: Some(files), item: None, action: None };
                self.send_plugin_event(&zone.plugin, event, cx);
            }
            Err(err) => crate::plugins::log(&zone.plugin, format!("a drop could not be copied: {err:#}"), cx),
        }
        true
    }

    pub(super) fn render_plugin_graph(&self, plugin: &str, node: &Node, path: &mut Vec<usize>, cx: &mut Context<Self>) -> AnyElement {
        let Node::Graph { id, nodes, edges, height, movable, fill } = node else { return div().into_any_element() };
        let graph_key = format!("{plugin}/{id}");
        let view = self.plugin_graph_views.borrow().get(&graph_key).copied().unwrap_or_default();
        let z = view.zoom;

        // Where each node is: the tree's place, or where the user just dropped it while the
        // plugin has not sent the tree back yet.
        let mut moves = self.plugin_graph_moves.borrow_mut();
        let placed: Vec<(f32, f32)> = nodes
            .iter()
            .map(|n| {
                let key = format!("{graph_key}/{}", n.id);
                match moves.get(&key) {
                    Some((from, to)) if *from == (n.x, n.y) => *to,
                    Some(_) => {
                        moves.remove(&key);
                        (n.x, n.y)
                    }
                    None => (n.x, n.y),
                }
            })
            .collect();
        drop(moves);

        // The canvas: what the nodes take (as last measured) and room around them, never less
        // than the place it is shown in.
        let right = nodes.iter().zip(&placed).map(|(n, p)| p.0 + n.shown_width()).fold(0., f32::max);
        let measured = if z < COMPACT_ZOOM { view.overview.1 } else { view.extent.1 };
        let bottom = placed.iter().map(|p| p.1 + NODE_HEADER).fold(measured, f32::max);
        let content_w = ((right + CANVAS_ROOM) * z).max(view.viewport.0);
        let content_h = ((bottom + CANVAS_ROOM) * z).max(view.viewport.1);

        // Wires, from the geometry alone.
        let index: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
        let wires: Vec<Wire> = edges
            .iter()
            .filter_map(|e| {
                let (a, b) = (*index.get(e.from.node.as_str())?, *index.get(e.to.node.as_str())?);
                let out = nodes[a].outputs.iter().position(|p| p.id == e.from.port)?;
                let inp = nodes[b].inputs.iter().position(|p| p.id == e.to.port)?;
                let from = ((placed[a].0 + nodes[a].shown_width()) * z, (placed[a].1 + port_offset(out)) * z);
                let to = (placed[b].0 * z, (placed[b].1 + port_offset(inp)) * z);
                let tone = e.tone.unwrap_or(nodes[a].outputs[out].tone);
                Some(Wire { from, to, color: tone_color(tone), idle: e.idle })
            })
            .collect();

        let origin: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        let painted = origin.clone();
        let step = GRID_STEP * z;
        let backdrop = canvas(
            move |bounds, _, _| painted.set(Some(bounds)),
            move |bounds, _, window, _| {
                let o = bounds.origin;
                // The dot grid, only where it can be seen.
                let mask = window.content_mask().bounds;
                let dot = hex_alpha(0xffffff, 0.07);
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
                for w in &wires {
                    let a = point(o.x + px(w.from.0), o.y + px(w.from.1));
                    let b = point(o.x + px(w.to.0), o.y + px(w.to.1));
                    if let Some(path) = wire(a, b, z) {
                        window.paint_path(path, hex_alpha(w.color, if w.idle { 0.28 } else { 0.85 }));
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        // How far down the nodes reach, measured as they are drawn, for the canvas and for fit.
        let reach: Rc<Cell<f32>> = Rc::new(Cell::new(0.));
        let mut canvas_area = div().relative().w(px(content_w)).h(px(content_h)).child(backdrop);
        for (i, graph_node) in nodes.iter().enumerate() {
            path.push(i);
            canvas_area = canvas_area.child(self.render_graph_node(
                plugin,
                id,
                &graph_key,
                graph_node,
                placed[i],
                *movable,
                z,
                reach.clone(),
                path,
                cx,
            ));
            path.pop();
        }
        if *movable {
            let (owner, element, key) = (plugin.to_string(), id.clone(), graph_key.clone());
            let start: HashMap<String, (f32, f32)> = nodes.iter().zip(&placed).map(|(n, p)| (n.id.clone(), *p)).collect();
            canvas_area = canvas_area.on_drop(cx.listener(move |this, drag: &PluginGraphDrag, window, cx| {
                let (Some(bounds), true) = (origin.get(), drag.graph == key) else { return };
                let at = window.mouse_position() - bounds.origin - drag.grab.get();
                // Back to the graph's own pixels, whatever the zoom.
                let to = ((f32::from(at.x) / z).max(0.).round(), (f32::from(at.y) / z).max(0.).round());
                let from = start.get(&drag.node).copied().unwrap_or_default();
                if from == to {
                    return;
                }
                this.plugin_graph_moves.borrow_mut().insert(format!("{key}/{}", drag.node), (from, to));
                let event = UiEvent {
                    element: element.clone(),
                    event: "move".into(),
                    value: Some(serde_json::json!({ "x": to.0, "y": to.1 })),
                    item: Some(drag.node.clone()),
                    action: None,
                };
                this.send_plugin_event(&owner, event, cx);
                cx.notify();
            }));
        }

        // The place the graph is shown in: its size (for fit and for a canvas that fills it), and
        // ⌘ or Ctrl with the wheel to zoom — taken before the canvas scrolls with it.
        let views = self.plugin_graph_views.clone();
        let key = graph_key.clone();
        let measure = canvas(
            {
                let (views, key) = (views.clone(), key.clone());
                move |bounds, _, _| {
                    let mut views = views.borrow_mut();
                    let view = views.entry(key.clone()).or_default();
                    view.viewport = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                    bounds
                }
            },
            move |bounds, _, window, _| {
                // Painted after every card was laid out: how far they reach is known now.
                if let Some(view) = views.borrow_mut().get_mut(&key) {
                    let reached = (right, reach.get() / z);
                    if z < COMPACT_ZOOM {
                        view.overview = reached;
                    } else {
                        view.extent = reached;
                    }
                }
                let (views, key) = (views.clone(), key.clone());
                window.on_mouse_event(move |event: &gpui::ScrollWheelEvent, phase, window, cx| {
                    let zooming = event.modifiers.platform || event.modifiers.control;
                    if phase != gpui::DispatchPhase::Capture || !zooming || !bounds.contains(&event.position) {
                        return;
                    }
                    let dy = f32::from(event.delta.pixel_delta(px(16.)).y);
                    if dy != 0. {
                        let mut views = views.borrow_mut();
                        let view = views.entry(key.clone()).or_default();
                        view.zoom = clamp_zoom(view.zoom * (1. + dy * 0.004));
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
            .w_full()
            .when(*fill, |d| d.flex_1().min_h_0().h_full())
            .when(!*fill, |d| d.h(px(height.unwrap_or(GRAPH_HEIGHT))).flex_shrink_0())
            .rounded_lg()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(CANVAS))
            .overflow_hidden()
            .child(measure)
            .child(div().id(SharedString::from(format!("plugin-graph-{graph_key}"))).size_full().overflow_scroll().child(canvas_area))
            .child(self.render_graph_zoom(&graph_key, view, cx))
            .into_any_element()
    }

    /// − 100% + and fit, at the canvas's bottom right corner.
    fn render_graph_zoom(&self, graph_key: &str, view: GraphView, cx: &mut Context<Self>) -> AnyElement {
        let set = |zoom: fn(GraphView) -> f32| {
            let (views, key) = (self.plugin_graph_views.clone(), graph_key.to_string());
            move |_: &ClickEvent, window: &mut Window, _: &mut gpui::App| {
                let mut views = views.borrow_mut();
                let view = views.entry(key.clone()).or_default();
                view.zoom = clamp_zoom(zoom(*view));
                window.refresh();
            }
        };
        let button = |id: &str, glyph: &'static str, tip: SharedString, zoom: fn(GraphView) -> f32| {
            crate::ui::icon_only(SharedString::from(format!("plugin-graph-{id}-{graph_key}")), glyph, set(zoom))
                .tooltip(Tooltip::text(tip, None))
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
            .child(button("out", "minus", t(cx, "plugins.graph.zoom_out").into(), |v| v.zoom / ZOOM_STEP))
            .child(
                div()
                    .id(SharedString::from(format!("plugin-graph-reset-{graph_key}")))
                    .w(px(44.))
                    .text_center()
                    .t_caption()
                    .font_family(super::plugin_ui::MONO)
                    .text_color(hex(Chrome::FOREGROUND))
                    .cursor_pointer()
                    .tooltip(Tooltip::text(SharedString::from(t(cx, "plugins.graph.zoom_reset")), None))
                    .on_click(set(|_| 1.))
                    .child(format!("{}%", (view.zoom * 100.).round())),
            )
            .child(button("in", "plus", t(cx, "plugins.graph.zoom_in").into(), |v| v.zoom * ZOOM_STEP))
            .child(button("fit", "maximize-2", t(cx, "plugins.graph.zoom_fit").into(), fit_zoom))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_graph_node(
        &self,
        plugin: &str,
        graph: &str,
        graph_key: &str,
        node: &GraphNode,
        at: (f32, f32),
        movable: bool,
        z: f32,
        reach: Rc<Cell<f32>>,
        path: &mut Vec<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let width = node.shown_width() * z;
        let accent = if node.tone == Tone::Neutral { Chrome::MUTED } else { tone_color(node.tone) };
        let border = if node.selected {
            hex(Chrome::ACCENT)
        } else {
            match node.state {
                FlowState::Active => hex_alpha(Chrome::ORANGE, 0.7),
                FlowState::Error => hex_alpha(Chrome::ERROR, 0.7),
                _ if node.tone != Tone::Neutral => hex_alpha(accent, 0.35),
                _ => hex(Chrome::BORDER),
            }
        };
        let mark = match node.state {
            FlowState::Active => Some(
                crate::ui::dot_spinner(
                    SharedString::from(format!("plugin-graph-spin-{graph_key}-{}", node.id)),
                    12. * z,
                    hex(Chrome::ORANGE),
                )
                .into_any_element(),
            ),
            FlowState::Done => Some(icon("circle-check", IconSize::INLINE, hex(Chrome::SUCCESS)).into_any_element()),
            FlowState::Error => Some(icon("circle-x", IconSize::INLINE, hex(Chrome::ERROR)).into_any_element()),
            _ => None,
        };
        // Text of the card's own parts follows the zoom; what the plugin put inside keeps the
        // app's sizes, and is left out when the graph is zoomed far out (an overview of pictures).
        let (title_size, caption_size) = (px(12.5 * z), px(10.5 * z));
        let compact = z < COMPACT_ZOOM;

        let mut heading = div()
            .id(SharedString::from(format!("plugin-graph-head-{graph_key}-{}", node.id)))
            .h(px(NODE_HEADER * z))
            .px(px(10. * z))
            .flex()
            .items_center()
            .gap(px(8. * z))
            .rounded_t_lg()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(0x202024))
            .cursor_pointer()
            .on_click(cx.listener(emit(plugin.to_string(), graph.to_string(), "select", None, Some(node.id.clone()))))
            .child(div().size(px(8. * z)).flex_shrink_0().rounded_full().bg(hex(accent)))
            .children(node.icon.as_deref().map(|g| icon(crate::ui::icon_named(Some(g)), IconSize::INLINE, hex(Chrome::FOREGROUND))))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(title_size)
                    .font_weight(crate::theme::EMPHASIS)
                    .text_color(hex(Chrome::BRIGHT))
                    .child(node.title.clone()),
            )
            .children(node.subtitle.clone().filter(|_| !compact).map(|s| {
                div()
                    .flex_shrink_0()
                    .max_w(px(width * 0.45))
                    .truncate()
                    .text_size(caption_size)
                    .font_family(super::plugin_ui::MONO)
                    .text_color(hex(Chrome::MUTED))
                    .child(s)
            }))
            .children(mark);
        if movable {
            let drag = PluginGraphDrag {
                graph: graph_key.to_string(),
                node: node.id.clone(),
                title: node.title.clone(),
                width,
                grab: Rc::new(Cell::new(Point::default())),
            };
            heading = heading.cursor_grab().on_drag(drag, |drag, offset, _, cx| {
                drag.grab.set(offset);
                cx.new(|_| drag.clone())
            });
        }

        // The ports: a row each under the heading, the label inside the card and the dot on its
        // edge. The dots hang off the card itself (an absolute element is placed against its
        // parent), at the same height the wires are drawn to.
        let rows = node.inputs.len().max(node.outputs.len());
        let ports = (rows > 0).then(|| {
            let mut block = div().py(px(PORT_PAD * z)).flex().flex_col();
            for row in 0..rows {
                let label = |port: Option<&agentty_bridge::plugins::ui::GraphPort>, right: bool| {
                    div().flex_1().min_w_0().flex().when(right, |d| d.justify_end()).children(port.map(|p| {
                        div()
                            .truncate()
                            .text_size(caption_size)
                            .text_color(hex(Chrome::MUTED))
                            .child(p.label.clone().unwrap_or_else(|| p.id.clone()))
                    }))
                };
                block = block.child(
                    div()
                        .h(px(PORT_ROW * z))
                        .px(px(10. * z))
                        .flex()
                        .items_center()
                        .gap(px(8. * z))
                        .child(label(node.inputs.get(row), false))
                        .child(label(node.outputs.get(row), true)),
                );
            }
            block
        });
        let size = (PORT_DOT * z).max(6.);
        let dot = |index: usize, tone: Tone, right: bool| {
            div()
                .absolute()
                .size(px(size))
                .rounded_full()
                .border_2()
                .border_color(hex(RAISED))
                .bg(hex(tone_color(tone)))
                // The card's border is inside its box: measured from its outer corner, like the wires.
                .top(px(port_offset(index) * z - size / 2. - 1.))
                .when(right, |d| d.right(px(-size / 2. - 1.)))
                .when(!right, |d| d.left(px(-size / 2. - 1.)))
        };
        let dots: Vec<_> = node
            .inputs
            .iter()
            .enumerate()
            .map(|(i, p)| dot(i, p.tone, false))
            .chain(node.outputs.iter().enumerate().map(|(i, p)| dot(i, p.tone, true)))
            .collect();

        // Pictures inside follow the zoom too (see `render_plugin_image`).
        let shown: Vec<Node> = if compact {
            node.children.iter().filter(|c| matches!(c, Node::Image { .. })).cloned().collect()
        } else {
            node.children.clone()
        };
        let body = (!shown.is_empty()).then(|| {
            self.plugin_graph_scale.set(z);
            let body = self.render_plugin_children(
                plugin,
                &shown,
                path,
                div().p(px(10. * z)).pt(px(4. * z)).flex().flex_col().gap(px(8. * z)).min_w_0(),
                cx,
            );
            self.plugin_graph_scale.set(1.);
            body
        });

        let top = at.1 * z;
        div()
            .absolute()
            .left(px(at.0 * z))
            .top(px(top))
            .w(px(width))
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(border)
            .bg(hex(RAISED))
            .shadow_lg()
            .when(node.state == FlowState::Off, |d| d.opacity(0.55))
            .child(heading)
            .children(ports)
            .children(body)
            .children(dots)
            .child(
                canvas(
                    move |bounds, _, _| {
                        // The canvas starts at the card's top: its bottom is how far the graph reaches.
                        reach.set(reach.get().max(top + f32::from(bounds.size.height)));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_raster_pictures_are_drawn() {
        assert!(is_raster(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0]));
        assert!(is_raster(&[0xff, 0xd8, 0xff, 0xe0]));
        assert!(is_raster(b"GIF89a....."));
        assert!(is_raster(b"RIFF\x24\x00\x00\x00WEBPVP8 "));
        assert!(!is_raster(b"<svg xmlns=\"http://www.w3.org/2000/svg\">"));
        assert!(!is_raster(b"RIFF\x24\x00\x00\x00WAVEfmt "));
        assert!(!is_raster(b""));
        assert_eq!(raster_format(&[0xff, 0xd8, 0xff, 0xdb]), Some(gpui::ImageFormat::Jpeg));
    }

    fn picture(bytes: usize) -> Option<Arc<gpui::Image>> {
        Some(Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, vec![0; bytes])))
    }

    fn stamp(n: u64) -> Stamp {
        (SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(n), n)
    }

    fn is_fresh(cached: Cached) -> bool {
        matches!(cached, Cached::Fresh(Some(_)))
    }

    #[test]
    fn a_full_cache_lets_go_of_the_picture_used_longest_ago_only() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut cache = PictureCache::default();
        for (name, seconds) in [("a", 0), ("b", 10), ("c", 20)] {
            assert!(cache.start_loading(Path::new(name), stamp(1)));
            cache.finish_loading(name.into(), stamp(1), picture(40), 100, at(seconds));
        }
        // Three of 40 bytes do not fit in 100: `a`, read first and not drawn since, went.
        assert!(matches!(cache.get(Path::new("a"), stamp(1), at(21)), Cached::Stale(None)));
        assert_eq!(cache.bytes, 80);
        // `b` is drawn, then `d` arrives: `c` is the one used longest ago now, not `b`.
        assert!(is_fresh(cache.get(Path::new("b"), stamp(1), at(30))));
        cache.finish_loading("d".into(), stamp(1), picture(40), 100, at(40));
        assert!(is_fresh(cache.get(Path::new("b"), stamp(1), at(41))));
        assert!(is_fresh(cache.get(Path::new("d"), stamp(1), at(41))));
        assert!(matches!(cache.get(Path::new("c"), stamp(1), at(41)), Cached::Stale(None)));
        assert_eq!(cache.bytes, 80);
    }

    #[test]
    fn what_is_on_screen_stays_however_much_it_is() {
        // A panel showing more than fits: letting one go would read it again on the next frame
        // and push out another, for as long as the panel is open. So they all stay (up to twice
        // the limit)...
        let now = Instant::now();
        let mut cache = PictureCache::default();
        for name in ["a", "b", "c", "large"] {
            cache.finish_loading(name.into(), stamp(1), picture(if name == "large" { 80 } else { 40 }), 100, now);
        }
        assert_eq!(cache.entries.len(), 4);
        assert_eq!(cache.bytes, 200);
        // ...until the panel shows something else: then the ones no longer drawn go.
        let later = now + ON_SCREEN * 2;
        assert!(is_fresh(cache.get(Path::new("b"), stamp(1), later)));
        cache.finish_loading("e".into(), stamp(1), picture(40), 100, later);
        let mut kept: Vec<&str> = cache.entries.keys().map(|path| path.to_str().unwrap()).collect();
        kept.sort();
        assert_eq!(kept, ["b", "e"]);
        assert_eq!(cache.bytes, 80);
    }

    #[test]
    fn past_the_hard_cap_the_oldest_on_screen_go_and_wait_before_a_new_read() {
        let start = Instant::now();
        let mut cache = PictureCache::default();
        for (name, ms) in [("a", 0), ("b", 10), ("c", 20), ("huge", 30)] {
            let size = if name == "huge" { 150 } else { 40 };
            cache.finish_loading(name.into(), stamp(1), picture(size), 100, start + Duration::from_millis(ms));
        }
        // 270 bytes, all on screen, over the hard cap of 200: `a` and `b`, drawn longest ago, go.
        assert_eq!(cache.bytes, 190);
        let now = start + Duration::from_millis(40);
        // A dropped picture is not read again on the next frame (that would go round)...
        assert!(matches!(cache.get(Path::new("a"), stamp(1), now), Cached::Fresh(None)));
        assert!(is_fresh(cache.get(Path::new("c"), stamp(1), now)));
        // ...only once a while has passed.
        assert!(matches!(cache.get(Path::new("a"), stamp(1), now + RETRY_DROPPED), Cached::Stale(None)));
    }

    #[test]
    fn where_a_picture_is_is_looked_up_once_a_second() {
        let start = Instant::now();
        let mut cache = PictureCache::default();
        let looks = Cell::new(0);
        let look = || {
            looks.set(looks.get() + 1);
            Some((PathBuf::from("files/a.png"), stamp(1)))
        };
        for ms in [0, 100, 900] {
            assert!(cache.lookup("demo", "a.png", start + Duration::from_millis(ms), look).is_some());
        }
        assert_eq!(looks.get(), 1);
        cache.lookup("demo", "b.png", start, look);
        assert_eq!(looks.get(), 2, "another picture is its own lookup");
        cache.lookup("demo", "a.png", start + LOOKUP_EVERY, look);
        assert_eq!(looks.get(), 3);
    }

    #[test]
    fn a_changed_file_is_read_once_and_drawn_as_before_meanwhile() {
        let now = Instant::now();
        let mut cache = PictureCache::default();
        assert!(cache.start_loading(Path::new("a"), stamp(1)));
        assert!(!cache.start_loading(Path::new("a"), stamp(1)), "one read per version, however many frames ask");
        cache.finish_loading("a".into(), stamp(1), picture(10), 100, now);
        // The file changed: the old picture still shows until the new one is read.
        assert!(matches!(cache.get(Path::new("a"), stamp(2), now), Cached::Stale(Some(_))));
        assert!(cache.start_loading(Path::new("a"), stamp(2)));
        cache.finish_loading("a".into(), stamp(2), None, 100, now);
        // It is not a picture any more: nothing is drawn, and nothing is counted.
        assert!(matches!(cache.get(Path::new("a"), stamp(2), now), Cached::Fresh(None)));
        assert_eq!(cache.bytes, 0);
    }

    #[test]
    fn only_a_picture_is_read_for_drawing() {
        let dir = std::env::temp_dir().join(format!("agentty-picture-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        std::fs::write(&png, [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]).unwrap();
        assert_eq!(read_picture(&png).unwrap().bytes().len(), 11);
        let svg = dir.join("a.svg");
        std::fs::write(&svg, b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
        assert!(read_picture(&svg).is_none());
        assert!(read_picture(&dir.join("missing.png")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ports_sit_in_rows_under_the_heading() {
        assert_eq!(port_offset(0), NODE_HEADER + PORT_PAD + PORT_ROW / 2.);
        assert_eq!(port_offset(2) - port_offset(1), PORT_ROW);
    }

    #[test]
    fn a_wire_is_a_path() {
        assert!(wire(point(px(0.), px(0.)), point(px(200.), px(80.)), 1.).is_some());
        // Backwards (input left of the output) still draws.
        assert!(wire(point(px(300.), px(0.)), point(px(10.), px(10.)), 0.5).is_some());
    }

    #[test]
    fn zoom_is_bounded_and_fits_the_nodes() {
        assert_eq!(clamp_zoom(10.), ZOOM_RANGE.1);
        assert_eq!(clamp_zoom(0.01), ZOOM_RANGE.0);
        assert_eq!(clamp_zoom(f32::NAN), 1.);
        // Whole cards reaching 1200 × 700 in a place of 1224 × 724: as drawn.
        let view = GraphView { zoom: 1., viewport: (1224., 724.), extent: (1200., 700.), overview: (1200., 400.) };
        assert_eq!(fit_zoom(view), 1.);
        // In half the width whole cards would be too small: the overview, cut to stay one.
        let narrow = GraphView { viewport: (624., 724.), ..view };
        assert_eq!(fit_zoom(narrow), 0.5);
        let tiny = GraphView { viewport: (324., 724.), ..view };
        assert_eq!(fit_zoom(tiny), 0.25);
        // Measured only as the overview, a fit that lands on whole cards is still taken.
        let unmeasured = GraphView { extent: (0., 0.), ..view };
        assert!(fit_zoom(unmeasured) < COMPACT_ZOOM);
        assert_eq!(fit_zoom(GraphView::default()), 1., "nothing measured yet: as drawn");
    }
}
