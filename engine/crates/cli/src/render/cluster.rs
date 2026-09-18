//! The session header printed once after connecting, and the `.cluster` panel.
use crate::client::session::{Cluster, Whoami};
use crate::render::human_bytes;
use crate::theme::Theme;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

const RULE: &str = "─────────────────────────────────────────────────────────────";

/// What the header needs beyond the coordinator's payloads.
pub struct HeaderFacts<'a> {
    pub cli_version: &'a str,
    pub server: &'a str,
    pub cluster: Option<&'a Cluster>,
    pub whoami: Option<&'a Whoami>,
    pub auth_mode: &'a str,
    pub insecure_development: bool,
    pub user: &'a str,
    pub now_unix: u64,
}

fn row(label: &str, parts: Vec<Span<'static>>, theme: &Theme) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("  {label:<9} "), theme.dim)];
    spans.extend(parts);
    Line::from(spans)
}

fn joined(parts: &[String]) -> String {
    parts.join("  ·  ")
}

pub fn header(facts: &HeaderFacts<'_>, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec![
            Span::styled("  KAVEON", theme.title),
            Span::styled(format!("  v{}", facts.cli_version), theme.dim),
        ]),
        Line::from(Span::styled(format!("  {RULE}"), theme.dim)),
    ];
    match facts.cluster {
        Some(cluster) => {
            lines.push(row(
                "Engine",
                vec![Span::raw(joined(&[
                    facts.server.to_owned(),
                    format!("v{}", cluster.coordinator.version),
                    cluster.environment.clone(),
                ]))],
                theme,
            ));
            let (ready, stale) = cluster.ready_workers(facts.now_unix);
            let mut parts = vec![cluster.coordinator.node_id.clone()];
            let mut style = Style::default();
            if cluster.workers.is_empty() {
                parts.push("no workers — statements run on the coordinator".into());
                style = theme.warning;
            } else {
                parts.push(format!(
                    "{ready} worker{} ready",
                    if ready == 1 { "" } else { "s" }
                ));
                if stale > 0 {
                    parts.push(format!("{stale} stale"));
                    style = theme.warning;
                }
            }
            if let Some(limit) = cluster.admission_limit_bytes() {
                parts.push(format!("{} admission", human_bytes(limit)));
            }
            lines.push(row(
                "Cluster",
                vec![Span::styled(joined(&parts), style)],
                theme,
            ));
        }
        None => {
            lines.push(row(
                "Engine",
                vec![Span::raw(facts.server.to_owned())],
                theme,
            ));
            lines.push(row(
                "Cluster",
                vec![Span::styled("unavailable", theme.warning)],
                theme,
            ));
        }
    }
    let mut session = Vec::new();
    match facts.whoami {
        Some(who) => {
            session.push(who.display.clone().unwrap_or_else(|| who.principal.clone()));
            if !who.role.is_empty() {
                session.push(who.role.clone());
            }
        }
        None => session.push(facts.user.to_owned()),
    }
    session.push(if facts.insecure_development {
        format!("auth {} (insecure development)", facts.auth_mode)
    } else {
        format!("auth {}", facts.auth_mode)
    });
    lines.push(row("Session", vec![Span::raw(joined(&session))], theme));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  SQL ends with ;   .help for commands   Ctrl-C cancels a running query",
        theme.dim,
    )));
    lines.push(Line::raw(""));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::session::{Admission, Node};

    fn node(id: &str, heartbeat: u64) -> Node {
        Node {
            node_id: id.into(),
            role: String::new(),
            address: String::new(),
            version: "0.1.0".into(),
            environment: "docker".into(),
            last_heartbeat: heartbeat,
            memory_rss_bytes: 0,
            memory_limit_bytes: None,
            admission: None,
        }
    }

    fn facts<'a>(cluster: Option<&'a Cluster>, whoami: Option<&'a Whoami>) -> HeaderFacts<'a> {
        HeaderFacts {
            cli_version: "0.3.0",
            server: "http://localhost:8081",
            cluster,
            whoami,
            auth_mode: "none",
            insecure_development: true,
            user: "prproddu",
            now_unix: 1000,
        }
    }

    #[test]
    fn header_shows_engine_cluster_and_session_lines() {
        let mut coordinator = node("coordinator-1", 1000);
        coordinator.admission = Some(Admission {
            limit_bytes: 4 * 1024 * 1024 * 1024,
            admitted_bytes: 0,
            queue_depth: 0,
        });
        let cluster = Cluster {
            environment: "docker".into(),
            coordinator,
            workers: vec![node("w1", 1000), node("w2", 900)],
        };
        let whoami = Whoami {
            principal: "prproddu".into(),
            display: None,
            role: "admin".into(),
            auth: "development".into(),
        };
        let text = crate::render::to_plain(&header(
            &facts(Some(&cluster), Some(&whoami)),
            &Theme::mono(),
        ));
        assert!(text.starts_with("  KAVEON  v0.3.0\n"), "{text}");
        assert!(text.contains("Engine    http://localhost:8081  ·  v0.1.0  ·  docker"));
        assert!(text.contains(
            "Cluster   coordinator-1  ·  1 worker ready  ·  1 stale  ·  4.0 GiB admission"
        ));
        assert!(text.contains("Session   prproddu  ·  admin  ·  auth none (insecure development)"));
        assert!(
            text.contains("SQL ends with ;   .help for commands   Ctrl-C cancels a running query")
        );
    }

    #[test]
    fn header_without_cluster_or_whoami_degrades() {
        let mut facts = facts(None, None);
        facts.server = "https://engine.example";
        facts.auth_mode = "auto";
        facts.insecure_development = false;
        facts.user = "ana";
        let text = crate::render::to_plain(&header(&facts, &Theme::mono()));
        assert!(text.contains("Cluster   unavailable"));
        assert!(text.contains("Session   ana  ·  auth auto\n"));
        assert!(!text.contains("insecure"));
    }

    #[test]
    fn zero_workers_is_called_out() {
        let cluster = Cluster {
            environment: "docker".into(),
            coordinator: node("c", 1000),
            workers: vec![],
        };
        let text = crate::render::to_plain(&header(&facts(Some(&cluster), None), &Theme::mono()));
        assert!(text.contains("no workers — statements run on the coordinator"));
    }
}
