//! Zellij plugin entry point for the Codex status dashboard.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use zellij_tile::prelude::*;

const PIPE_NAME: &str = "codex_status";
const SHOW_DASHBOARD_PIPE_NAME: &str = "show_dashboard";
const DASHBOARD_PANE_TITLE: &str = "Codex Dashboard";
const NEOVIM_PANE_TITLE: &str = "Neovim";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Status {
    Running,
    Idle,
    Input,
    Stuck,
    Error,
    Done,
    Paused,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Idle => "idle",
            Self::Input => "input",
            Self::Stuck => "stuck",
            Self::Error => "error",
            Self::Done => "done",
            Self::Paused => "paused",
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Self::Running => "●",
            Self::Idle => "○",
            Self::Input => "!",
            Self::Stuck | Self::Error => "✕",
            Self::Done => "✓",
            Self::Paused => "Ⅱ",
        }
    }

    fn ansi_color(self) -> u8 {
        match self {
            Self::Running => 32,
            Self::Idle => 90,
            Self::Input => 33,
            Self::Stuck => 31,
            Self::Error => 91,
            Self::Done => 34,
            Self::Paused => 35,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct AgentReport {
    id: String,
    agent: String,
    status: Status,
    #[serde(default)]
    task: String,
    #[serde(default)]
    worktree: String,
    #[serde(default)]
    pane_id: Option<u32>,
    #[serde(default)]
    tab_id: Option<usize>,
    #[serde(default)]
    tab_position: Option<usize>,
    #[serde(default)]
    remove: bool,
}

#[derive(Default)]
struct App {
    agents: BTreeMap<String, AgentReport>,
    focused_pane_id: Option<u32>,
    error: Option<String>,
    pane_manifest: PaneManifest,
    tabs: Vec<TabInfo>,
    suppressed_neovim: Option<SuppressedNeovim>,
    pending_neovim_suppression: Option<PendingNeovimSuppression>,
}

#[derive(Clone, Copy)]
struct SuppressedNeovim {
    pane_id: PaneId,
    tab_id: Option<usize>,
}

#[derive(Clone, Copy)]
struct PendingNeovimSuppression {
    tab_position: usize,
    tab_id: Option<usize>,
}

register_plugin!(App);

impl App {
    fn apply_report(&mut self, mut agent: AgentReport) {
        if let Some(pane_id) = agent.pane_id {
            let (known_tab_id, known_tab_position) = self.known_tab_location(pane_id);
            agent.tab_id = agent.tab_id.or(known_tab_id);
            agent.tab_position = agent.tab_position.or(known_tab_position);
        }
        if agent.remove {
            self.agents.remove(&agent.id);
            if let Some(pane_id) = agent.pane_id {
                self.agents
                    .retain(|_, existing| existing.pane_id != Some(pane_id));
            }
        } else {
            if agent.status == Status::Done && agent.pane_id == self.focused_pane_id {
                agent.status = Status::Idle;
            }
            if let Some(pane_id) = agent.pane_id {
                self.agents
                    .retain(|_, existing| existing.pane_id != Some(pane_id));
            }
            self.agents.insert(agent.id.clone(), agent);
        }
    }

    fn show_dashboard(&mut self) {
        let Ok((tab_position, focused_pane_id)) = get_focused_pane_info() else {
            self.error = Some("could not determine the focused Zellij pane".to_string());
            show_self(true);
            self.center_dashboard();
            return;
        };

        let codex_surface_is_visible = !self.floating_panes_are_visible(tab_position);
        self.observe_current_view(tab_position, focused_pane_id, codex_surface_is_visible);
        self.error = None;
        self.suppressed_neovim = None;
        self.pending_neovim_suppression = None;
        let tab_id = self.tab_id_at_position(tab_position);
        let should_suppress_neovim = codex_surface_is_visible;
        let neovim_to_suppress = should_suppress_neovim
            .then(|| self.neovim_pane_id(tab_position))
            .flatten();
        if should_suppress_neovim && neovim_to_suppress.is_none() {
            self.pending_neovim_suppression = Some(PendingNeovimSuppression {
                tab_position,
                tab_id,
            });
        }

        if self.move_to_tab(tab_position) {
            hide_self();
        }
        rename_plugin_pane(get_plugin_ids().plugin_id, DASHBOARD_PANE_TITLE);
        show_self(true);
        if let Some(neovim_pane_id) = neovim_to_suppress {
            self.suppress_neovim(neovim_pane_id, tab_id);
        }
        self.center_dashboard();
    }

    fn hide_dashboard(&mut self) {
        self.pending_neovim_suppression = None;
        hide_self();
        if let Some(neovim) = self.suppressed_neovim.take() {
            show_pane_with_id(neovim.pane_id, true, false);
            if let Err(error) = hide_floating_panes(neovim.tab_id) {
                self.error = Some(format!("could not restore the workbench view: {error}"));
            }
        }
    }

    fn center_dashboard(&self) {
        let plugin_id = PaneId::Plugin(get_plugin_ids().plugin_id);
        let coordinates = FloatingPaneCoordinates::default()
            .with_x_percent(25)
            .with_y_percent(25)
            .with_width_percent(50)
            .with_height_percent(50);
        change_floating_panes_coordinates(vec![(plugin_id, coordinates)]);
    }

    fn move_to_tab(&self, tab_position: usize) -> bool {
        let plugin_id = get_plugin_ids().plugin_id;
        if self.plugin_tab_position(plugin_id) != Some(tab_position) {
            break_panes_to_tab_with_index(&[PaneId::Plugin(plugin_id)], tab_position, false);
            return true;
        }
        false
    }

    fn tab_id_at_position(&self, tab_position: usize) -> Option<usize> {
        self.tabs
            .iter()
            .find(|tab| tab.position == tab_position)
            .map(|tab| tab.tab_id)
    }

    fn known_tab_location(&self, pane_id: u32) -> (Option<usize>, Option<usize>) {
        if let Some(agent) = self
            .agents
            .values()
            .find(|agent| agent.pane_id == Some(pane_id))
        {
            if agent.tab_id.is_some() || agent.tab_position.is_some() {
                return (agent.tab_id, agent.tab_position);
            }
        }

        self.pane_manifest
            .panes
            .iter()
            .find(|(_, panes)| {
                panes
                    .iter()
                    .any(|pane| !pane.is_plugin && pane.id == pane_id)
            })
            .map_or((None, None), |(tab_position, _)| {
                (self.tab_id_at_position(*tab_position), Some(*tab_position))
            })
    }

    fn floating_panes_are_visible(&self, tab_position: usize) -> bool {
        let cached_visibility = self
            .tabs
            .iter()
            .find(|tab| tab.position == tab_position)
            .is_some_and(|tab| tab.are_floating_panes_visible);
        self.tab_id_at_position(tab_position)
            .and_then(get_tab_info)
            .map_or(cached_visibility, |tab| tab.are_floating_panes_visible)
    }

    fn neovim_pane_id(&self, tab_position: usize) -> Option<PaneId> {
        let pane_id = self
            .pane_manifest
            .panes
            .get(&tab_position)?
            .iter()
            .find(|pane| {
                !pane.is_plugin
                    && pane.is_floating
                    && !pane.is_suppressed
                    && pane.title == NEOVIM_PANE_TITLE
            })
            .map(|pane| PaneId::Terminal(pane.id))?;
        get_pane_info(pane_id)
            .filter(|pane| {
                !pane.is_plugin
                    && pane.is_floating
                    && !pane.is_suppressed
                    && pane.title == NEOVIM_PANE_TITLE
            })
            .map(|_| pane_id)
    }

    fn plugin_tab_position(&self, plugin_id: u32) -> Option<usize> {
        self.pane_manifest
            .panes
            .iter()
            .find(|(_, panes)| {
                panes
                    .iter()
                    .any(|pane| pane.is_plugin && pane.id == plugin_id)
            })
            .map(|(tab_position, _)| *tab_position)
    }

    fn observe_focused_pane(&mut self, pane_id: PaneId) -> bool {
        self.focused_pane_id = match pane_id {
            PaneId::Terminal(pane_id) => Some(pane_id),
            PaneId::Plugin(_) => None,
        };

        let mut status_changed = false;
        if let Some(pane_id) = self.focused_pane_id {
            for agent in self.agents.values_mut() {
                if agent.pane_id == Some(pane_id) && agent.status == Status::Done {
                    agent.status = Status::Idle;
                    status_changed = true;
                }
            }
        }
        status_changed
    }

    fn observe_current_view(
        &mut self,
        tab_position: usize,
        pane_id: PaneId,
        codex_surface_is_visible: bool,
    ) -> bool {
        let mut status_changed = self.observe_focused_pane(pane_id);
        if codex_surface_is_visible {
            let visible_pane_ids = self
                .pane_manifest
                .panes
                .get(&tab_position)
                .into_iter()
                .flatten()
                .filter(|pane| !pane.is_plugin && !pane.exited && !pane.is_suppressed)
                .map(|pane| pane.id)
                .collect::<BTreeSet<_>>();
            status_changed |= self.observe_active_tab(
                self.tab_id_at_position(tab_position),
                tab_position,
                &visible_pane_ids,
            );
        }
        status_changed
    }

    fn observe_active_tab(
        &mut self,
        tab_id: Option<usize>,
        tab_position: usize,
        visible_pane_ids: &BTreeSet<u32>,
    ) -> bool {
        let mut status_changed = false;
        for agent in self.agents.values_mut() {
            let stable_tab_matches = agent
                .tab_id
                .zip(tab_id)
                .is_some_and(|(agent_tab_id, active_tab_id)| agent_tab_id == active_tab_id);
            let position_matches = (agent.tab_id.is_none() || tab_id.is_none())
                && agent.tab_position == Some(tab_position);
            let pane_is_visible = agent
                .pane_id
                .is_some_and(|pane_id| visible_pane_ids.contains(&pane_id));
            if (stable_tab_matches || position_matches || pane_is_visible)
                && agent.status == Status::Done
            {
                agent.status = Status::Idle;
                status_changed = true;
            }
        }
        status_changed
    }

    fn refresh_current_view(&mut self) -> bool {
        let Ok((tab_position, pane_id)) = get_focused_pane_info() else {
            return false;
        };
        let codex_surface_is_visible = !self.floating_panes_are_visible(tab_position);
        self.observe_current_view(tab_position, pane_id, codex_surface_is_visible)
    }

    fn discover_codex_panes(&mut self, pane_manifest: &PaneManifest) -> bool {
        let mut discovered = Vec::new();

        for (tab_position, panes) in &pane_manifest.panes {
            let tab_id = self.tab_id_at_position(*tab_position);
            for pane in panes {
                if pane.is_plugin || pane.exited {
                    continue;
                }

                let pane_id = PaneId::Terminal(pane.id);
                let running_command = get_pane_running_command(pane_id).unwrap_or_default();
                let launch_command = pane.terminal_command.iter().cloned().collect::<Vec<_>>();
                if !is_codex_command(&running_command) && !is_codex_command(&launch_command) {
                    continue;
                }

                let worktree = get_pane_cwd(pane_id)
                    .ok()
                    .and_then(|cwd| {
                        cwd.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                    })
                    .unwrap_or_default();
                discovered.push((pane.id, worktree, tab_id, *tab_position));
            }
        }

        self.reconcile_discovered_panes(discovered)
    }

    fn reconcile_discovered_panes(
        &mut self,
        discovered: Vec<(u32, String, Option<usize>, usize)>,
    ) -> bool {
        let agents_before = self.agents.clone();
        let discovered_pane_ids = discovered
            .iter()
            .map(|(pane_id, _, _, _)| *pane_id)
            .collect::<BTreeSet<_>>();
        self.agents.retain(|id, agent| {
            !id.starts_with("discovered:pane:")
                || agent
                    .pane_id
                    .is_some_and(|pane_id| discovered_pane_ids.contains(&pane_id))
        });

        for (pane_id, worktree, tab_id, tab_position) in discovered {
            if self
                .agents
                .values()
                .any(|agent| agent.pane_id == Some(pane_id))
            {
                continue;
            }

            let id = format!("discovered:pane:{pane_id}");
            self.agents.insert(
                id.clone(),
                AgentReport {
                    id,
                    agent: format!("codex-{pane_id}"),
                    status: Status::Idle,
                    task: "Discovered running Codex".to_string(),
                    worktree,
                    pane_id: Some(pane_id),
                    tab_id,
                    tab_position: Some(tab_position),
                    remove: false,
                },
            );
        }

        self.agents != agents_before
    }

    fn update_pane_manifest(&mut self, pane_manifest: PaneManifest) -> bool {
        let mut should_render = self.discover_codex_panes(&pane_manifest);
        self.pane_manifest = pane_manifest;
        should_render |= self.refresh_current_view();
        if let Some(pending) = self.pending_neovim_suppression {
            if let Some(neovim_pane_id) = self.neovim_pane_id(pending.tab_position) {
                self.pending_neovim_suppression = None;
                self.suppress_neovim(neovim_pane_id, pending.tab_id);
            }
        }

        should_render
    }

    fn suppress_neovim(&mut self, pane_id: PaneId, tab_id: Option<usize>) {
        hide_pane_with_id(pane_id);
        self.suppressed_neovim = Some(SuppressedNeovim { pane_id, tab_id });
    }
}

impl ZellijPlugin for App {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::ReadCliPipes,
        ]);
        subscribe(&[EventType::Key, EventType::PaneUpdate, EventType::TabUpdate]);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key) if key.bare_key == BareKey::Esc && key.key_modifiers.is_empty() => {
                self.hide_dashboard();
                false
            }
            Event::PaneUpdate(pane_manifest) => self.update_pane_manifest(pane_manifest),
            Event::TabUpdate(tabs) => {
                self.tabs = tabs;
                self.refresh_current_view()
            }
            _ => false,
        }
    }

    fn pipe(&mut self, message: PipeMessage) -> bool {
        if message.name == SHOW_DASHBOARD_PIPE_NAME {
            self.show_dashboard();
            #[cfg(target_family = "wasm")]
            unblock_cli_pipe_input(&message.name);
            return true;
        }
        if message.name != PIPE_NAME {
            return false;
        }

        let Some(payload) = message.payload else {
            return false;
        };

        match serde_json::from_str(&payload) {
            Ok(agent) => {
                let agent: AgentReport = agent;
                self.apply_report(agent);
                self.refresh_current_view();
                self.error = None;
            }
            Err(error) => self.error = Some(format!("invalid status report: {error}")),
        }
        // CLI pipes are flow-controlled. Without this, Zellij never delivers
        // the end-of-stream message and `zellij pipe` times out after one
        // second, making lifecycle reports unreliable.
        #[cfg(target_family = "wasm")]
        unblock_cli_pipe_input(&message.name);
        true
    }

    fn render(&mut self, _rows: usize, _cols: usize) {
        if let Some(error) = &self.error {
            println!("\u{1b}[91m✕ {error}\u{1b}[0m");
            return;
        }

        if self.agents.is_empty() {
            println!("Waiting for a Codex status report…");
            return;
        }

        println!(
            "{:<20}  {:<18}  {:<12}  TASK",
            "WORKTREE", "AGENT", "STATUS"
        );
        for agent in self.agents.values() {
            let pane = agent.pane_id.map(|id| format!("p{id}")).unwrap_or_default();
            let agent_label = if pane.is_empty() {
                agent.agent.clone()
            } else {
                format!("{} ({pane})", agent.agent)
            };
            println!(
                "{:<20}  {:<18}  \u{1b}[{}m{} {:<9}\u{1b}[0m  {}",
                truncate(&agent.worktree, 20),
                truncate(&agent_label, 18),
                agent.status.ansi_color(),
                agent.status.symbol(),
                agent.status.label(),
                truncate(&agent.task, 80),
            );
        }
        println!();
        println!("Esc: hide dashboard");
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let single_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = single_line.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() && max_chars > 1 {
        format!(
            "{}…",
            prefix.chars().take(max_chars - 1).collect::<String>()
        )
    } else {
        prefix
    }
}

fn is_codex_command(command: &[String]) -> bool {
    command.iter().any(|argument| {
        argument
            .split(|character: char| {
                character.is_whitespace() || matches!(character, '\'' | '"' | ';' | '|' | '&')
            })
            .filter(|token| !token.is_empty())
            .any(|token| {
                matches!(
                    Path::new(token).file_name().and_then(|name| name.to_str()),
                    Some("codex" | "codex.js" | "zellij-codex-launch")
                )
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_status_report() {
        let report: AgentReport = serde_json::from_str(
            r#"{"id":"thread-1","agent":"implementation","status":"running","task":"Migrating grpc","worktree":"grpc-migration","pane_id":7,"tab_id":3,"tab_position":2}"#,
        )
        .unwrap();

        assert_eq!(report.agent, "implementation");
        assert_eq!(report.id, "thread-1");
        assert_eq!(report.status, Status::Running);
        assert_eq!(report.task, "Migrating grpc");
        assert_eq!(report.worktree, "grpc-migration");
        assert_eq!(report.tab_id, Some(3));
        assert_eq!(report.tab_position, Some(2));
    }

    #[test]
    fn rejects_an_unknown_status() {
        let result = serde_json::from_str::<AgentReport>(
            r#"{"id":"thread-1","agent":"implementation","status":"thinking"}"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn every_status_has_the_requested_color() {
        assert_eq!(Status::Running.ansi_color(), 32);
        assert_eq!(Status::Idle.ansi_color(), 90);
        assert_eq!(Status::Input.ansi_color(), 33);
        assert_eq!(Status::Stuck.ansi_color(), 31);
        assert_eq!(Status::Error.ansi_color(), 91);
        assert_eq!(Status::Done.ansi_color(), 34);
        assert_eq!(Status::Paused.ansi_color(), 35);
    }

    #[test]
    fn reports_with_different_ids_coexist() {
        let mut app = App::default();
        for id in ["thread-1", "thread-2"] {
            let report = AgentReport {
                id: id.to_string(),
                agent: "codex".to_string(),
                status: Status::Idle,
                task: String::new(),
                worktree: "demo".to_string(),
                pane_id: None,
                tab_id: None,
                tab_position: None,
                remove: false,
            };
            app.agents.insert(report.id.clone(), report);
        }

        assert_eq!(app.agents.len(), 2);
    }

    #[test]
    fn a_removal_report_deletes_the_agent() {
        let mut app = App::default();
        let id = "quadratic-tiger:pane:12".to_string();
        app.agents.insert(
            id.clone(),
            AgentReport {
                id: id.clone(),
                agent: "codex-12".to_string(),
                status: Status::Done,
                task: String::new(),
                worktree: "demo".to_string(),
                pane_id: Some(12),
                tab_id: Some(3),
                tab_position: Some(2),
                remove: false,
            },
        );

        app.apply_report(AgentReport {
            id,
            agent: "codex-12".to_string(),
            status: Status::Done,
            task: String::new(),
            worktree: "demo".to_string(),
            pane_id: Some(12),
            tab_id: Some(3),
            tab_position: Some(2),
            remove: true,
        });

        assert!(app.agents.is_empty());
    }

    #[test]
    fn discovers_workbench_codex_commands() {
        assert!(is_codex_command(&[
            "node".to_string(),
            "/usr/bin/codex".to_string(),
            "--yolo".to_string(),
        ]));
        assert!(is_codex_command(&[
            "bash".to_string(),
            "if command -v zellij-codex-launch; then exec zellij-codex-launch --yolo; fi"
                .to_string(),
        ]));
        assert!(!is_codex_command(&[
            "bash".to_string(),
            "-lc".to_string(),
            "nvim .".to_string(),
        ]));
    }

    #[test]
    fn hook_reports_replace_discovery_and_closed_panes_are_removed() {
        let mut app = App::default();
        app.reconcile_discovered_panes(vec![(12, "demo".to_string(), Some(3), 2)]);
        app.apply_report(AgentReport {
            id: "quadratic-tiger:pane:12".to_string(),
            agent: "codex-12".to_string(),
            status: Status::Running,
            task: "Implement pane discovery".to_string(),
            worktree: "demo".to_string(),
            pane_id: Some(12),
            tab_id: Some(3),
            tab_position: Some(2),
            remove: false,
        });

        assert_eq!(app.agents.len(), 1);
        assert!(app.agents.contains_key("quadratic-tiger:pane:12"));

        app.agents.clear();
        app.reconcile_discovered_panes(vec![(12, "demo".to_string(), Some(3), 2)]);
        app.reconcile_discovered_panes(Vec::new());
        assert!(app.agents.is_empty());
    }

    #[test]
    fn done_is_unread_until_the_codex_pane_is_observed() {
        let mut app = App::default();
        let report = AgentReport {
            id: "quadratic-tiger:pane:12".to_string(),
            agent: "codex-12".to_string(),
            status: Status::Done,
            task: "Implemented the requested change".to_string(),
            worktree: "demo".to_string(),
            pane_id: Some(12),
            tab_id: Some(3),
            tab_position: Some(2),
            remove: false,
        };
        let other_report = AgentReport {
            id: "quadratic-tiger:pane:13".to_string(),
            agent: "codex-13".to_string(),
            status: Status::Done,
            task: "Another completed result".to_string(),
            worktree: "other".to_string(),
            pane_id: Some(13),
            tab_id: Some(4),
            tab_position: Some(3),
            remove: false,
        };

        app.apply_report(report.clone());
        app.apply_report(other_report.clone());
        assert_eq!(app.agents[&report.id].status, Status::Done);

        app.observe_focused_pane(PaneId::Terminal(7));
        assert_eq!(app.agents[&report.id].status, Status::Done);

        app.observe_focused_pane(PaneId::Terminal(12));
        assert_eq!(app.agents[&report.id].status, Status::Idle);
        assert_eq!(app.agents[&other_report.id].status, Status::Done);

        app.focused_pane_id = None;
        app.apply_report(report.clone());
        app.observe_active_tab(Some(3), 2, &BTreeSet::new());
        assert_eq!(app.agents[&report.id].status, Status::Idle);
        assert_eq!(app.agents[&other_report.id].status, Status::Done);

        app.focused_pane_id = None;
        app.apply_report(report.clone());
        app.observe_active_tab(None, 2, &BTreeSet::from([12]));
        assert_eq!(app.agents[&report.id].status, Status::Idle);
        assert_eq!(app.agents[&other_report.id].status, Status::Done);
    }

    #[test]
    fn truncates_unicode_without_splitting_characters() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("ação", 4), "ação");
    }

    #[test]
    fn collapses_line_breaks_and_other_whitespace_before_rendering() {
        assert_eq!(
            truncate("Investigate the\nagent\r\n  description\tbug", 80),
            "Investigate the agent description bug"
        );
    }
}
