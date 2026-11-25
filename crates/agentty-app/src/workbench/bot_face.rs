//! The characters bots are drawn as: one tile each (`assets/bots`), with what the bot is doing at
//! its corner.

use super::bots::BotState;
use crate::theme::{hex, Chrome};
use agentty_bridge::bots::Bot;
use gpui::{div, prelude::*, px, Div, SharedString};

/// The character `name` (one of `agentty_bridge::bots::AVATARS`) on `color`, at `size` points.
pub(super) fn face(name: &str, color: u32, size: f32) -> Div {
    div()
        .flex_none()
        .size(px(size))
        .rounded(px(size * 0.2375))
        .bg(hex(color))
        .child(gpui::img(SharedString::from(format!("bots/{name}.svg"))).size(px(size)))
}

/// The character each of `agentty_bridge::bots::AVATARS` stands on when no colour was chosen.
pub(super) fn own_color(name: &str) -> u32 {
    use agentty_bridge::bots::{AVATARS, AVATAR_COLORS};
    AVATAR_COLORS[AVATARS.iter().position(|avatar| *avatar == name).unwrap_or(0)].1
}

/// A picture of the user's own, cut to the tile.
pub(super) fn picture(path: std::path::PathBuf, size: f32) -> Div {
    div().flex_none().size(px(size)).child(gpui::img(path).size(px(size)).rounded(px(size * 0.2375)).object_fit(gpui::ObjectFit::Cover))
}

/// What a bot is drawn as: its own picture when it has one, else its character.
pub(super) fn tile(bot: &Bot, size: f32) -> Div {
    match crate::bots::store().image_path(bot) {
        Some(path) => picture(path, size),
        None => face(bot.avatar(), bot.avatar_color(), size),
    }
}

/// How each bot is drawn, by its handle, for what draws a bot it only knows by name (a message
/// in a chat). Kept as the list is read; drawing never reads the bots' folders.
static LOOKS: std::sync::Mutex<Vec<Look>> = std::sync::Mutex::new(Vec::new());

struct Look {
    handle: String,
    character: &'static str,
    color: u32,
    picture: Option<std::path::PathBuf>,
}

pub(super) fn remember(bots: &[Bot]) {
    if let Ok(mut looks) = LOOKS.lock() {
        *looks = bots
            .iter()
            .map(|bot| Look {
                handle: bot.handle.clone(),
                character: bot.avatar(),
                color: bot.avatar_color(),
                picture: crate::bots::store().image_path(bot),
            })
            .collect();
    }
}

/// The bot `@handle` as it is drawn, when there is such a bot.
pub(super) fn of_handle(handle: &str, size: f32) -> Option<Div> {
    let looks = LOOKS.lock().ok()?;
    let look = looks.iter().find(|look| look.handle == handle)?;
    Some(match &look.picture {
        Some(path) => picture(path.clone(), size),
        None => face(look.character, look.color, size),
    })
}

/// A bot's character and its state: a spinner at the corner while it works, a dot while it waits
/// for the user, faded when it is switched off. `ground` is the colour it stands on, which rings
/// the corner mark.
pub(super) fn avatar(bot: &Bot, state: BotState, size: f32, ground: u32) -> Div {
    let mark = (size * 0.4).max(12.);
    let corner = |child: gpui::AnyElement| {
        div()
            .absolute()
            .right(px(-mark * 0.25))
            .bottom(px(-mark * 0.25))
            .size(px(mark))
            .rounded_full()
            .bg(hex(ground))
            .flex()
            .items_center()
            .justify_center()
            .child(child)
    };
    let tile = tile(bot, size);
    div()
        .relative()
        .flex_none()
        .size(px(size))
        .child(if state == BotState::Off { tile.opacity(0.4) } else { tile })
        .when(state == BotState::Working, |d| {
            d.child(corner(
                crate::ui::dot_spinner(SharedString::from(format!("bot-face-{}-{size}", bot.handle)), mark * 0.72, hex(Chrome::ORANGE))
                    .into_any_element(),
            ))
        })
        .when(state == BotState::NeedsYou, |d| {
            d.child(corner(div().size(px(mark * 0.62)).rounded_full().bg(hex(Chrome::ATTENTION)).into_any_element()))
        })
}
