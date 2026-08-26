//! Zellij plugin entry point and the first one-agent vertical slice.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use zellij_tile::prelude::*;

const PIPE_NAME: &str = "codex_status";

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
    remove: bool,
}

#[derive(Default)]
struct App {
    agents: BTreeMap<String, AgentReport>,
    focused_pane_id: Option<u32>,
    error: Option<String>,
}

register_plugin!(App);

impl App {
    fn apply_report(&mut self, mut agent: AgentReport) {
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

    fn observe_focused_pane(&mut self, pane_id: PaneId) {
        self.focused_pane_id = match pane_id {
            PaneId::Terminal(pane_id) => Some(pane_id),
            PaneId::Plugin(_) => None,
        };

        if let Some(pane_id) = self.focused_pane_id {
            for agent in self.agents.values_mut() {
                if agent.pane_id == Some(pane_id) && agent.status == Status::Done {
                    agent.status = Status::Idle;
                }
            }
        }
    }

    fn refresh_focused_pane(&mut self) {
        if let Ok((_, pane_id)) = get_focused_pane_info() {
            self.observe_focused_pane(pane_id);
        }
    }

    fn discover_codex_panes(&mut self, pane_manifest: PaneManifest) {
        let mut discovered = Vec::new();

        for pane in pane_manifest.panes.values().flatten() {
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
            discovered.push((pane.id, worktree));
        }

        self.reconcile_discovered_panes(discovered);
    }

    fn reconcile_discovered_panes(&mut self, discovered: Vec<(u32, String)>) {
        let discovered_pane_ids = discovered
            .iter()
            .map(|(pane_id, _)| *pane_id)
            .collect::<BTreeSet<_>>();
        self.agents.retain(|id, agent| {
            !id.starts_with("discovered:pane:")
                || agent
                    .pane_id
                    .is_some_and(|pane_id| discovered_pane_ids.contains(&pane_id))
        });

        for (pane_id, worktree) in discovered {
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
                    remove: false,
                },
            );
        }
    }
}

impl ZellijPlugin for App {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ReadCliPipes,
        ]);
        subscribe(&[EventType::Key, EventType::PaneUpdate, EventType::TabUpdate]);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key) => {
                if key.bare_key == BareKey::Esc && key.key_modifiers.is_empty() {
                    hide_self();
                }
            }
            Event::PaneUpdate(pane_manifest) => {
                self.discover_codex_panes(pane_manifest);
                self.refresh_focused_pane();
            }
            Event::TabUpdate(_) => self.refresh_focused_pane(),
            _ => {}
        }
        true
    }

    fn pipe(&mut self, message: PipeMessage) -> bool {
        if message.name != PIPE_NAME {
            return false;
        }

        let Some(payload) = message.payload else {
            return false;
        };

        match serde_json::from_str(&payload) {
            Ok(agent) => {
                let agent: AgentReport = agent;
                self.refresh_focused_pane();
                self.apply_report(agent);
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
            r#"{"id":"thread-1","agent":"implementation","status":"running","task":"Migrating grpc","worktree":"grpc-migration","pane_id":7}"#,
        )
        .unwrap();

        assert_eq!(report.agent, "implementation");
        assert_eq!(report.id, "thread-1");
        assert_eq!(report.status, Status::Running);
        assert_eq!(report.task, "Migrating grpc");
        assert_eq!(report.worktree, "grpc-migration");
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
            remove: true,
        });

        assert!(app.agents.is_empty());
    }

    #[test]
    fn discovers_codex_commands_started_by_a_workbench() {
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
    fn a_hook_report_replaces_the_discovered_pane() {
        let mut app = App::default();
        app.reconcile_discovered_panes(vec![(12, "demo".to_string())]);

        app.apply_report(AgentReport {
            id: "quadratic-tiger:pane:12".to_string(),
            agent: "codex-12".to_string(),
            status: Status::Running,
            task: "Implement pane discovery".to_string(),
            worktree: "demo".to_string(),
            pane_id: Some(12),
            remove: false,
        });

        assert_eq!(app.agents.len(), 1);
        assert_eq!(app.agents.values().next().unwrap().status, Status::Running);
        assert!(app.agents.contains_key("quadratic-tiger:pane:12"));
    }

    #[test]
    fn a_completed_response_is_done_until_its_pane_is_focused() {
        let mut app = App::default();
        let report = AgentReport {
            id: "quadratic-tiger:pane:12".to_string(),
            agent: "codex-12".to_string(),
            status: Status::Done,
            task: "Implemented the requested change".to_string(),
            worktree: "demo".to_string(),
            pane_id: Some(12),
            remove: false,
        };

        app.apply_report(report.clone());
        assert_eq!(app.agents[&report.id].status, Status::Done);

        app.observe_focused_pane(PaneId::Terminal(7));
        assert_eq!(app.agents[&report.id].status, Status::Done);

        app.observe_focused_pane(PaneId::Terminal(12));
        assert_eq!(app.agents[&report.id].status, Status::Idle);

        app.apply_report(report.clone());
        assert_eq!(app.agents[&report.id].status, Status::Idle);
    }

    #[test]
    fn removes_a_discovered_agent_after_its_pane_closes() {
        let mut app = App::default();
        app.reconcile_discovered_panes(vec![(12, "demo".to_string())]);

        app.reconcile_discovered_panes(Vec::new());

        assert!(app.agents.is_empty());
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
