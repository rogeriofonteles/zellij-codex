//! Zellij plugin entry point and the first one-agent vertical slice.

use serde::Deserialize;
use std::collections::BTreeMap;
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
    error: Option<String>,
}

register_plugin!(App);

impl App {
    fn apply_report(&mut self, agent: AgentReport) {
        if agent.remove {
            self.agents.remove(&agent.id);
        } else {
            self.agents.insert(agent.id.clone(), agent);
        }
    }
}

impl ZellijPlugin for App {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        subscribe(&[EventType::Key]);
    }

    fn update(&mut self, event: Event) -> bool {
        if let Event::Key(key) = event {
            if key.bare_key == BareKey::Esc && key.key_modifiers.is_empty() {
                hide_self();
            }
        }
        false
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
