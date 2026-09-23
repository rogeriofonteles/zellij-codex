//! Zellij plugin entry point for the Codex status dashboard.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use zellij_tile::prelude::*;

const PIPE_NAME: &str = "codex_status";
const SHOW_DASHBOARD_PIPE_NAME: &str = "show_dashboard";
const SYNC_AGENTS_PIPE_NAME: &str = "sync_agents";
const AGENT_SNAPSHOT_PIPE_NAME: &str = "agent_snapshot";
const DASHBOARD_PANE_TITLE: &str = "Codex Dashboard";

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
    visible_floating_pane: Option<FloatingPaneState>,
    suppressed_floating_panes: Vec<FloatingPaneState>,
    pending_floating_pane_suppression: Option<PendingFloatingPaneSuppression>,
    checkpoint_pending: bool,
    bootstrap_pending: bool,
    dashboard_named: bool,
    dashboard_visible: bool,
    plugin_id: Option<u32>,
    status_source: Option<u32>,
    pending_reports: Vec<PipeMessage>,
    seen_reports: VecDeque<(String, String)>,
    plugin_url: Option<String>,
    status_observers: BTreeSet<u32>,
    last_published: Option<AgentSnapshot>,
    published_peers: BTreeSet<u32>,
    is_monitor: bool,
    monitor_start_pending: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct AgentSnapshot {
    agents: BTreeMap<String, AgentReport>,
    error: Option<String>,
}

#[derive(Clone, Copy)]
struct FloatingPaneState {
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
struct PendingFloatingPaneSuppression {
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
            if agent.status == Status::Done
                && agent.pane_id.is_some()
                && agent.pane_id == self.focused_pane_id
            {
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
        let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .cloned()
            .or_else(|| get_tab_info(tab_id))
        else {
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
                .with_args(BTreeMap::from([(
                    "target_client_id".to_string(),
                    get_plugin_ids().client_id.to_string(),
                )]))
                .new_plugin_instance_should_float(true)
                .new_plugin_instance_should_have_pane_title(DASHBOARD_PANE_TITLE);
            if let Some(id) = self.dashboard_in_tab(tab_position) {
                message.plugin_url = None;
                message.destination_plugin_id = Some(id);
            }
            if let Some(args) = message.new_plugin_args.as_mut() {
                args.should_focus = Some(false);
            }
            if message.destination_plugin_id.is_none() {
                message.message_payload = Some(
                    serde_json::to_string(&AgentSnapshot {
                        agents: self.agents.clone(),
                        error: self.error.clone(),
                    })
                    .unwrap(),
                );
            }
            if let Some(source) = self.status_source {
                message
                    .message_args
                    .insert("status_source".into(), source.to_string());
            }
            pipe_message_to_plugin(message);
            return;
        }
        self.dashboard_visible = true;
        // Status snapshots arrive in the background; opening only displays the cache.
        if focused_pane_id == PaneId::Plugin(get_plugin_ids().plugin_id) {
            self.center_dashboard();
            return;
        }

        let focused_pane_info = get_pane_info(focused_pane_id);
        let codex_surface_is_visible = focused_pane_info
            .as_ref()
            .is_some_and(|pane| !pane.is_floating && !pane.is_suppressed);
        self.visible_floating_pane = focused_pane_info
            .filter(|pane| pane.is_floating && !pane.is_suppressed)
            .map(|pane| FloatingPaneState {
                pane_id: focused_pane_id,
                tab_id: self.tab_id_at_position(tab_position),
                rectangle: pane_rectangle(&pane),
            });
        self.suppressed_floating_panes.clear();
        self.pending_floating_pane_suppression = None;
        let tab_id = self.tab_id_at_position(tab_position);
        let panes_to_suppress = if codex_surface_is_visible {
            self.floating_pane_states(tab_position, tab_id)
        } else {
            Vec::new()
        };
        if codex_surface_is_visible && panes_to_suppress.is_empty() {
            self.pending_floating_pane_suppression = Some(PendingFloatingPaneSuppression {
                tab_position,
                tab_id,
            });
        }

        if !self.dashboard_named {
            rename_plugin_pane(get_plugin_ids().plugin_id, DASHBOARD_PANE_TITLE);
            self.dashboard_named = true;
        }
        self.suppress_floating_panes(panes_to_suppress);
        show_self(true);
        self.center_dashboard();
    }

    fn hide_dashboard(&mut self) {
        self.dashboard_visible = false;
        self.pending_floating_pane_suppression = None;
        hide_self();
        if let Some(pane) = self.visible_floating_pane.take() {
            change_floating_panes_coordinates(vec![(pane.pane_id, pane.rectangle.coordinates())]);
        }
        let mut tabs_to_hide = BTreeSet::new();
        for pane in self.suppressed_floating_panes.drain(..) {
            show_pane_with_id(pane.pane_id, true, false);
            change_floating_panes_coordinates(vec![(pane.pane_id, pane.rectangle.coordinates())]);
            tabs_to_hide.insert(pane.tab_id);
        }
        for tab_id in tabs_to_hide {
            if let Err(error) = hide_floating_panes(tab_id) {
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

    fn agent_tab_name(&self, agent: &AgentReport) -> &str {
        self.tabs
            .iter()
            .find(|tab| match agent.tab_id {
                Some(tab_id) => tab.tab_id == tab_id,
                None => agent.tab_position == Some(tab.position),
            })
            .map(|tab| tab.name.as_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("—")
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

    fn floating_pane_states(
        &self,
        tab_position: usize,
        tab_id: Option<usize>,
    ) -> Vec<FloatingPaneState> {
        let Some(panes) = self.pane_manifest.panes.get(&tab_position) else {
            return Vec::new();
        };
        // PaneUpdate can lag behind restoration, so recheck candidates against
        // live state before deciding which panes opening the dashboard would reveal.
        floating_pane_candidates(panes)
            .into_iter()
            .filter(|pane_id| *pane_id != PaneId::Plugin(get_plugin_ids().plugin_id))
            .filter_map(|pane_id| {
                get_pane_info(pane_id)
                    .filter(|pane| pane.is_floating && !pane.is_suppressed && !pane.exited)
                    .map(|pane| FloatingPaneState {
                        pane_id,
                        tab_id,
                        rectangle: pane_rectangle(&pane),
                    })
            })
            .collect()
    }

    fn dashboard_in_tab(&self, tab_position: usize) -> Option<u32> {
        self.pane_manifest
            .panes
            .get(&tab_position)?
            .iter()
            .filter(|pane| {
                pane.is_plugin
                    && !pane.exited
                    && pane.plugin_url == self.plugin_url
                    && Some(pane.id) != self.status_source
            })
            .map(|pane| pane.id)
            .min()
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
        if !self.owns_status() {
            return false;
        }
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
        self.recovered_running_panes
            .retain(|pane_id| live_pane_ids.contains(pane_id));
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
            if let Some(agent) = self
                .agents
                .values_mut()
                .find(|agent| agent.pane_id == Some(pane_id))
            {
                agent.tab_id = tab_id;
                agent.tab_position = Some(tab_position);
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
        if self.plugin_url.is_none() {
            self.plugin_url = pane_manifest
                .panes
                .values()
                .flatten()
                .find(|pane| pane.is_plugin && Some(pane.id) == self.plugin_id)
                .and_then(|pane| pane.plugin_url.clone());
        }
        let live_pane_ids = pane_manifest
            .panes
            .values()
            .flatten()
            .filter(|pane| !pane.is_plugin && !pane.exited)
            .map(|pane| pane.id)
            .collect::<BTreeSet<_>>();
        let mut should_render = false;
        if self.owns_status() {
            should_render |= self.reconcile_live_panes(&live_pane_ids);
            should_render |= self.discover_codex_panes(&pane_manifest);
        }
        self.pane_manifest = pane_manifest;
        if self.status_source.is_some() {
            for report in std::mem::take(&mut self.pending_reports) {
                self.receive_report(report);
            }
        }
        should_render |= self.refresh_current_view();
        if let Some(pending) = self.pending_floating_pane_suppression {
            self.pending_floating_pane_suppression = None;
            let panes = self.floating_pane_states(pending.tab_position, pending.tab_id);
            self.suppress_floating_panes(panes);
        }
        if self.owns_status() {
            self.publish_agents();
        }
        should_render
    }

    fn update_session(&mut self, session: SessionInfo) {
        if session.plugins.is_empty() {
            return;
        }
        let Some(id) = self.plugin_id else {
            return;
        };
        if let Some(plugin) = session.plugins.get(&id) {
            self.plugin_url = Some(plugin.location.clone());
        }
        let Some(url) = self.plugin_url.as_ref() else {
            return;
        };
        let peers = session
            .plugins
            .iter()
            .filter(|(_, plugin)| &plugin.location == url)
            .collect::<Vec<_>>();
        let source = monitor_source(&session.plugins, url);
        let previous_source = self.status_source;
        self.status_source = source;
        if source.is_some() {
            self.monitor_start_pending = false;
        }
        if self.owns_status() {
            self.status_observers = peers.iter().map(|(id, _)| **id).collect();
            self.publish_agents();
        } else if source.is_none()
            && !self.monitor_start_pending
            && peers.first().is_some_and(|(peer, _)| **peer == id)
        {
            self.monitor_start_pending = true;
            #[cfg(target_family = "wasm")]
            load_new_plugin(
                "zellij:OWN_URL",
                BTreeMap::from([("role".into(), "monitor".into())]),
                true,
                false,
            );
        } else if source != previous_source {
            self.request_agent_sync(false);
        }
        if self.status_source.is_some() {
            for report in std::mem::take(&mut self.pending_reports) {
                self.receive_report(report);
            }
        }
    }

    fn refresh_agents(&mut self) {
        if self.owns_status() {
            self.error = None;
            self.update_pane_manifest(self.pane_manifest.clone());
        } else {
            self.request_agent_sync(true);
        }
    }

    fn owns_status(&self) -> bool {
        self.plugin_id.is_none() || (self.is_monitor && self.status_source == self.plugin_id)
    }

    fn request_agent_sync(&self, _refresh: bool) {
        #[cfg(target_family = "wasm")]
        if let Some(source) = self.status_source {
            let mut message =
                MessageToPlugin::new(SYNC_AGENTS_PIPE_NAME).with_destination_plugin_id(source);
            if _refresh {
                message.message_args.insert("refresh".into(), "true".into());
            }
            pipe_message_to_plugin(message);
        }
    }

    fn receive_report(&mut self, message: PipeMessage) -> bool {
        if !self.owns_status() {
            if !message.is_private {
                return false;
            }
            if let Some(source) = self.status_source {
                let mut forwarded =
                    MessageToPlugin::new(PIPE_NAME).with_destination_plugin_id(source);
                if let PipeSource::Cli(pipe_id) = &message.source {
                    forwarded
                        .message_args
                        .insert("origin_pipe_id".into(), pipe_id.clone());
                } else {
                    forwarded.message_args = message.args;
                }
                forwarded.message_payload = message.payload;
                pipe_message_to_plugin(forwarded);
            } else {
                self.pending_reports.push(message);
            }
            return false;
        }
        let Some(payload) = message.payload else {
            return false;
        };
        let origin = match message.source {
            PipeSource::Cli(pipe_id) => Some(pipe_id),
            _ => message.args.get("origin_pipe_id").cloned(),
        };
        if let Some(origin) = origin {
            let key = (origin, payload.clone());
            if self.seen_reports.contains(&key) {
                return false;
            }
            self.seen_reports.push_back(key);
            if self.seen_reports.len() > 64 {
                self.seen_reports.pop_front();
            }
        }
        match serde_json::from_str(&payload) {
            Ok(agent) => {
                self.apply_report(agent);
                self.refresh_current_view();
                self.error = None;
            }
            Err(error) => self.error = Some(format!("invalid status report: {error}")),
        }
        self.publish_agents();
        true
    }

    fn accept_snapshot(&mut self, source: u32, snapshot: AgentSnapshot) -> bool {
        if self.owns_status() || self.status_source != Some(source) {
            return false;
        }
        let changed = self.agents != snapshot.agents || self.error != snapshot.error;
        self.agents = snapshot.agents;
        self.error = snapshot.error;
        changed
    }

    fn publish_agents(&mut self) {
        if !self.owns_status() {
            return;
        }
        let snapshot = AgentSnapshot {
            agents: self.agents.clone(),
            error: self.error.clone(),
        };
        let changed = self.last_published.as_ref() != Some(&snapshot);
        let peers: BTreeSet<u32> = self
            .pane_manifest
            .panes
            .values()
            .flatten()
            .filter(|pane| pane.is_plugin && !pane.exited && pane.plugin_url == self.plugin_url)
            .map(|pane| pane.id)
            .chain(self.status_observers.iter().copied())
            .filter(|id| Some(*id) != self.plugin_id)
            .collect();
        let recipients = if changed {
            peers.clone()
        } else {
            peers.difference(&self.published_peers).copied().collect()
        };
        if changed {
            self.schedule_checkpoint();
        }
        self.last_published = Some(snapshot.clone());
        self.published_peers = peers;
        if recipients.is_empty() {
            return;
        }
        #[cfg(target_family = "wasm")]
        {
            let ids = get_plugin_ids();
            let snapshot = serde_json::to_string(&snapshot).unwrap();
            for id in &recipients {
                pipe_message_to_plugin(
                    MessageToPlugin::new(AGENT_SNAPSHOT_PIPE_NAME)
                        .with_destination_plugin_id(*id)
                        .with_payload(snapshot.clone())
                        .with_args(BTreeMap::from([(
                            "target_client_id".to_string(),
                            ids.client_id.to_string(),
                        )])),
                );
            }
        }
        let _ = recipients;
    }

    fn handle_event(&mut self, event: Event) -> bool {
        match event {
            Event::Timer(_) => {
                if self.bootstrap_pending {
                    self.bootstrap_pending = false;
                    if self.status_source.is_none() || self.plugin_url.is_none() {
                        if let Ok(snapshot) = get_session_list() {
                            if let Some(session) = snapshot
                                .live_sessions
                                .into_iter()
                                .find(|s| s.is_current_session)
                            {
                                self.update_session(session);
                            }
                        }
                    }
                    if self.status_source.is_none() || self.plugin_url.is_none() {
                        self.bootstrap_pending = true;
                        set_timeout(1.0);
                    }
                }
                if self.checkpoint_pending {
                    self.checkpoint_pending = false;
                    self.save_agents();
                    return false;
                }
                false
            }
            Event::Key(key) if key.bare_key == BareKey::Esc && key.key_modifiers.is_empty() => {
                self.hide_dashboard();
                false
            }
            Event::Key(key)
                if key.bare_key == BareKey::Char('r') && key.key_modifiers.is_empty() =>
            {
                self.refresh_agents();
                true
            }
            Event::PaneRenderReport(panes) => {
                if !self.owns_status() {
                    return false;
                }
                let mut changed = false;
                for (pane_id, contents) in panes {
                    if let PaneId::Terminal(pane_id) = pane_id {
                        changed |= self.observe_pane_contents(pane_id, &contents.viewport);
                    }
                }
                changed |= self.refresh_current_view();
                if changed {
                    self.publish_agents();
                }
                changed
            }
            Event::PaneUpdate(pane_manifest) => self.update_pane_manifest(pane_manifest),
            Event::TabUpdate(tabs) => {
                let tabs_changed = self.tabs != tabs;
                self.tabs = tabs;
                let status_changed = self.refresh_current_view();
                if status_changed {
                    self.publish_agents();
                }
                tabs_changed | status_changed
            }
            _ => false,
        }
    }

    fn handle_message(&mut self, message: PipeMessage) -> bool {
        #[cfg(target_family = "wasm")]
        if let PipeSource::Cli(pipe_id) = &message.source {
            unblock_cli_pipe_input(pipe_id);
        }
        #[cfg(target_family = "wasm")]
        if message.name == "stop_dashboard" {
            if let Ok(snapshot) = get_session_list() {
                if let Some(session) = snapshot.live_sessions.iter().find(|s| s.is_current_session)
                {
                    let own_id = get_plugin_ids().plugin_id;
                    if let Some(own_plugin) = session.plugins.get(&own_id) {
                        for (id, plugin) in &session.plugins {
                            if plugin.location == own_plugin.location && *id != own_id {
                                close_plugin_pane(*id);
                            }
                        }
                    }
                    // Background plugins have no pane for the CLI close-pane
                    // action to find; the plugin API also unloads those instances.
                    close_plugin_pane(own_id);
                }
            }
            return false;
        }
        #[cfg(target_family = "wasm")]
        if message.name == "list_agents" {
            if let PipeSource::Cli(pipe_id) = &message.source {
                if message.args.contains_key("include_metadata") {
                    cli_pipe_output(
                        pipe_id,
                        &format!(
                            "{}\n",
                            serde_json::json!({
                                "plugin_id": self.plugin_id,
                                "client_id": get_plugin_ids().client_id,
                                "status_source": self.status_source,
                                "is_monitor": self.is_monitor,
                                "error": self.error,
                                "agents": self.agents,
                            })
                        ),
                    );
                    return false;
                }
                if let Ok(payload) = serde_json::to_string(&self.agents) {
                    cli_pipe_output(pipe_id, &format!("{payload}\n"));
                }
            }
            return false;
        }
        if message.name == AGENT_SNAPSHOT_PIPE_NAME {
            if message.args.get("target_client_id") != Some(&get_plugin_ids().client_id.to_string())
            {
                return false;
            }
            if let (PipeSource::Plugin(source), Some(payload)) = (&message.source, &message.payload)
            {
                if let Ok(snapshot) = serde_json::from_str(payload) {
                    return self.accept_snapshot(*source, snapshot);
                }
            }
            return false;
        }
        if message.name == SYNC_AGENTS_PIPE_NAME {
            if self.owns_status() {
                if let PipeSource::Plugin(source) = message.source {
                    self.status_observers.insert(source);
                    self.published_peers.remove(&source);
                }
                if message.args.contains_key("refresh") {
                    self.refresh_agents();
                } else {
                    self.publish_agents();
                }
                return true;
            }
            return false;
        }
        if message.name == SHOW_DASHBOARD_PIPE_NAME {
            if matches!(message.source, PipeSource::Plugin(_)) {
                if message.args.get("target_client_id")
                    != Some(&get_plugin_ids().client_id.to_string())
                {
                    return false;
                }
            }
            if self.is_monitor {
                return false;
            }
            if let Some(source) = message
                .args
                .get("status_source")
                .and_then(|id| id.parse().ok())
            {
                self.status_source = Some(source);
                if let Some(payload) = message.payload.as_ref() {
                    if let Ok(snapshot) = serde_json::from_str(payload) {
                        self.accept_snapshot(source, snapshot);
                    }
                }
            }
            self.show_dashboard();
            if message.payload.is_some() {
                self.request_agent_sync(false);
            }
            return true;
        }
        if message.name != PIPE_NAME {
            return false;
        }

        self.receive_report(message)
    }

    fn schedule_checkpoint(&mut self) {
        if !self.checkpoint_pending {
            self.checkpoint_pending = true;
            #[cfg(target_family = "wasm")]
            set_timeout(0.5);
        }
    }

    fn save_agents(&self) {
        #[cfg(target_family = "wasm")]
        if let Err(error) = serde_json::to_vec(&self.agents)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                let path = report_cache_path();
                let ids = get_plugin_ids();
                let temporary = format!("{path}.{}.{}", ids.plugin_id, ids.client_id);
                std::fs::write(&temporary, bytes)?;
                std::fs::rename(temporary, path)
            })
        {
            eprintln!("could not cache agent reports: {error}");
        }
    }

    fn suppress_floating_panes(&mut self, panes: Vec<FloatingPaneState>) {
        for pane in panes {
            hide_pane_with_id(pane.pane_id);
            self.suppressed_floating_panes.push(pane);
        }
    }
}

impl ZellijPlugin for App {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.plugin_id = Some(get_plugin_ids().plugin_id);
        self.is_monitor = configuration
            .get("role")
            .is_some_and(|role| role == "monitor");
        self.dashboard_visible = !self.is_monitor;
        if self.is_monitor {
            self.status_source = self.plugin_id;
        }
        self.dashboard_tab_id = configuration
            .get("dashboard_tab_id")
            .and_then(|value| value.parse().ok());
        self.dashboard_named = self.dashboard_tab_id.is_some();
        #[cfg(target_family = "wasm")]
        if self.is_monitor {
            match std::fs::read(report_cache_path()) {
                Ok(bytes) => match serde_json::from_slice(&bytes) {
                    Ok(agents) => self.agents = agents,
                    Err(error) => eprintln!("could not decode cached agent reports: {error}"),
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => eprintln!("could not read cached agent reports: {error}"),
            }
        }
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::ReadCliPipes,
            PermissionType::ReadPaneContents,
            PermissionType::MessageAndLaunchOtherPlugins,
        ]);
        let mut events = vec![
            EventType::Key,
            EventType::PaneUpdate,
            EventType::TabUpdate,
            EventType::Timer,
        ];
        if self.is_monitor {
            events.push(EventType::PaneRenderReport);
        }
        subscribe(&events);
        self.bootstrap_pending = true;
        set_timeout(0.1);
    }

    fn update(&mut self, event: Event) -> bool {
        self.handle_event(event) && self.dashboard_visible
    }

    fn pipe(&mut self, message: PipeMessage) -> bool {
        self.handle_message(message) && self.dashboard_visible
    }

    fn render(&mut self, _rows: usize, _cols: usize) {
        if let Some(error) = &self.error {
            println!("\u{1b}[91m✕ {error}\u{1b}[0m");
            println!("r: refresh agents · Esc: hide dashboard");
            return;
        }

        if self.agents.is_empty() {
            println!("Waiting for a Codex status report…");
            println!("r: refresh agents · Esc: hide dashboard");
            return;
        }

        println!(
            "{:<20}  {:<20}  {:<18}  {:<12}  TASK",
            "TAB", "WORKTREE", "AGENT", "STATUS"
        );
        for agent in self.agents.values() {
            let pane = agent.pane_id.map(|id| format!("p{id}")).unwrap_or_default();
            let agent_label = if pane.is_empty() {
                agent.agent.clone()
            } else {
                format!("{} ({pane})", agent.agent)
            };
            println!(
                "{:<20}  {:<20}  {:<18}  \u{1b}[{}m{} {:<9}\u{1b}[0m  {}",
                truncate(self.agent_tab_name(agent), 20),
                truncate(&agent.worktree, 20),
                truncate(&agent_label, 18),
                agent.status.ansi_color(),
                agent.status.symbol(),
                agent.status.label(),
                truncate(&agent.task, 80),
            );
        }
        println!();
        println!("r: refresh agents · Esc: hide dashboard");
    }
}

fn monitor_source(plugins: &BTreeMap<u32, PluginInfo>, url: &str) -> Option<u32> {
    plugins
        .iter()
        .find(|(_, plugin)| {
            plugin.location == url
                && plugin
                    .configuration
                    .get("role")
                    .is_some_and(|role| role == "monitor")
        })
        .map(|(id, _)| *id)
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

fn floating_pane_candidates(panes: &[PaneInfo]) -> Vec<PaneId> {
    panes
        .iter()
        .filter(|pane| !pane.exited && (pane.is_floating || pane.is_suppressed))
        .map(|pane| {
            if pane.is_plugin {
                PaneId::Plugin(pane.id)
            } else {
                PaneId::Terminal(pane.id)
            }
        })
        .collect()
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
    fn dashboards_share_status_without_reinterpreting_the_current_tab() {
        let mut owner = App {
            plugin_id: Some(4),
            status_source: Some(4),
            is_monitor: true,
            ..Default::default()
        };
        let mut dashboard = App {
            plugin_id: Some(7),
            status_source: Some(4),
            ..Default::default()
        };
        for status in [Status::Running, Status::Done, Status::Idle] {
            owner.apply_report(AgentReport {
                id: "agent".into(),
                agent: "codex".into(),
                status,
                task: String::new(),
                worktree: String::new(),
                pane_id: Some(12),
                tab_id: Some(2),
                tab_position: Some(0),
                remove: false,
            });
            let snapshot = AgentSnapshot {
                agents: owner.agents.clone(),
                error: None,
            };
            assert!(dashboard.accept_snapshot(4, snapshot.clone()));
            assert!(!dashboard.accept_snapshot(
                99,
                AgentSnapshot {
                    agents: BTreeMap::new(),
                    error: None,
                }
            ));
            dashboard.update(Event::TabUpdate(vec![TabInfo {
                tab_id: 2,
                position: 0,
                active: true,
                ..Default::default()
            }]));
            assert!(!dashboard.refresh_current_view());
            dashboard.update(Event::PaneRenderReport(std::collections::HashMap::from([
                (
                    PaneId::Terminal(12),
                    PaneContents {
                        viewport: vec!["• Working (esc to interrupt)".into()],
                        ..Default::default()
                    },
                ),
            ])));
            assert_eq!(dashboard.agents, owner.agents);
            assert!(!owner.accept_snapshot(7, snapshot));
        }
    }

    #[test]
    fn relayed_duplicates_do_not_restore_an_older_status() {
        let mut app = App {
            plugin_id: Some(4),
            ..Default::default()
        };
        let running = PipeMessage::new(
            PipeSource::Cli("first-report".into()),
            PIPE_NAME,
            &Some(r#"{"id":"agent","agent":"codex","status":"running"}"#.into()),
            &None,
            true,
        );
        assert!(!app.receive_report(running.clone()));
        app.is_monitor = true;
        app.update_session(SessionInfo {
            plugins: BTreeMap::from([(
                4,
                PluginInfo {
                    location: "file:codex.wasm".into(),
                    configuration: BTreeMap::from([("role".into(), "monitor".into())]),
                },
            )]),
            ..Default::default()
        });
        assert_eq!(app.agents["agent"].status, Status::Running);
        assert!(app.receive_report(PipeMessage::new(
            PipeSource::Cli("second-report".into()),
            PIPE_NAME,
            &Some(r#"{"id":"agent","agent":"codex","status":"done"}"#.into()),
            &None,
            true,
        )));
        let mut forwarded = running;
        forwarded.source = PipeSource::Plugin(7);
        forwarded
            .args
            .insert("origin_pipe_id".into(), "first-report".into());
        assert!(!app.receive_report(forwarded));
        assert_eq!(app.agents["agent"].status, Status::Done);
    }

    #[test]
    fn status_owner_is_the_background_monitor() {
        let url = "file:codex.wasm";
        let mut plugins = BTreeMap::from([
            (
                4,
                PluginInfo {
                    location: url.into(),
                    ..Default::default()
                },
            ),
            (
                7,
                PluginInfo {
                    location: url.into(),
                    configuration: BTreeMap::from([("role".into(), "monitor".into())]),
                },
            ),
        ]);
        assert_eq!(monitor_source(&plugins, url), Some(7));
        plugins.remove(&7);
        assert_eq!(monitor_source(&plugins, url), None);
    }

    #[test]
    fn tab_names_follow_stable_ids_and_live_renames() {
        let mut app = App::default();
        app.tabs = vec![
            TabInfo {
                tab_id: 7,
                position: 0,
                name: "source".into(),
                ..Default::default()
            },
            TabInfo {
                tab_id: 8,
                position: 1,
                name: "other".into(),
                ..Default::default()
            },
        ];
        let report: AgentReport = serde_json::from_str(
            r#"{"id":"thread-1","agent":"codex","status":"idle","tab_id":7,"tab_position":1}"#,
        )
        .unwrap();
        assert_eq!(app.agent_tab_name(&report), "source");

        let mut tabs = app.tabs.clone();
        tabs[0].name = "renamed".into();
        assert!(app.handle_event(Event::TabUpdate(tabs)));
        assert_eq!(app.agent_tab_name(&report), "renamed");

        app.tabs.remove(0);
        assert_eq!(app.agent_tab_name(&report), "—");
    }

    #[test]
    fn tab_names_support_legacy_positions_and_unknown_locations() {
        let app = App {
            tabs: vec![TabInfo {
                tab_id: 7,
                position: 2,
                name: "legacy".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut report: AgentReport = serde_json::from_str(
            r#"{"id":"thread-1","agent":"codex","status":"idle","tab_position":2}"#,
        )
        .unwrap();
        assert_eq!(app.agent_tab_name(&report), "legacy");
        report.tab_position = None;
        assert_eq!(app.agent_tab_name(&report), "—");
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
    fn floating_candidates_include_manual_panes_and_stale_suppressed_panes() {
        let panes = vec![
            PaneInfo {
                id: 17,
                title: "[host] project / editor".into(),
                is_suppressed: true,
                ..PaneInfo::default()
            },
            PaneInfo {
                id: 18,
                title: "manual editor".into(),
                is_floating: true,
                ..PaneInfo::default()
            },
            PaneInfo {
                id: 19,
                is_plugin: true,
                is_floating: true,
                ..PaneInfo::default()
            },
            PaneInfo {
                id: 20,
                ..PaneInfo::default()
            },
            PaneInfo {
                id: 21,
                is_floating: true,
                exited: true,
                ..PaneInfo::default()
            },
        ];

        assert_eq!(
            floating_pane_candidates(&panes),
            vec![
                PaneId::Terminal(17),
                PaneId::Terminal(18),
                PaneId::Plugin(19)
            ]
        );
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
    fn manifest_refresh_prunes_closed_agents_and_updates_surviving_locations() {
        let mut app = App::default();
        app.tabs = vec![TabInfo {
            tab_id: 7,
            position: 2,
            name: "live".into(),
            ..Default::default()
        }];
        for pane_id in [1, 2, 3] {
            let report: AgentReport = serde_json::from_value(serde_json::json!({
                "id": format!("agent-{pane_id}"),
                "agent": "codex",
                "status": "running",
                "pane_id": pane_id,
                "tab_id": 99,
                "tab_position": 8,
            }))
            .unwrap();
            app.apply_report(report);
        }
        app.recovered_running_panes = BTreeSet::from([1, 2, 3]);
        let manifest = PaneManifest {
            panes: std::collections::HashMap::from([(
                2,
                vec![
                    PaneInfo {
                        id: 1,
                        ..Default::default()
                    },
                    PaneInfo {
                        id: 2,
                        exited: true,
                        ..Default::default()
                    },
                    PaneInfo {
                        id: 4,
                        terminal_command: Some("codex".into()),
                        ..Default::default()
                    },
                ],
            )]),
        };
        assert!(app.update_pane_manifest(manifest.clone()));
        assert_eq!(app.agents.len(), 2);
        let survivor = &app.agents["agent-1"];
        assert_eq!(survivor.status, Status::Running);
        assert_eq!((survivor.tab_id, survivor.tab_position), (Some(7), Some(2)));
        assert!(app.agents.contains_key("discovered:pane:4"));
        assert_eq!(app.recovered_running_panes, BTreeSet::from([1]));
        assert!(!app.update_pane_manifest(manifest));
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
