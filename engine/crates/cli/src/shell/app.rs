//! The interactive shell: an inline ratatui viewport pinned at the bottom
//! (editor box + status line) with everything finished pushed into normal
//! terminal scrollback above it.
//!
//! A statement runs on a worker thread (`client::statement`); this thread
//! keeps drawing, finds the statement's record on the coordinator by its
//! tag and polls it for the running line, and turns Ctrl-C into a cancel.
use crate::args::Options;
use crate::auth::Session;
use crate::client::session::{self as api, CliHttp, Cluster, Whoami};
use crate::client::statement::{self, Handle, SharedSession, StatementEvent, StatementRequest};
use crate::render;
use crate::shell::editor::{Editor, EditorAction};
use crate::shell::progress::{self, Phase, Progress};
use crate::shell::status::{StatusFacts, host_of, prompt, prompt_width, status_line};
use crate::theme::Theme;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The editor shows up to this many SQL lines before scrolling inside.
const EDITOR_MAX_LINES: u16 = 6;
const EDITOR_MAX_ROWS: u16 = EDITOR_MAX_LINES + 2;
const CLUSTER_POLL: Duration = Duration::from_secs(30);
/// The query record is polled this often while a statement runs.
const STATE_POLL: Duration = Duration::from_millis(250);
/// How many history polls look for the record by tag before giving up on
/// an id (a minute at `STATE_POLL`); the statement still finishes.
const TAG_POLLS: u32 = 240;
/// While queued for admission the cluster payload is refreshed this often
/// for the queue depth.
const QUEUED_CLUSTER_POLL: Duration = Duration::from_secs(1);
/// One spinner frame per this many milliseconds.
const SPINNER_FRAME_MS: u128 = 80;

type Term = Terminal<CrosstermBackend<io::Stdout>>;

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn refresh_cluster(app: &mut App, options: &Options) {
    refresh_cluster_fields(
        &app.session,
        &mut app.cluster,
        &mut app.last_cluster_poll,
        options,
    );
}

/// By field, so a caller holding another part of the app can refresh.
fn refresh_cluster_fields(
    session: &SharedSession,
    cluster: &mut Option<Cluster>,
    last_poll: &mut Instant,
    options: &Options,
) {
    if let Ok(fresh) = api::fetch_cluster(&lock(session), &options.server) {
        *cluster = Some(fresh);
    }
    *last_poll = Instant::now();
}

/// The session, recovered if a worker thread panicked while holding it.
fn lock(session: &SharedSession) -> MutexGuard<'_, Session> {
    session
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A statement on the coordinator.
struct Running {
    handle: Handle,
    progress: Progress,
    /// The interactive row limit appended to the statement, for the
    /// summary's note when the result fills it.
    preview_limit: Option<usize>,
    /// History polls made while the record was not yet found by tag.
    polls: u32,
    last_poll: Instant,
    cancel_requested: bool,
}

pub struct App {
    session: SharedSession,
    editor: Editor,
    theme: Theme,
    cluster: Option<Cluster>,
    whoami: Option<Whoami>,
    last_elapsed_ms: Option<u64>,
    last_scanned_rows: Option<u64>,
    history_path: Option<std::path::PathBuf>,
    last_cluster_poll: Instant,
    running: Option<Running>,
    /// Statements from one submission still to run, in order.
    pending: VecDeque<String>,
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

    /// Rows the inline viewport needs: the editor, the status line and,
    /// while a statement runs, the running line above the editor.
    fn viewport_rows(&self) -> u16 {
        self.editor.height(EDITOR_MAX_ROWS) + 1 + u16::from(self.running.is_some())
    }
}

pub fn run(session: Session, options: &mut Options) -> Result<(), String> {
    let theme = Theme::detect(&options.theme, true);
    let cluster = api::fetch_cluster(&session, &options.server).ok();
    let whoami = api::fetch_whoami(&session, &options.server).ok().flatten();
    let history_path = (!options.no_history)
        .then(|| {
            options
                .history_file
                .clone()
                .or_else(crate::input::default_history_file)
        })
        .flatten();
    let mut app = App {
        session: Arc::new(Mutex::new(session)),
        editor: Editor::new(),
        theme,
        cluster,
        whoami,
        last_elapsed_ms: None,
        last_scanned_rows: None,
        history_path,
        last_cluster_poll: Instant::now(),
        running: None,
        pending: VecDeque::new(),
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
    let result = event_loop(&mut app, options);
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

fn emit_error(terminal: &mut Term, message: &str, theme: &Theme) -> Result<(), String> {
    emit(
        terminal,
        vec![Line::styled(format!("error: {message}"), theme.error)],
    )
}

fn emit_blank(terminal: &mut Term) -> Result<(), String> {
    emit(terminal, vec![Line::raw("")])
}

fn is_quit(text: &str) -> bool {
    let word = text.trim().trim_end_matches(';').trim();
    word.eq_ignore_ascii_case("exit")
        || word.eq_ignore_ascii_case("quit")
        || matches!(word, ".quit" | ".exit" | ".q")
}

fn is_ctrl_c(key: &event::KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// Elapsed time the way the summary says it.
fn seconds(elapsed: Duration) -> String {
    let millis = elapsed.as_millis();
    if millis < 1000 {
        format!("{millis} ms")
    } else {
        format!("{:.2} s", elapsed.as_secs_f64())
    }
}

/// A statement's failure, worded as the batch path words it.
fn failure_message(failure: &CliHttp) -> String {
    if failure.timed_out {
        return "coordinator request timed out".to_owned();
    }
    if failure.connect {
        return format!("cannot connect to coordinator: {}", failure.message);
    }
    match failure.status {
        None => format!("coordinator request failed: {}", failure.message),
        Some(200) => failure.message.clone(),
        Some(status) => {
            let status = reqwest::StatusCode::from_u16(status)
                .map_or_else(|_| status.to_string(), |code| code.to_string());
            format!("coordinator returned HTTP {status}: {}", failure.message)
        }
    }
}

fn event_loop(app: &mut App, options: &mut Options) -> Result<(), String> {
    let host = host_of(&options.server);
    let mut rows = app.viewport_rows();
    let mut owned = make_terminal(rows)?;
    loop {
        let needed = app.viewport_rows();
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
        let running_line = app.running.as_ref().map(|running| {
            let tick = (running.handle.started.elapsed().as_millis() / SPINNER_FRAME_MS) as usize;
            progress::line(&running.progress, tick, &app.theme)
        });
        terminal
            .draw(|frame| {
                let editor_height = app.editor.height(EDITOR_MAX_ROWS);
                let (progress_area, editor_area, status_area) = match &running_line {
                    Some(_) => {
                        let [progress_area, editor_area, status_area] = Layout::vertical([
                            Constraint::Length(1),
                            Constraint::Length(editor_height),
                            Constraint::Length(1),
                        ])
                        .areas(frame.area());
                        (Some(progress_area), editor_area, status_area)
                    }
                    None => {
                        let [editor_area, status_area] = Layout::vertical([
                            Constraint::Length(editor_height),
                            Constraint::Length(1),
                        ])
                        .areas(frame.area());
                        (None, editor_area, status_area)
                    }
                };
                if let (Some(area), Some(line)) = (progress_area, running_line.clone()) {
                    frame.render_widget(Paragraph::new(line), area);
                }
                let [prompt_area, text_area] =
                    Layout::horizontal([Constraint::Length(prompt_width()), Constraint::Min(1)])
                        .areas(editor_area);
                frame.render_widget(
                    app.editor.widget(&app.theme, running_line.is_some()),
                    text_area,
                );
                // The rules span the whole width; the prompt sits on the first
                // text row between them.
                for y in [editor_area.y, editor_area.bottom().saturating_sub(1)] {
                    let rule = Rect {
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
                let prompt_row = Rect {
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

        if app.running.is_some() {
            poll_running(app, terminal, options)?;
        }

        if !event::poll(Duration::from_millis(66)).map_err(|error| error.to_string())? {
            if app.last_cluster_poll.elapsed() >= CLUSTER_POLL {
                refresh_cluster(app, options);
            }
            continue;
        }
        let Event::Key(key) = event::read().map_err(|error| error.to_string())? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if app.running.is_some() {
            // Only Ctrl-C means anything while a statement runs.
            if is_ctrl_c(&key) {
                interrupt_running(app, terminal, options)?;
            }
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
                let echo_prefix = if options.context_explicit {
                    format!("kaveon {}.{} › ", options.catalog, options.schema)
                } else {
                    "kaveon › ".to_owned()
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
                submit(app, terminal, options, &text)?;
            }
        }
    }
}

/// Dot commands, `.limit`, `help` and `clear` run here and now; SQL is
/// split into statements and started through the worker thread.
fn submit(
    app: &mut App,
    terminal: &mut Term,
    options: &mut Options,
    text: &str,
) -> Result<(), String> {
    if let Some(rest) = text.strip_prefix(".limit") {
        let argument = rest.trim();
        let argument = (!argument.is_empty()).then_some(argument);
        match crate::shell::rowlimit::parse_command(argument) {
            Ok(limit) => {
                if let Some(limit) = limit {
                    options.row_limit = limit;
                }
                emit_text(
                    terminal,
                    &format!(
                        "row limit {} for queries without LIMIT (1 to {}); scripts run with -e or -f are unlimited",
                        render::thousands(options.row_limit as i128),
                        render::thousands(crate::shell::rowlimit::HARD_ROW_LIMIT as i128)
                    ),
                    &app.theme,
                    false,
                )?;
            }
            Err(error) => emit_error(terminal, &error, &app.theme)?,
        }
        return emit_blank(terminal);
    }
    if text.starts_with('.') {
        let output = crate::remote::meta_command_to_string(&lock(&app.session), options, text);
        match output {
            Ok(output) => emit_text(terminal, output.trim_end_matches('\n'), &app.theme, false)?,
            Err(error) => emit_error(terminal, &error, &app.theme)?,
        }
        return emit_blank(terminal);
    }
    let word = text.trim().trim_end_matches(';').trim();
    if word.eq_ignore_ascii_case("help") {
        emit(terminal, render::help::help(&app.theme))?;
        return emit_blank(terminal);
    }
    if word.eq_ignore_ascii_case("clear") {
        return emit_blank(terminal);
    }
    match crate::input::split_statements(text) {
        Ok(statements) => {
            app.pending = statements.into();
            start_next(app, terminal, options)
        }
        Err(error) => {
            emit_error(terminal, &error, &app.theme)?;
            emit_blank(terminal)
        }
    }
}

/// Runs pending statements in order: SHOW, USE and DESCRIBE finish on this
/// thread; the first SQL statement goes to a worker thread and the rest
/// wait for it. An error drops what is left.
fn start_next(app: &mut App, terminal: &mut Term, options: &mut Options) -> Result<(), String> {
    use crate::shell::rowlimit::{HARD_ROW_LIMIT, Limited, inspect, refusal};
    while let Some(statement) = app.pending.pop_front() {
        let (sql, preview_limit) = match inspect(&statement, options.row_limit) {
            Limited::Appended(sql) => (sql, Some(options.row_limit)),
            Limited::Explicit(explicit) if explicit > HARD_ROW_LIMIT => {
                app.pending.clear();
                emit_error(terminal, &refusal(explicit), &app.theme)?;
                return emit_blank(terminal);
            }
            Limited::Explicit(_) | Limited::Unchanged => (statement.clone(), None),
        };
        let before = (options.catalog.clone(), options.schema.clone());
        let metadata = crate::remote::run_metadata_statement(&lock(&app.session), options, &sql);
        match metadata {
            Ok(Some(executed)) => {
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
                emit_blank(terminal)?;
            }
            Ok(None) => {
                let handle = statement::submit(
                    Arc::clone(&app.session),
                    options.server.clone(),
                    StatementRequest::new(&sql, options),
                    options.timeout,
                );
                app.editor.set_text(&statement);
                app.running = Some(Running {
                    handle,
                    progress: Progress::default(),
                    preview_limit,
                    polls: 0,
                    last_poll: Instant::now(),
                    cancel_requested: false,
                });
                return Ok(());
            }
            Err(error) => {
                app.pending.clear();
                let message =
                    crate::remote::explain_missing_table(&lock(&app.session), options, &error)
                        .unwrap_or(error);
                emit_error(terminal, &message, &app.theme)?;
                return emit_blank(terminal);
            }
        }
    }
    Ok(())
}

/// Every `STATE_POLL`: the record by tag until it is found, then by id.
/// Then whatever the worker thread has reported.
fn poll_running(app: &mut App, terminal: &mut Term, options: &mut Options) -> Result<(), String> {
    let Some(running) = app.running.as_mut() else {
        return Ok(());
    };
    let elapsed = running.handle.started.elapsed();
    running.progress.elapsed = elapsed;
    if running.last_poll.elapsed() >= STATE_POLL {
        running.last_poll = Instant::now();
        let record = match running.progress.query_id.clone() {
            Some(id) => api::fetch_query(&lock(&app.session), &options.server, &id).ok(),
            None if running.polls < TAG_POLLS => {
                running.polls += 1;
                api::find_query_by_tag(&lock(&app.session), &options.server, &running.handle.tag)
                    .ok()
                    .flatten()
            }
            None => None,
        };
        if let Some(record) = record {
            let mut progress = Progress::from_record(&record, elapsed);
            if running.cancel_requested {
                progress.phase = Phase::Cancelling;
            }
            if progress.phase == Phase::Queued {
                if app.last_cluster_poll.elapsed() >= QUEUED_CLUSTER_POLL {
                    refresh_cluster_fields(
                        &app.session,
                        &mut app.cluster,
                        &mut app.last_cluster_poll,
                        options,
                    );
                }
                progress.queue_ahead = app
                    .cluster
                    .as_ref()
                    .map(|cluster| cluster.queue_depth().saturating_sub(1))
                    .filter(|ahead| *ahead > 0);
            }
            running.progress = progress;
        }
    }
    let event = match running.handle.events.try_recv() {
        Ok(event) => event,
        Err(mpsc::TryRecvError::Empty) => return Ok(()),
        Err(mpsc::TryRecvError::Disconnected) => StatementEvent::Failed(CliHttp::local(
            "the statement's worker thread ended without a result",
        )),
    };
    let running = app
        .running
        .take()
        .expect("the running statement is present while its event is handled");
    finish(app, terminal, options, running, event)
}

/// The worker thread reported: the result and its summary, or the error,
/// into scrollback; then the next pending statement.
fn finish(
    app: &mut App,
    terminal: &mut Term,
    options: &mut Options,
    running: Running,
    event: StatementEvent,
) -> Result<(), String> {
    app.editor.clear();
    match event {
        StatementEvent::Finished(result) => {
            let names = result.column_names();
            let table = crate::output::format_rows(&names, &result.data, options.output_format);
            emit_text(terminal, table.trim_end_matches('\n'), &app.theme, false)?;
            let summary = crate::remote::statement_summary(
                &lock(&app.session),
                options,
                &result.id,
                result.data.len(),
                result.elapsed_ms,
                running.preview_limit,
            );
            app.last_elapsed_ms = Some(result.elapsed_ms);
            app.last_scanned_rows = summary.as_ref().and_then(|summary| summary.rows_scanned);
            if let Some(summary) = summary {
                emit(terminal, render::summary::lines(&summary, &app.theme))?;
            }
            emit_blank(terminal)?;
            start_next(app, terminal, options)
        }
        StatementEvent::Failed(failure) => {
            app.pending.clear();
            // Ours, or cancelled from elsewhere (the web UI, another client).
            let cancelled =
                running.cancel_requested || failure.code.as_deref() == Some("QUERY_CANCELED");
            if cancelled {
                let summary = render::summary::Summary {
                    ok: false,
                    cancelled: true,
                    elapsed_ms: running.handle.started.elapsed().as_millis() as u64,
                    query_id: running.progress.query_id.clone(),
                    ..render::summary::Summary::default()
                };
                emit(terminal, render::summary::lines(&summary, &app.theme))?;
            } else {
                let message = failure_message(&failure);
                let message =
                    crate::remote::explain_missing_table(&lock(&app.session), options, &message)
                        .unwrap_or(message);
                emit_error(terminal, &message, &app.theme)?;
            }
            emit_blank(terminal)
        }
    }
}

/// Ctrl-C while a statement runs: cancel it on the coordinator when its
/// id is known; a second press, or no id to cancel, abandons the wait.
fn interrupt_running(
    app: &mut App,
    terminal: &mut Term,
    options: &mut Options,
) -> Result<(), String> {
    let Some(running) = app.running.as_mut() else {
        return Ok(());
    };
    if !running.cancel_requested {
        if running.progress.query_id.is_none() {
            // One more look before giving up on a cancel: the record may
            // have appeared since the last poll.
            let found =
                api::find_query_by_tag(&lock(&app.session), &options.server, &running.handle.tag)
                    .ok()
                    .flatten();
            if let Some(record) = found {
                running.progress = Progress::from_record(&record, running.handle.started.elapsed());
            }
        }
        if let Some(id) = running.progress.query_id.clone() {
            let _ = api::cancel_query(&lock(&app.session), &options.server, &id);
            running.cancel_requested = true;
            running.progress.phase = Phase::Cancelling;
            return Ok(());
        }
    }
    let running = app
        .running
        .take()
        .expect("the running statement is present while it is interrupted");
    app.pending.clear();
    app.editor.clear();
    emit(
        terminal,
        vec![Line::from(vec![
            Span::styled(" ✗ ", app.theme.error),
            Span::raw(format!(
                "abandoned after {}; the statement may still finish on the coordinator",
                seconds(running.handle.started.elapsed())
            )),
        ])],
    )?;
    emit_blank(terminal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_read_as_the_batch_path_words_them() {
        let timed_out = CliHttp {
            status: None,
            code: None,
            message: "operation timed out".into(),
            timed_out: true,
            connect: false,
        };
        assert_eq!(failure_message(&timed_out), "coordinator request timed out");
        let refused = CliHttp {
            status: None,
            code: None,
            message: "connection refused".into(),
            timed_out: false,
            connect: true,
        };
        assert_eq!(
            failure_message(&refused),
            "cannot connect to coordinator: connection refused"
        );
        let statement = CliHttp {
            status: Some(200),
            code: None,
            message: "query q failed: table 'a.b.c' not found".into(),
            timed_out: false,
            connect: false,
        };
        assert_eq!(
            failure_message(&statement),
            "query q failed: table 'a.b.c' not found"
        );
        let http = CliHttp {
            status: Some(413),
            code: Some("RESULT_TOO_LARGE".into()),
            message: "inline result exceeds 16 MiB".into(),
            timed_out: false,
            connect: false,
        };
        assert_eq!(
            failure_message(&http),
            "coordinator returned HTTP 413 Payload Too Large: inline result exceeds 16 MiB"
        );
        let transport = CliHttp::local("session lock poisoned");
        assert_eq!(
            failure_message(&transport),
            "coordinator request failed: session lock poisoned"
        );
    }

    #[test]
    fn elapsed_reads_like_the_summary() {
        assert_eq!(seconds(Duration::from_millis(400)), "400 ms");
        assert_eq!(seconds(Duration::from_millis(4100)), "4.10 s");
    }

    #[test]
    fn quit_words() {
        assert!(is_quit("exit"));
        assert!(is_quit("QUIT;"));
        assert!(is_quit(".q"));
        assert!(!is_quit("SELECT 1"));
    }
}
