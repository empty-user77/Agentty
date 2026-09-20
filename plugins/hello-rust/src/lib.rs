//! Hello world for Agentty's WebAssembly plugins: a panel with a counter and a button.
//!
//! Build it with `./build.sh`, then add it in Agentty: **Plugins → Install from folder** and pick
//! this folder. Its icon appears in the activity bar on the left, because `agentty-plugin.json`
//! asks for the `sidebar` surface.
//!
//! The plugin asks for no permissions at all, so it can draw its panel and nothing else. Run it
//! and look at the module with `wasm-objdump -x` if you like: the only imports are Agentty's
//! three, and there is no way from here to a file, a socket or a process.

use agentty_plugin::{export_plugin, ui, Host, Plugin, UiEvent};

#[derive(Default)]
struct Hello {
    clicks: u32,
    name: String,
}

impl Hello {
    fn draw(&self, host: &Host) {
        let greeting = match self.name.trim() {
            "" => "Hello, world!".to_string(),
            name => format!("Hello, {name}!"),
        };
        host.set_panel(ui::column(vec![
            ui::styled_text(greeting, "title"),
            ui::text(match self.clicks {
                0 => "The button has not been pressed yet.".to_string(),
                1 => "Pressed once.".to_string(),
                n => format!("Pressed {n} times."),
            }),
            ui::input("name", "Your name", self.name.clone()),
            ui::row(vec![ui::styled_button("press", "Press me", "primary"), ui::button("reset", "Reset")]),
            ui::divider(),
            ui::styled_text("This plugin is a Rust program compiled to WebAssembly. It reaches nothing outside Agentty.", "muted"),
        ]));
    }
}

impl Plugin for Hello {
    fn init(&mut self, host: &Host, info: &agentty_plugin::serde_json::Value) {
        host.log(format!("hello-rust started as {}", info.get("id").and_then(|id| id.as_str()).unwrap_or("?")));
    }

    fn panel_open(&mut self, host: &Host) {
        self.draw(host);
    }

    fn command(&mut self, host: &Host, _command: &str) {
        host.show_panel();
        self.draw(host);
    }

    fn ui_event(&mut self, host: &Host, event: UiEvent) {
        match event.element.as_str() {
            "press" => {
                self.clicks += 1;
                host.set_badge(self.clicks.to_string());
            }
            "reset" => {
                self.clicks = 0;
                self.name.clear();
                host.set_badge("");
            }
            "name" => self.name = event.text(),
            _ => return,
        }
        self.draw(host);
    }
}

export_plugin!(Hello);
