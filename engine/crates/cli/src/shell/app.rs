//! The interactive shell: an inline ratatui viewport pinned at the bottom
//! (editor box + status line) with everything finished pushed into normal
//! terminal scrollback above it.
use crate::args::Options;
use crate::auth::Session;
use crate::client::session::{self as api, Cluster, Whoami};
use crate::render;
use crate::shell::editor::{Editor, EditorAction};
use crate::shell::status::{StatusFacts, host_of, prompt, prompt_width, status_line};
use crate::theme::Theme;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::io;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The editor shows up to this many SQL lines before scrolling inside.
const EDITOR_MAX_LINES: u16 = 6;
const EDITOR_MAX_ROWS: u16 = EDITOR_MAX_LINES + 2;
const CLUSTER_POLL: Duration = Duration::from_secs(30);

type Term = Terminal<CrosstermBackend<io::Stdout>>;

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

pub struct App {
    editor: Editor,
    theme: Theme,
    cluster: Option<Cluster>,
    whoami: Option<Whoami>,
    last_elapsed_ms: Option<u64>,
    last_scanned_rows: Option<u64>,
    history_path: Option<std::path::PathBuf>,
}

impl App {
    fn status_facts<'a>(
        &'a self,
        context: Option<(&'a str, &'a str)>,
        host: &'a str,
    ) -> StatusFacts<'a> {
        StatusFacts {
            context,
            host,
            workers_ready: self
                .cluster
                .as_ref()
                .map(|cluster| cluster.ready_workers(now_unix())),
            last_elapsed_ms: self.last_elapsed_ms,
            last_scanned_rows: self.last_scanned_rows,
        }
    }

    fn insecure_development(&self, options: &Options) -> bool {
        self.whoami
            .as_ref()
            .is_some_and(|who| who.auth == "development")
            || (self.whoami.is_none() && options.auth == "none")
    }
}

pub fn run(session: &Session, options: &mut Options) -> Result<(), String> {
    let theme = Theme::detect(&options.theme, true);
    let cluster = api::fetch_cluster(session, &options.server).ok();
    let whoami = api::fetch_whoami(session, &options.server).ok().flatten();
    let history_path = (!options.no_history)
        .then(|| {
            options
                .history_file
                .clone()
                .or_else(crate::input::default_history_file)
        })
        .flatten();
    let mut app = App {
        editor: Editor::new(),
        theme,
        cluster,
        whoami,
        last_elapsed_ms: None,
        last_scanned_rows: None,
        history_path,
    };
    if let Some(path) = &app.history_path
        && let Ok(text) = std::fs::read_to_string(path)
    {
        app.editor.set_history(
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| line.replace("\\n", "\n"))
                .collect(),
        );
    }

    if !options.no_header {
        let header = render::cluster::header(
            &render::cluster::HeaderFacts {
                cli_version: env!("CARGO_PKG_VERSION"),
                server: &options.server,
                cluster: app.cluster.as_ref(),
                whoami: app.whoami.as_ref(),
                auth_mode: &options.auth,
                insecure_development: app.insecure_development(options),
                user: &options.user,
                now_unix: now_unix(),
            },
            &app.theme,
        );
        print!("{}", render::to_ansi(&header));
    }

    enable_raw_mode().map_err(|error| format!("cannot enter raw mode: {error}"))?;
    let result = event_loop(&mut app, session, options);
    let _ = disable_raw_mode();
    println!();
    save_history(&app);
    result
}

/// An inline viewport of exactly `rows` lines at the cursor. The height is
/// fixed per terminal, so the loop makes a new one when the editor grows.
fn make_terminal(rows: u16) -> Result<Term, String> {
    Terminal::with_options(
        CrosstermBackend::new(io::stdout()),
        TerminalOptions {
            viewport: Viewport::Inline(rows),
        },
    )
    .map_err(|error| format!("cannot initialize the terminal: {error}"))
}

fn save_history(app: &App) {
    let Some(path) = &app.history_path else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tail: Vec<String> = app
        .editor
        .history()
        .iter()
        .rev()
        .take(1000)
        .rev()
        .map(|statement| statement.replace('\n', "\\n"))
        .collect();
    let _ = std::fs::write(path, tail.join("\n") + "\n");
}

/// Push finished lines above the viewport, into normal scrollback.
fn emit(terminal: &mut Term, lines: Vec<Line<'static>>) -> Result<(), String> {
    let height = lines.len() as u16;
    if height == 0 {
        return Ok(());
    }
    terminal
        .insert_before(height, |buf| {
            Paragraph::new(Text::from(lines)).render(buf.area, buf);
        })
        .map_err(|error| error.to_string())
}

fn emit_text(terminal: &mut Term, text: &str, theme: &Theme, dim: bool) -> Result<(), String> {
    let lines = text
        .lines()
        .map(|line| {
            if dim {
                Line::styled(line.to_owned(), theme.dim)
            } else {
                Line::raw(line.to_owned())
            }
        })
        .collect();
    emit(terminal, lines)
}

fn is_quit(text: &str) -> bool {
    let word = text.trim().trim_end_matches(';').trim();
    word.eq_ignore_ascii_case("exit")
        || word.eq_ignore_ascii_case("quit")
        || matches!(word, ".quit" | ".exit" | ".q")
}

fn event_loop(app: &mut App, session: &Session, options: &mut Options) -> Result<(), String> {
    let host = host_of(&options.server);
    let mut last_cluster_poll = Instant::now();
    let mut rows = app.editor.height(EDITOR_MAX_ROWS) + 1;
    let mut owned = make_terminal(rows)?;
    loop {
        let needed = app.editor.height(EDITOR_MAX_ROWS) + 1;
        if needed != rows {
            owned.clear().map_err(|error| error.to_string())?;
            drop(owned);
            owned = make_terminal(needed)?;
            rows = needed;
        }
        let terminal = &mut owned;
        let context = options
            .context_explicit
            .then_some((options.catalog.as_str(), options.schema.as_str()));
        terminal
            .draw(|frame| {
                let editor_height = app.editor.height(EDITOR_MAX_ROWS);
                let [editor_area, status_area] =
                    Layout::vertical([Constraint::Length(editor_height), Constraint::Length(1)])
                        .areas(frame.area());
                let [prompt_area, text_area] =
                    Layout::horizontal([Constraint::Length(prompt_width()), Constraint::Min(1)])
                        .areas(editor_area);
                frame.render_widget(app.editor.widget(&app.theme, false), text_area);
                // The rules span the whole width; the prompt sits on the first
                // text row between them.
                for y in [editor_area.y, editor_area.bottom().saturating_sub(1)] {
                    let rule = ratatui::layout::Rect {
                        y,
                        height: 1,
                        ..prompt_area
                    };
                    frame.render_widget(
                        Paragraph::new(Span::styled(
                            "─".repeat(prompt_area.width as usize),
                            app.theme.dim,
                        )),
                        rule,
                    );
                }
                let prompt_row = ratatui::layout::Rect {
                    y: (prompt_area.y + 1).min(editor_area.bottom().saturating_sub(1)),
                    height: 1,
                    ..prompt_area
                };
                frame.render_widget(Paragraph::new(prompt(&app.theme)), prompt_row);
                frame.render_widget(
                    Paragraph::new(status_line(&app.status_facts(context, &host), &app.theme)),
                    status_area,
                );
            })
            .map_err(|error| error.to_string())?;

        if !event::poll(Duration::from_millis(66)).map_err(|error| error.to_string())? {
            if last_cluster_poll.elapsed() >= CLUSTER_POLL {
                if let Ok(cluster) = api::fetch_cluster(session, &options.server) {
                    app.cluster = Some(cluster);
                }
                last_cluster_poll = Instant::now();
            }
            continue;
        }
        let Event::Key(key) = event::read().map_err(|error| error.to_string())? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match app.editor.handle(&key) {
            EditorAction::None => {}
            EditorAction::Quit => {
                terminal.clear().map_err(|error| error.to_string())?;
                return Ok(());
            }
            EditorAction::Clear => {
                terminal.clear().map_err(|error| error.to_string())?;
            }
            EditorAction::Interrupt => {
                if app.editor.is_empty() {
                    emit_text(terminal, "Ctrl-D or exit to leave", &app.theme, true)?;
                } else {
                    app.editor.clear();
                }
            }
            EditorAction::Submit(text) => {
                if !is_quit(&text) {
                    app.editor.push_history(text.clone());
                }
                let echo_prefix = match context {
                    Some((catalog, schema)) => format!("kaveon {catalog}.{schema} › "),
                    None => "kaveon › ".to_owned(),
                };
                let echo = text
                    .lines()
                    .enumerate()
                    .map(|(index, line)| {
                        let prefix = if index == 0 {
                            echo_prefix.clone()
                        } else {
                            " ".repeat(echo_prefix.chars().count())
                        };
                        Line::styled(format!("{prefix}{line}"), app.theme.dim)
                    })
                    .collect();
                emit(terminal, echo)?;
                if is_quit(&text) {
                    terminal.clear().map_err(|error| error.to_string())?;
                    return Ok(());
                }
                let before = (options.catalog.clone(), options.schema.clone());
                match run_statement(session, options, &text) {
                    Ok(executed) => {
                        if (options.catalog.as_str(), options.schema.as_str())
                            != (before.0.as_str(), before.1.as_str())
                        {
                            options.context_explicit = true;
                        }
                        emit_text(
                            terminal,
                            executed.output.trim_end_matches('\n'),
                            &app.theme,
                            false,
                        )?;
                        if let Some(elapsed) = executed.elapsed_ms {
                            app.last_elapsed_ms = Some(elapsed);
                            app.last_scanned_rows = executed.scanned_rows;
                        }
                    }
                    Err(error) => emit(
                        terminal,
                        vec![Line::styled(format!("error: {error}"), app.theme.error)],
                    )?,
                }
                emit(terminal, vec![Line::raw("")])?;
            }
        }
    }
}

/// Stage 1: statements run through the existing synchronous path and
/// return their plain output. The worker thread and the running line
/// replace this in stage 2.
fn run_statement(
    session: &Session,
    options: &mut Options,
    text: &str,
) -> Result<crate::remote::Executed, String> {
    let plain = |output: String| crate::remote::Executed {
        output,
        elapsed_ms: None,
        scanned_rows: None,
    };
    if let Some(rest) = text.strip_prefix(".limit") {
        let argument = rest.trim();
        let argument = (!argument.is_empty()).then_some(argument);
        if let Some(limit) = crate::shell::rowlimit::parse_command(argument)? {
            options.row_limit = limit;
        }
        return Ok(plain(format!(
            "row limit {} for queries without LIMIT (1 to {}); scripts run with -e or -f are unlimited\n",
            render::thousands(options.row_limit as i128),
            render::thousands(crate::shell::rowlimit::HARD_ROW_LIMIT as i128)
        )));
    }
    if text.starts_with('.') {
        return crate::remote::meta_command_to_string(session, options, text).map(plain);
    }
    let word = text.trim().trim_end_matches(';').trim();
    if word.eq_ignore_ascii_case("help") {
        let mut help = render::to_ansi(&render::help::help(&Theme::detect(&options.theme, true)));
        help.push('\n');
        return Ok(plain(help));
    }
    if word.eq_ignore_ascii_case("clear") {
        return Ok(plain(String::new()));
    }
    let mut merged = crate::remote::Executed {
        output: String::new(),
        elapsed_ms: None,
        scanned_rows: None,
    };
    for statement in crate::input::split_statements(text)? {
        use crate::shell::rowlimit::{HARD_ROW_LIMIT, Limited, inspect, refusal};
        let outcome = match inspect(&statement, options.row_limit) {
            Limited::Appended(sql) => {
                crate::remote::execute_with_limit(session, options, &sql, Some(options.row_limit))
            }
            Limited::Explicit(explicit) if explicit > HARD_ROW_LIMIT => {
                return Err(refusal(explicit));
            }
            Limited::Explicit(_) | Limited::Unchanged => {
                crate::remote::execute_to_string(session, options, &statement)
            }
        };
        let executed = match outcome {
            Ok(executed) => executed,
            Err(error) => {
                return Err(
                    crate::remote::explain_missing_table(session, options, &error).unwrap_or(error),
                );
            }
        };
        merged.output.push_str(&executed.output);
        if executed.elapsed_ms.is_some() {
            merged.elapsed_ms = executed.elapsed_ms;
            merged.scanned_rows = executed.scanned_rows;
        }
    }
    Ok(merged)
}
