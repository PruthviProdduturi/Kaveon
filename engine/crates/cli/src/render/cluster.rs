//! The session header printed once after connecting, and the `.cluster` panel.
use crate::client::session::{Cluster, Whoami};
use crate::render::human_bytes;
use crate::theme::Theme;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// The same rule as `Cluster::ready_workers`: the coordinator drops a
/// worker after 30 s without a heartbeat, so this only guards against
/// clock skew between the client and the coordinator.
const STALE_HEARTBEAT_SECS: u64 = 90;
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
        Line::raw(""),
        Line::from(Span::styled(format!("  {RULE}"), theme.accent)),
        Line::from(vec![
            Span::styled("  KAVEON", theme.title),
            Span::styled(format!("  v{}", facts.cli_version), theme.dim),
        ]),
        Line::from(Span::styled(format!("  {RULE}"), theme.accent)),
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

/// The `.cluster` panel: one line per node, then the coordinator's
/// admission state. A node whose heartbeat is stale is in the warning colour.
pub fn panel(cluster: &Cluster, now_unix: u64, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let nodes = std::iter::once((&cluster.coordinator, "coordinator"))
        .chain(cluster.workers.iter().map(|worker| (worker, "worker")));
    for (node, default_role) in nodes {
        let role = if node.role.is_empty() {
            default_role
        } else {
            node.role.as_str()
        };
        let age = now_unix.saturating_sub(node.last_heartbeat);
        let mut facts = vec![
            role.to_owned(),
            format!("v{}", node.version),
            if node.last_heartbeat == 0 {
                "no heartbeat".to_owned()
            } else {
                format!("heartbeat {age} s ago")
            },
            format!("rss {}", human_bytes(node.memory_rss_bytes)),
        ];
        if let Some(limit) = node.memory_limit_bytes {
            facts.push(format!("limit {}", human_bytes(limit)));
        }
        let stale = node.last_heartbeat == 0 || age > STALE_HEARTBEAT_SECS;
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(node.node_id.clone(), theme.accent),
            Span::styled(
                format!(" · {}", facts.join(" · ")),
                if stale {
                    theme.warning
                } else {
                    Style::default()
                },
            ),
        ]));
    }
    let admission = match &cluster.coordinator.admission {
        Some(admission) => format!(
            "  admission {} / {} admitted · queue depth {}",
            human_bytes(admission.admitted_bytes),
            human_bytes(admission.limit_bytes),
            admission.queue_depth
        ),
        None => "  admission unavailable".to_owned(),
    };
    lines.push(Line::from(Span::styled(admission, theme.dim)));
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
        assert!(text.starts_with("\n  ─"), "{text}");
        assert!(text.contains("\n  KAVEON  v0.3.0\n"), "{text}");
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

    #[test]
    fn panel_lists_nodes_and_admission() {
        let mut coordinator = node("coordinator-1", 997);
        coordinator.role = "coordinator".into();
        coordinator.memory_rss_bytes = 22 * 1024 * 1024 + 104_858;
        coordinator.memory_limit_bytes = Some(4 * 1024 * 1024 * 1024);
        coordinator.admission = Some(Admission {
            limit_bytes: 4 * 1024 * 1024 * 1024,
            admitted_bytes: 512 * 1024 * 1024,
            queue_depth: 2,
        });
        let mut w1 = node("w1", 999);
        w1.role = "worker".into();
        w1.memory_rss_bytes = 96 * 1024 * 1024;
        w1.memory_limit_bytes = Some(2 * 1024 * 1024 * 1024);
        let mut w2 = node("w2", 800);
        w2.memory_rss_bytes = 1024 * 1024;
        let cluster = Cluster {
            environment: "docker".into(),
            coordinator,
            workers: vec![w1, w2],
        };
        let lines = panel(&cluster, 1000, &Theme::mono());
        let text = crate::render::to_plain(&lines);
        assert_eq!(
            text,
            "  coordinator-1 · coordinator · v0.1.0 · heartbeat 3 s ago · rss 22.1 MiB · limit 4.0 GiB\n  \
             w1 · worker · v0.1.0 · heartbeat 1 s ago · rss 96.0 MiB · limit 2.0 GiB\n  \
             w2 · worker · v0.1.0 · heartbeat 200 s ago · rss 1.0 MiB\n  \
             admission 512.0 MiB / 4.0 GiB admitted · queue depth 2\n"
        );
        let theme = Theme {
            warning: ratatui::style::Style::default().fg(ratatui::style::Color::Yellow),
            ..Theme::mono()
        };
        let lines = panel(&cluster, 1000, &theme);
        assert_eq!(lines[1].spans[2].style, Style::default());
        assert_eq!(lines[2].spans[2].style, theme.warning);
    }

    #[test]
    fn panel_without_admission_says_so() {
        let cluster = Cluster {
            environment: String::new(),
            coordinator: node("c", 0),
            workers: vec![],
        };
        let text = crate::render::to_plain(&panel(&cluster, 1000, &Theme::mono()));
        assert_eq!(
            text,
            "  c · coordinator · v0.1.0 · no heartbeat · rss 0 B\n  admission unavailable\n"
        );
    }
}
