mod line;
mod tab;

use std::cmp::{max, min};
use std::collections::BTreeMap;
use std::convert::TryInto;

use tab::get_tab_to_focus;
use zellij_tile::prelude::*;

use crate::line::tab_line;
use crate::tab::tab_style;

#[derive(Debug, Default)]
pub struct LinePart {
    part: String,
    len: usize,
    tab_index: Option<usize>,
}

impl LinePart {
    pub fn append(&mut self, to_append: &LinePart) {
        self.part.push_str(&to_append.part);
        self.len += to_append.len;
    }
}

#[derive(Default, Debug)]
struct State {
    tabs: Vec<TabInfo>,
    active_tab_idx: usize,
    mode_info: ModeInfo,
    tab_line: Vec<LinePart>,
    hide_swap_layout_indication: bool,
    cached_keybinds: KeybindsVec,
    replace_pane: Option<u32>,
    permissions_granted: bool,
    own_tab_position: Option<usize>,
    panes: PaneManifest,
    badge_command: Option<String>,
}

static ARROW_SEPARATOR: &str = "";

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.replace_pane = configuration
            .get("replace_pane_id")
            .and_then(|id| id.parse().ok());
        self.badge_command = configuration.get("badge_command").cloned();
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::RunCommands,
        ]);
        self.hide_swap_layout_indication = configuration
            .get("hide_swap_layout_indication")
            .map(|s| s == "true")
            .unwrap_or(false);
        subscribe(&[
            EventType::TabUpdate,
            EventType::ModeUpdate,
            EventType::Mouse,
            EventType::InitialKeybinds,
            EventType::PaneUpdate,
            EventType::PermissionRequestResult,
            EventType::RunCommandResult,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        let mut should_render = false;
        match event {
            Event::PermissionRequestResult(PermissionStatus::Granted) => {
                self.permissions_granted = true;
                set_selectable(false);
            }
            Event::PaneUpdate(manifest) if self.permissions_granted => {
                let own_id = get_plugin_ids().plugin_id;
                self.own_tab_position = manifest.panes.iter().find_map(|(position, panes)| {
                    panes
                        .iter()
                        .any(|p| p.is_plugin && p.id == own_id)
                        .then_some(*position)
                });
                if let Some(target) = self.replace_pane {
                    let present = |id| {
                        manifest
                            .panes
                            .values()
                            .flatten()
                            .any(|p| p.is_plugin && p.id == id)
                    };
                    if present(own_id) && present(target) {
                        self.replace_pane = None;
                        replace_pane_with_existing_pane(
                            PaneId::Plugin(target),
                            PaneId::Plugin(own_id),
                            false,
                        );
                        set_pane_borderless(PaneId::Plugin(own_id), true);
                        set_selectable(false);
                    }
                }
                self.panes = manifest;
            }
            Event::RunCommandResult(code, _, stderr, _) => {
                if code != Some(0) {
                    eprintln!(
                        "Could not clear completed badges: {}",
                        String::from_utf8_lossy(&stderr)
                    );
                }
            }
            Event::InitialKeybinds(keybinds) => {
                self.cached_keybinds = keybinds;
                if !self.cached_keybinds.is_empty() {
                    self.mode_info.keybinds = self.cached_keybinds.clone();
                }
                should_render = true;
            }
            Event::ModeUpdate(mut mode_info) => {
                if mode_info.keybinds.is_empty() && !self.cached_keybinds.is_empty() {
                    mode_info.keybinds = self.cached_keybinds.clone();
                } else if !mode_info.keybinds.is_empty() {
                    self.cached_keybinds = mode_info.keybinds.clone();
                }
                if self.mode_info != mode_info {
                    should_render = true;
                }
                self.mode_info = mode_info;
            }
            Event::TabUpdate(tabs) => {
                if let Some(active_tab_index) = tabs.iter().position(|t| t.active) {
                    // tabs are indexed starting from 1 so we need to add 1
                    let active_tab_idx = active_tab_index + 1;

                    if self.active_tab_idx != active_tab_idx || self.tabs != tabs {
                        should_render = true;
                    }
                    self.active_tab_idx = active_tab_idx;
                    self.tabs = tabs;
                } else {
                    eprintln!("Could not find active tab.");
                }
            }
            Event::Mouse(me) => match me {
                Mouse::LeftClick(_, col) => {
                    let tab_to_focus = get_tab_to_focus(&self.tab_line, self.active_tab_idx, col);
                    if let Some(idx) = tab_to_focus {
                        switch_tab_to(idx.try_into().unwrap());
                    }
                }
                Mouse::ScrollUp(_) => {
                    switch_tab_to(min(self.active_tab_idx + 1, self.tabs.len()) as u32);
                }
                Mouse::ScrollDown(_) => {
                    switch_tab_to(max(self.active_tab_idx.saturating_sub(1), 1) as u32);
                }
                _ => {}
            },
            _ => {
                eprintln!("Got unrecognized event: {:?}", event);
            }
        }
        if self.tabs.is_empty() {
            // no need to render if we have no tabs, this can sometimes happen on startup before we
            // get the tab update and then we definitely don't want to render
            should_render = false;
        }
        should_render
    }

    fn pipe(&mut self, message: PipeMessage) -> bool {
        if message.name != "zellij_codex_clear_done" || !self.permissions_granted {
            return false;
        }
        // Broadcast keybindings reach every tab bar; only the current tab handles this one.
        let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| tab.active && Some(tab.position) == self.own_tab_position)
        else {
            return false;
        };
        let Some(session) = self.mode_info.session_name.as_deref() else {
            return false;
        };
        let Some(pane) = self.panes.panes.get(&tab.position).and_then(|panes| {
            panes.iter().find(|pane| {
                pane.is_focused
                    && !pane.is_plugin
                    && !pane.is_suppressed
                    && pane.is_floating == tab.are_floating_panes_visible
            })
        }) else {
            return false;
        };
        let pane_id = pane.id.to_string();
        let mut command = match self.badge_command.as_deref() {
            Some(command) => vec![command],
            None => vec![
                "/bin/sh",
                "-c",
                "exec \"$HOME/.local/bin/zellij-codex-badges\" \"$@\"",
                "zellij-codex-clear-done",
            ],
        };
        command.extend(["--session", session, "--clear-done", "--pane-id", &pane_id]);
        run_command(&command, BTreeMap::new());
        false
    }

    fn render(&mut self, _rows: usize, cols: usize) {
        if self.tabs.is_empty() {
            return;
        }
        let mut all_tabs: Vec<LinePart> = vec![];
        let mut active_tab_index = 0;
        let mut is_alternate_tab = false;
        for t in &mut self.tabs {
            let mut tabname = t.name.clone();
            if t.active && self.mode_info.mode == InputMode::RenameTab {
                if tabname.is_empty() {
                    tabname = String::from("Enter name...");
                }
                active_tab_index = t.position;
            } else if t.active {
                active_tab_index = t.position;
            }
            let tab = tab_style(
                tabname,
                t,
                is_alternate_tab,
                self.mode_info.style.colors,
                self.mode_info.capabilities,
            );
            is_alternate_tab = !is_alternate_tab;
            all_tabs.push(tab);
        }

        let background = self.mode_info.style.colors.text_unselected.background;

        self.tab_line = tab_line(
            self.mode_info.session_name.as_deref(),
            all_tabs,
            active_tab_index,
            cols.saturating_sub(1),
            self.mode_info.style.colors,
            self.mode_info.capabilities,
            self.mode_info.style.hide_session_name,
            self.tabs.iter().find(|t| t.active),
            &self.mode_info,
            self.hide_swap_layout_indication,
            &background,
        );

        let output = self
            .tab_line
            .iter()
            .fold(String::new(), |output, part| output + &part.part);

        match background {
            PaletteColor::Rgb((r, g, b)) => {
                print!("{}\u{1b}[48;2;{};{};{}m\u{1b}[0K", output, r, g, b);
            }
            PaletteColor::EightBit(color) => {
                print!("{}\u{1b}[48;5;{}m\u{1b}[0K", output, color);
            }
        }
    }
}

fn style(foreground: PaletteColor, background: PaletteColor) -> ansi_term::Style {
    let color = |value| match value {
        PaletteColor::Rgb((r, g, b)) => ansi_term::Color::RGB(r, g, b),
        PaletteColor::EightBit(index) => ansi_term::Color::Fixed(index),
    };
    ansi_term::Style::new()
        .fg(color(foreground))
        .on(color(background))
}

#[cfg(all(test, not(target_family = "wasm")))]
#[no_mangle]
extern "C" fn host_run_plugin_command() {
    panic!("unit tests must not call the Zellij host");
}
