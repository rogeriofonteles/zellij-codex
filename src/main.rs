//! Zellij plugin entry point for the Codex status dashboard.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use zellij_tile::prelude::*;

const PIPE_NAME: &str = "codex_status";
const SHOW_DASHBOARD_PIPE_NAME: &str = "show_dashboard";
const DASHBOARD_PANE_TITLE: &str = "Codex Dashboard";
const NEOVIM_PANE_TITLE: &str = "Neovim";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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
    dashboard_tab_id: Option<usize>,
    focused_pane_id: Option<u32>,
    recovered_running_panes: BTreeSet<u32>,
    error: Option<String>,
    pane_manifest: PaneManifest,
    tabs: Vec<TabInfo>,
    visible_neovim: Option<SuppressedNeovim>,
    suppressed_neovim: Option<SuppressedNeovim>,
    pending_neovim_suppression: Option<PendingNeovimSuppression>,
}

#[derive(Clone, Copy)]
struct SuppressedNeovim {
    pane_id: PaneId,
    tab_id: Option<usize>,
    rectangle: PaneRectangle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PaneRectangle {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

impl PaneRectangle {
    fn coordinates(self) -> FloatingPaneCoordinates {
        FloatingPaneCoordinates::default()
            .with_x_fixed(self.x)
            .with_y_fixed(self.y)
            .with_width_fixed(self.width)
            .with_height_fixed(self.height)
    }
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
        let Ok((tab_id, focused_pane_id)) = get_focused_pane_info() else {
            self.error = Some("could not determine the focused Zellij pane".to_string());
            return;
        };
        let Some(tab) = get_tab_info(tab_id) else {
            return;
        };
        let tab_position = tab.position;
        self.tabs.retain(|known| known.tab_id != tab_id);
        self.tabs.push(tab);
        // Cross-tab moves can discard panes when tab IDs have gaps. Each tab
        // keeps its own dashboard pane and receives the current agent reports.
        if self.dashboard_tab_id != Some(tab_id)
            && self.plugin_tab_position(get_plugin_ids().plugin_id) != Some(tab_position)
        {
            let mut message = MessageToPlugin::new(SHOW_DASHBOARD_PIPE_NAME)
                .with_plugin_url("zellij:OWN_URL")
                .with_plugin_config(BTreeMap::from([
                    ("dashboard_tab_id".to_string(), tab_id.to_string()),
                    ("caller_cwd".to_string(), ".".to_string()),
                ]))
                .with_payload(serde_json::to_string(&self.agents).unwrap())
                .with_args(BTreeMap::from([(
                    "target_client_id".to_string(),
                    get_plugin_ids().client_id.to_string(),
                )]))
                .new_plugin_instance_should_float(true);
            if let Some(args) = message.new_plugin_args.as_mut() {
                args.should_focus = Some(false);
            }
            pipe_message_to_plugin(message);
            return;
        }
        if focused_pane_id == PaneId::Plugin(get_plugin_ids().plugin_id) {
            self.center_dashboard();
            return;
        }

        let focused_pane_info = get_pane_info(focused_pane_id);
        let codex_surface_is_visible = focused_pane_info
            .as_ref()
            .is_some_and(|pane| !pane.is_floating && !pane.is_suppressed);
        self.observe_current_view(tab_position, focused_pane_id, codex_surface_is_visible);
        self.error = None;
        self.visible_neovim = focused_pane_info
            .filter(|pane| {
                pane.is_floating && !pane.is_suppressed && pane.title == NEOVIM_PANE_TITLE
            })
            .map(|pane| SuppressedNeovim {
                pane_id: focused_pane_id,
                tab_id: self.tab_id_at_position(tab_position),
                rectangle: pane_rectangle(&pane),
            });
        self.suppressed_neovim = None;
        self.pending_neovim_suppression = None;
        let tab_id = self.tab_id_at_position(tab_position);
        let should_suppress_neovim = codex_surface_is_visible;
        let neovim_to_suppress = should_suppress_neovim
            .then(|| self.neovim_pane_id(tab_position))
            .flatten()
            .and_then(|pane_id| self.neovim_state(pane_id, tab_id));
        if should_suppress_neovim && neovim_to_suppress.is_none() {
            self.pending_neovim_suppression = Some(PendingNeovimSuppression {
                tab_position,
                tab_id,
            });
        }

        rename_plugin_pane(get_plugin_ids().plugin_id, DASHBOARD_PANE_TITLE);
        if let Some(neovim) = neovim_to_suppress {
            self.suppress_neovim(neovim);
        }
        show_self(true);
        self.center_dashboard();
        focus_pane_with_id(PaneId::Plugin(get_plugin_ids().plugin_id), true, true);
    }

    fn hide_dashboard(&mut self) {
        self.pending_neovim_suppression = None;
        hide_self();
        if let Some(neovim) = self.visible_neovim.take() {
            change_floating_panes_coordinates(vec![(
                neovim.pane_id,
                neovim.rectangle.coordinates(),
            )]);
        } else if let Some(neovim) = self.suppressed_neovim.take() {
            show_pane_with_id(neovim.pane_id, true, false);
            change_floating_panes_coordinates(vec![(
                neovim.pane_id,
                neovim.rectangle.coordinates(),
            )]);
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

    fn neovim_pane_id(&self, tab_position: usize) -> Option<PaneId> {
        // PaneUpdate can lag behind show_pane_with_id()/hide_pane_with_id(). Use
        // the manifest only to locate Neovim's stable pane ID, then decide from
        // the synchronous live pane state. Otherwise a manifest captured while
        // Neovim was suppressed prevents the next dashboard opening from hiding
        // it, and show_self() reveals both floating panes.
        let pane_id = neovim_pane_candidate(self.pane_manifest.panes.get(&tab_position)?)?;
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
        let Some(tab) = self.tabs.iter().find(|tab| tab.active) else {
            return false;
        };
        let position = tab.position;
        let floating = tab.are_floating_panes_visible;
        let focused = self
            .pane_manifest
            .panes
            .get(&position)
            .into_iter()
            .flatten()
            .find(|pane| pane.is_focused && !pane.is_suppressed && pane.is_floating == floating)
            .map(|pane| {
                if pane.is_plugin {
                    PaneId::Plugin(pane.id)
                } else {
                    PaneId::Terminal(pane.id)
                }
            });
        focused.is_some_and(|pane| self.observe_current_view(position, pane, !floating))
    }

    fn discover_codex_panes(&mut self, pane_manifest: &PaneManifest) -> bool {
        let discovered = pane_manifest
            .panes
            .iter()
            .flat_map(|(position, panes)| {
                let tab = self.tabs.iter().find(|tab| tab.position == *position);
                panes
                    .iter()
                    .filter(|pane| !pane.is_plugin && !pane.exited)
                    .filter(|pane| {
                        self.agents
                            .values()
                            .any(|agent| agent.pane_id == Some(pane.id))
                            || is_codex_command(
                                &pane.terminal_command.iter().cloned().collect::<Vec<_>>(),
                            )
                    })
                    .map(move |pane| {
                        (
                            pane.id,
                            tab.map(|tab| tab.name.clone()).unwrap_or_default(),
                            tab.map(|tab| tab.tab_id),
                            *position,
                        )
                    })
            })
            .collect();
        self.reconcile_discovered_panes(discovered)
    }

    fn observe_pane_contents(&mut self, pane_id: u32, viewport: &[String]) -> bool {
        if codex_activity(viewport).is_none() {
            return false;
        }
        if !self
            .agents
            .values()
            .any(|agent| agent.pane_id == Some(pane_id))
        {
            let (tab_id, position) = self.known_tab_location(pane_id);
            let worktree = self
                .tabs
                .iter()
                .find(|tab| Some(tab.position) == position)
                .map(|tab| tab.name.clone())
                .unwrap_or_default();
            self.apply_report(AgentReport {
                id: format!("discovered:pane:{pane_id}"),
                agent: format!("codex-{pane_id}"),
                status: Status::Idle,
                task: "Discovered running Codex".to_string(),
                worktree,
                pane_id: Some(pane_id),
                tab_id,
                tab_position: position,
                remove: false,
            });
            self.observe_activity(pane_id, viewport);
            return true;
        }
        self.observe_activity(pane_id, viewport)
    }

    fn observe_activity(&mut self, pane_id: u32, viewport: &[String]) -> bool {
        let Some(running) = codex_activity(viewport) else {
            return false;
        };
        if running {
            self.recovered_running_panes.insert(pane_id);
        } else if !self.recovered_running_panes.remove(&pane_id) {
            return false;
        }
        let mut changed = false;
        for agent in self
            .agents
            .values_mut()
            .filter(|agent| agent.pane_id == Some(pane_id))
        {
            let status = if running {
                Status::Running
            } else {
                Status::Done
            };
            changed |= agent.status != status;
            agent.status = status;
        }
        changed
    }

    fn reconcile_live_panes(&mut self, live_pane_ids: &BTreeSet<u32>) -> bool {
        let agents_before = self.agents.clone();
        self.agents.retain(|_, agent| {
            agent
                .pane_id
                .map_or(true, |pane_id| live_pane_ids.contains(&pane_id))
        });
        self.agents != agents_before
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
        let live_pane_ids = pane_manifest
            .panes
            .values()
            .flatten()
            .filter(|pane| !pane.is_plugin && !pane.exited)
            .map(|pane| pane.id)
            .collect::<BTreeSet<_>>();
        let mut should_render = self.reconcile_live_panes(&live_pane_ids);
        should_render |= self.discover_codex_panes(&pane_manifest);
        self.pane_manifest = pane_manifest;
        should_render |= self.refresh_current_view();
        if let Some(pending) = self.pending_neovim_suppression {
            if let Some(neovim_pane_id) = self.neovim_pane_id(pending.tab_position) {
                self.pending_neovim_suppression = None;
                if let Some(neovim) = self.neovim_state(neovim_pane_id, pending.tab_id) {
                    self.suppress_neovim(neovim);
                }
            }
        }
        should_render
    }

    fn neovim_state(&self, pane_id: PaneId, tab_id: Option<usize>) -> Option<SuppressedNeovim> {
        let Some(pane_info) = get_pane_info(pane_id) else {
            return None;
        };
        Some(SuppressedNeovim {
            pane_id,
            tab_id,
            rectangle: pane_rectangle(&pane_info),
        })
    }

    fn suppress_neovim(&mut self, neovim: SuppressedNeovim) {
        hide_pane_with_id(neovim.pane_id);
        self.suppressed_neovim = Some(neovim);
    }
}

impl ZellijPlugin for App {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.dashboard_tab_id = configuration
            .get("dashboard_tab_id")
            .and_then(|value| value.parse().ok());
        #[cfg(target_family = "wasm")]
        match std::fs::read(report_cache_path()) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(agents) => self.agents = agents,
                Err(error) => eprintln!("could not decode cached agent reports: {error}"),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!("could not read cached agent reports: {error}"),
        }
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::ReadCliPipes,
            PermissionType::ReadPaneContents,
            PermissionType::MessageAndLaunchOtherPlugins,
        ]);
        subscribe(&[
            EventType::Key,
            EventType::PaneUpdate,
            EventType::TabUpdate,
            EventType::PaneRenderReport,
            EventType::Timer,
        ]);
        if self.dashboard_tab_id.is_none() {
            set_timeout(0.1);
        }
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::Timer(_) => {
                if let Ok(snapshot) = get_session_list() {
                    if let Some(session) = snapshot
                        .live_sessions
                        .into_iter()
                        .find(|session| session.is_current_session)
                    {
                        self.tabs = session.tabs;
                        return self.update_pane_manifest(session.panes);
                    }
                }
                false
            }
            Event::Key(key) if key.bare_key == BareKey::Esc && key.key_modifiers.is_empty() => {
                self.hide_dashboard();
                false
            }
            Event::PaneRenderReport(panes) => {
                let mut changed = false;
                for (pane_id, contents) in panes {
                    if let PaneId::Terminal(pane_id) = pane_id {
                        changed |= self.observe_pane_contents(pane_id, &contents.viewport);
                    }
                }
                changed | self.refresh_current_view()
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
        #[cfg(target_family = "wasm")]
        if let PipeSource::Cli(pipe_id) = &message.source {
            unblock_cli_pipe_input(pipe_id);
        }
        #[cfg(target_family = "wasm")]
        if message.name == "list_agents" {
            if let PipeSource::Cli(pipe_id) = &message.source {
                self.refresh_current_view();
                if let Ok(payload) = serde_json::to_string(&self.agents) {
                    cli_pipe_output(pipe_id, &format!("{payload}\n"));
                }
            }
            return true;
        }
        if message.name == SHOW_DASHBOARD_PIPE_NAME {
            if matches!(message.source, PipeSource::Plugin(_)) {
                if message.args.get("target_client_id")
                    != Some(&get_plugin_ids().client_id.to_string())
                {
                    return false;
                }
                if let Some(payload) = &message.payload {
                    if let Ok(agents) = serde_json::from_str(payload) {
                        self.agents = agents;
                    }
                }
            }
            self.show_dashboard();
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
                #[cfg(target_family = "wasm")]
                if let Err(error) = serde_json::to_vec(&self.agents)
                    .map_err(std::io::Error::other)
                    .and_then(|bytes| {
                        let path = report_cache_path();
                        let temporary = format!("{path}.{}", get_plugin_ids().client_id);
                        std::fs::write(&temporary, bytes)?;
                        std::fs::rename(temporary, path)
                    })
                {
                    eprintln!("could not cache agent reports: {error}");
                }
            }
            Err(error) => self.error = Some(format!("invalid status report: {error}")),
        }
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

fn codex_activity(viewport: &[String]) -> Option<bool> {
    let footer = viewport
        .iter()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(6)
        .map(|line| line.trim())
        .collect::<Vec<_>>();
    if footer
        .iter()
        .any(|line| line.starts_with('•') && line.contains("esc to interrupt"))
    {
        return Some(true);
    }
    // Only a recognizable Codex input footer can end recovered activity.
    if footer
        .first()
        .is_some_and(|line| line.starts_with("gpt-") && line.contains(" · "))
        && footer.iter().any(|line| line.starts_with('›'))
    {
        return Some(false);
    }
    None
}

#[cfg(target_family = "wasm")]
fn report_cache_path() -> String {
    // /data is client-specific. /cache survives client replacement; the server
    // PID separates live sessions and avoids restoring reports after a restart.
    format!("/cache/agent_reports_{}.json", get_plugin_ids().zellij_pid)
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

fn neovim_pane_candidate(panes: &[PaneInfo]) -> Option<PaneId> {
    panes
        .iter()
        .find(|pane| !pane.is_plugin && !pane.exited && pane.title == NEOVIM_PANE_TITLE)
        .map(|pane| PaneId::Terminal(pane.id))
}

fn pane_rectangle(pane: &PaneInfo) -> PaneRectangle {
    PaneRectangle {
        x: pane.pane_x,
        y: pane.pane_y,
        width: pane.pane_columns,
        height: pane.pane_rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_family = "wasm"))]
    #[no_mangle]
    extern "C" fn host_run_plugin_command() {
        panic!("unit tests must not call the Zellij host");
    }

    #[test]
    fn pane_updates_discover_agents_without_host_queries() {
        let mut app = App::default();
        app.tabs = vec![TabInfo {
            tab_id: 4,
            position: 2,
            name: "worktree".into(),
            active: true,
            ..Default::default()
        }];
        let manifest = PaneManifest {
            panes: std::collections::HashMap::from([(
                2,
                (0..64)
                    .map(|id| PaneInfo {
                        id,
                        terminal_command: Some("zellij-codex-launch".into()),
                        is_focused: id == 0,
                        ..Default::default()
                    })
                    .collect(),
            )]),
        };
        assert!(app.update_pane_manifest(manifest.clone()));
        assert!(!app.update_pane_manifest(manifest));
        assert_eq!(app.agents.len(), 64);
    }

    #[test]
    fn recovers_activity_without_a_lifecycle_report() {
        let mut app = App::default();
        app.reconcile_discovered_panes(vec![(17, "coverage".into(), Some(4), 3)]);
        let running = vec![
            "• Waiting for background terminal (6m • esc to interrupt)".into(),
            "  └ git status".into(),
            "› Ask Codex to do anything".into(),
            "gpt-6-astra low · ~/code/coverage".into(),
        ];
        assert!(app.observe_activity(17, &running));
        assert_eq!(app.agents.values().next().unwrap().status, Status::Running);
        assert!(!app.observe_activity(17, &["ordinary output".into()]));
        assert!(app.observe_activity(17, &running[2..]));
        assert_eq!(app.agents.values().next().unwrap().status, Status::Done);
        assert_eq!(codex_activity(&["quoted esc to interrupt".into()]), None);
    }

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
    fn stale_suppressed_neovim_is_still_a_live_lookup_candidate() {
        let panes = vec![PaneInfo {
            id: 17,
            title: NEOVIM_PANE_TITLE.to_string(),
            is_floating: false,
            is_suppressed: true,
            ..PaneInfo::default()
        }];

        assert_eq!(neovim_pane_candidate(&panes), Some(PaneId::Terminal(17)));
    }

    #[test]
    fn pane_rectangle_preserves_the_exact_geometry() {
        let pane = PaneInfo {
            pane_x: 0,
            pane_y: 1,
            pane_columns: 365,
            pane_rows: 91,
            ..PaneInfo::default()
        };

        assert_eq!(
            pane_rectangle(&pane),
            PaneRectangle {
                x: 0,
                y: 1,
                width: 365,
                height: 91,
            }
        );
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

        app.apply_report(AgentReport {
            id: "remote-agent".to_string(),
            agent: "remote-codex".to_string(),
            status: Status::Done,
            task: "Remote result".to_string(),
            worktree: "remote".to_string(),
            pane_id: None,
            tab_id: None,
            tab_position: None,
            remove: false,
        });
        assert!(!app.reconcile_live_panes(&BTreeSet::from([12])));

        assert!(app.reconcile_live_panes(&BTreeSet::new()));
        assert_eq!(app.agents.len(), 1);
        assert!(app.agents.contains_key("remote-agent"));
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
