//! The interactive shell: an inline ratatui viewport pinned at the bottom
//! (editor box + status line) with everything finished pushed into normal
//! terminal scrollback above it.
//!
//! A statement runs on a worker thread; this thread keeps drawing. Against
//! a coordinator (`client::statement`) it finds the statement's record by
//! its tag and polls it for the running line, and turns Ctrl-C into a
//! cancel. With `--local` the worker thread runs the embedded engine and
//! the running line shows elapsed time only.
//!
//! A paged statement's rows are read while it runs: once the record carries
//! a `next_uri`, page 0 is tried every `STREAM_POLL` until the coordinator
//! has written it, then shown under the running line and paged from there
//! (`client::pages`), the summary following when the POST returns.
//!
//! `.source` feeds a file through the submit path, `.tee` copies scrollback
//! to a file, `.edit` hands the last statement to `$VISUAL`/`$EDITOR`,
//! `\G` asks for the vertical format once, and `.watch` re-runs a
//! statement on an interval until a key is pressed.
use crate::args::Options;
use crate::auth::Session;
use crate::client::error::{CliError, ErrorKind};
use crate::client::metadata::NameCache;
use crate::client::pages::{Fetched, Page, PageCursor, UNSAFE_NEXT_URI};
use crate::client::session::{self as api, CliHttp, Cluster, METADATA_TIMEOUT, Whoami};
use crate::client::statement::{
    self, Column, Handle, SharedSession, StatementEvent, StatementRequest, StatementResult,
};
use crate::local::LocalEngine;
use crate::local::catalog::{CatalogCommand, parse_catalog_command};
use crate::output::OutputFormat;
use crate::render;
use crate::shell::commands::{self, Command, SessionSettings};
use crate::shell::editor::{Editor, EditorAction};
use crate::shell::progress::{self, Phase, Progress};
use crate::shell::status::{StatusFacts, host_of, prompt, prompt_width, status_line};
use crate::theme::Theme;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};
use sqlparser::dialect::GenericDialect;
use sqlparser::tokenizer::Tokenizer;
use std::collections::VecDeque;
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use unicode_width::UnicodeWidthStr;

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
/// While a paged statement runs, page 0 is tried this often at most, and a
/// page the coordinator has not written yet is retried this often.
const STREAM_POLL: Duration = Duration::from_millis(500);
/// Page fetches while the statement runs are bounded short, so a slow
/// answer never holds the running line for long.
const STREAM_TIMEOUT: Duration = Duration::from_secs(5);
/// One spinner frame per this many milliseconds.
const SPINNER_FRAME_MS: u128 = 80;
/// Keys closer together than this are one paste (see the event loop).
const PASTE_GAP: Duration = Duration::from_millis(12);
const EMBEDDED_ONLY: &str = "not available in embedded mode";

type Term = Terminal<CrosstermBackend<io::Stdout>>;
type SharedEngine = Arc<Mutex<LocalEngine>>;

/// `.tee`: everything pushed into scrollback, ANSI-free, appended here.
struct Tee {
    path: PathBuf,
    file: std::fs::File,
}

/// The inline viewport plus the `.tee` copy of what goes above it. The
/// terminal is remade whenever the viewport's height changes or the
/// screen is reset; the tee outlives both.
struct Screen {
    terminal: Term,
    /// The viewport height the terminal was made with.
    rows: u16,
    tee: Option<Tee>,
    /// A tee write failed: reported once by the loop, the tee dropped.
    tee_error: Option<String>,
}

impl Screen {
    fn new(rows: u16) -> Result<Screen, String> {
        Ok(Screen {
            terminal: make_terminal(rows)?,
            rows,
            tee: None,
            tee_error: None,
        })
    }

    /// A fresh viewport of `rows` lines at the cursor, the old one cleared.
    fn resize(&mut self, rows: u16) -> Result<(), String> {
        self.terminal.clear().map_err(|error| error.to_string())?;
        self.terminal = make_terminal(rows)?;
        self.rows = rows;
        Ok(())
    }

    /// The viewport remade where the cursor is, the screen left alone:
    /// after a child process (`.edit`) has used the terminal.
    fn remake(&mut self, rows: u16) -> Result<(), String> {
        self.terminal = make_terminal(rows)?;
        self.rows = rows;
        Ok(())
    }

    /// The whole screen cleared and the viewport remade at the top:
    /// `.clear`, Ctrl-L, every `.watch` run, and around `.edit`.
    fn reset(&mut self, rows: u16) -> Result<(), String> {
        let mut stdout = io::stdout();
        crossterm::execute!(
            stdout,
            Clear(ClearType::All),
            crossterm::cursor::MoveTo(0, 0)
        )
        .map_err(|error| error.to_string())?;
        self.terminal = make_terminal(rows)?;
        self.rows = rows;
        Ok(())
    }

    /// Push finished lines above the viewport, into normal scrollback, and
    /// into the tee file when one is open.
    fn emit(&mut self, lines: Vec<Line<'static>>) -> Result<(), String> {
        let height = lines.len() as u16;
        if height == 0 {
            return Ok(());
        }
        if let Some(tee) = self.tee.as_mut() {
            let text = render::to_plain(&lines);
            if let Err(error) = tee.file.write_all(text.as_bytes()) {
                self.tee_error = Some(format!(
                    "cannot write to {}: {error}; tee off",
                    tee.path.display()
                ));
                self.tee = None;
            }
        }
        self.terminal
            .insert_before(height, |buf| {
                Paragraph::new(Text::from(lines)).render(buf.area, buf);
            })
            .map_err(|error| error.to_string())
    }
}

/// `.watch`: the statement, how often, and when it runs next.
struct Watch {
    statement: String,
    interval: Duration,
    next_run: Instant,
    started: Instant,
    runs: u32,
}

/// Where statements run: a coordinator over HTTP, or the embedded engine
/// in this process (`--local`). Both answer through the same
/// `StatementEvent` channel, so the loop, the renderers and the summary do
/// not care which.
pub enum Backend {
    Remote(SharedSession),
    Local(SharedEngine),
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn refresh_cluster(app: &mut App, options: &Options) {
    if let Backend::Remote(session) = &app.backend {
        refresh_cluster_fields(
            session,
            &mut app.cluster,
            &mut app.last_cluster_poll,
            options,
        );
    }
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

/// The embedded engine, likewise.
fn lock_engine(engine: &SharedEngine) -> MutexGuard<'_, LocalEngine> {
    engine
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One line of `.history`: what ran, how it went.
struct HistoryEntry {
    statement: String,
    elapsed_ms: Option<u64>,
    ok: bool,
}

/// A statement on the coordinator, or on the embedded engine's thread.
struct Running {
    handle: Handle,
    progress: Progress,
    /// The statement as typed (without the appended limit), for the error
    /// panel's excerpt and the history.
    sql: String,
    /// `EXPLAIN <statement>`: render the plan instead of the rows;
    /// `EXPLAIN ANALYZE` adds what the run cost.
    explain: Option<Explain>,
    /// The interactive row limit appended to the statement, for the
    /// summary's note when the result fills it.
    preview_limit: Option<usize>,
    /// History polls made while the record was not yet found by tag.
    polls: u32,
    last_poll: Instant,
    cancel_requested: bool,
    /// The rows read while the statement runs, for paged delivery.
    stream: Stream,
}

/// The pages of a running paged statement, read as the coordinator writes
/// them.
enum Stream {
    /// The record has not offered a `next_uri`: inline delivery, or not
    /// planned yet.
    Off,
    /// Page 0 is tried every `STREAM_POLL` until the coordinator has
    /// written it.
    Pending {
        cursor: PageCursor,
        names: Vec<String>,
        next_try: Instant,
    },
    /// The rows come with the POST as they always did: the URI was refused,
    /// or the pages went away while the statement ran.
    Abandoned,
    /// Page 0 was shown. The pages continue through `App::paging` while
    /// the reader wants them; these are the counts once that has ended.
    Started {
        shown: usize,
        total: Option<usize>,
        truncated: bool,
    },
}

/// A result with more pages on the coordinator: the editor waits until the
/// reader asks for the next page or stops.
struct Paging {
    cursor: PageCursor,
    names: Vec<String>,
    /// Rows rendered so far.
    shown: usize,
    /// The format the first page was rendered with (AUTO resolved), so
    /// every page reads the same.
    format: OutputFormat,
    /// The next page was asked for before the coordinator wrote it: when
    /// to ask again.
    waiting: Option<Instant>,
    /// A page narrowed a column; the summary says so.
    truncated: bool,
    /// The statement finished while its pages were being read: its
    /// summary, shown once the paging ends.
    summary: Option<render::summary::Summary>,
}

impl Paging {
    fn new(cursor: PageCursor, names: Vec<String>, shown: usize, format: OutputFormat) -> Paging {
        Paging {
            cursor,
            names,
            shown,
            format,
            waiting: None,
            truncated: false,
            summary: None,
        }
    }

    /// `1,000 of 84,312 rows · Space or Enter for more · q to stop`, or
    /// `1,000 rows so far · waiting for the next page… · q to stop` while
    /// the coordinator is still writing the page asked for.
    fn hint(&self) -> String {
        let shown = render::thousands(self.shown as i128);
        let count = match self.cursor.total_rows {
            Some(total) => format!("{shown} of {} rows", render::thousands(total as i128)),
            None => format!("{shown} rows so far"),
        };
        if self.waiting.is_some() {
            format!("{count} · waiting for the next page… · q to stop")
        } else {
            format!("{count} · Space or Enter for more · q to stop")
        }
    }

    /// Rows for the summary: the total once a page said the writer was
    /// complete, else what was shown.
    fn rows(&self) -> usize {
        self.cursor.total_rows.unwrap_or(self.shown)
    }
}

/// What the shell does with what the page cursor answered. While the
/// statement still runs its POST is the arbiter of failure: transport
/// trouble is retried, and a page gone (404, 410) means the statement
/// failed or was cancelled, which the POST reports.
#[derive(Debug)]
enum PageStep {
    Show(Page),
    /// Not written yet: ask again later.
    Wait {
        rows_so_far: usize,
    },
    /// Ask again later, nothing to say.
    Retry,
    /// Every page shown.
    Done,
    /// The pages went away while the statement runs.
    Gone,
    /// An error to show; the paging stops.
    Fail(CliHttp),
}

fn page_step(fetched: Result<Fetched, CliHttp>, running: bool) -> PageStep {
    match fetched {
        Ok(Fetched::Page(page)) => PageStep::Show(page),
        Ok(Fetched::NotYet { rows_so_far, .. }) => PageStep::Wait { rows_so_far },
        Ok(Fetched::Exhausted) => PageStep::Done,
        Err(failure) if running && matches!(failure.status, Some(404 | 410)) => PageStep::Gone,
        Err(failure)
            if running && failure.status.is_none() && failure.message != UNSAFE_NEXT_URI =>
        {
            PageStep::Retry
        }
        Err(failure) => PageStep::Fail(failure),
    }
}

/// Whether `finish` renders the rows itself: not once the stream showed
/// page 0, which is already on screen with whatever followed it.
fn renders_rows_on_finish(stream: &Stream) -> bool {
    !matches!(stream, Stream::Started { .. })
}

/// Rows for the summary of a statement whose pages were read while it ran:
/// what the live paging knows, else what the stream counted when the
/// paging ended (the total once a page said the writer was complete).
fn streamed_rows(stream: &Stream, paging: Option<&Paging>) -> usize {
    match (paging, stream) {
        (Some(paging), _) => paging.rows(),
        (None, Stream::Started { shown, total, .. }) => total.unwrap_or(*shown),
        (None, _) => 0,
    }
}

pub struct App {
    backend: Backend,
    editor: Editor,
    theme: Theme,
    cluster: Option<Cluster>,
    whoami: Option<Whoami>,
    last_elapsed_ms: Option<u64>,
    last_scanned_rows: Option<u64>,
    history_path: Option<std::path::PathBuf>,
    last_cluster_poll: Instant,
    running: Option<Running>,
    paging: Option<Paging>,
    watch: Option<Watch>,
    /// The submission ended with `\G`: its results use the vertical format.
    vertical_once: bool,
    /// Statements from one submission still to run, in order.
    pending: VecDeque<String>,
    settings: SessionSettings,
    names: NameCache,
    history_log: Vec<HistoryEntry>,
    /// `.timing`: whether the summary lines are shown.
    timing: bool,
    /// `.ask`: the previous answer's frame (a follow-up inherits its slots)
    /// and the clarification the user may answer by number.
    ask_frame: Option<serde_json::Value>,
    ask_clarify: Option<Clarification>,
}

/// A pending `.ask` clarification: the slot kind, its (id, label,
/// description) options, answered by number, and how to resume the
/// question once one is chosen.
type Clarification = (
    String,
    Vec<(String, String, String)>,
    crate::client::dlm::Resume,
);

impl App {
    fn new(backend: Backend, options: &Options) -> App {
        let history_path = (!options.no_history)
            .then(|| {
                options
                    .history_file
                    .clone()
                    .or_else(crate::input::default_history_file)
            })
            .flatten();
        let mut app = App {
            backend,
            editor: {
                let mut editor = Editor::new();
                editor.set_vi(options.editing_mode.eq_ignore_ascii_case("VI"));
                editor
            },
            theme: Theme::detect(&options.theme, true),
            cluster: None,
            whoami: None,
            last_elapsed_ms: None,
            last_scanned_rows: None,
            history_path,
            last_cluster_poll: Instant::now(),
            running: None,
            paging: None,
            watch: None,
            vertical_once: false,
            pending: VecDeque::new(),
            settings: SessionSettings::default(),
            names: NameCache::default(),
            history_log: Vec::new(),
            timing: true,
            ask_frame: None,
            ask_clarify: None,
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
        app
    }

    fn session(&self) -> Option<&SharedSession> {
        match &self.backend {
            Backend::Remote(session) => Some(session),
            Backend::Local(_) => None,
        }
    }

    fn engine(&self) -> Option<&SharedEngine> {
        match &self.backend {
            Backend::Remote(_) => None,
            Backend::Local(engine) => Some(engine),
        }
    }

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
            mode: self.editor.mode_label(),
        }
    }

    /// The format results of the current submission use: VERTICAL after
    /// `\G`, else the session's.
    fn result_format(&self, options: &Options) -> OutputFormat {
        if self.vertical_once {
            OutputFormat::Vertical
        } else {
            options.output_format
        }
    }

    fn insecure_development(&self, options: &Options) -> bool {
        self.whoami
            .as_ref()
            .is_some_and(|who| who.auth == "development")
            || (self.whoami.is_none() && options.auth == "none")
    }

    /// Rows the inline viewport needs: the editor, the status line, the
    /// running line above the editor while a statement runs, and the paging
    /// hint (or the watch line) under it while one is active.
    /// Nothing runs, pages or watches: keys go to the editor.
    fn is_idle(&self) -> bool {
        self.running.is_none() && self.paging.is_none() && self.watch.is_none()
    }

    fn viewport_rows(&self) -> u16 {
        self.editor.height(EDITOR_MAX_ROWS)
            + 1
            + u16::from(self.running.is_some())
            + u16::from(
                self.paging.is_some()
                    || self.watch.is_some()
                    || self.editor.search_view().is_some(),
            )
    }
}

/// The shell against a coordinator.
pub fn run(session: Session, options: &mut Options) -> Result<(), String> {
    let cluster = api::fetch_cluster(&session, &options.server).ok();
    let whoami = api::fetch_whoami(&session, &options.server).ok().flatten();
    let mut app = App::new(Backend::Remote(Arc::new(Mutex::new(session))), options);
    app.cluster = cluster;
    app.whoami = whoami;
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
                embedded: None,
            },
            &app.theme,
        );
        print!("{}", render::to_ansi(&header));
    }
    start(app, options, host_of(&options.server))
}

/// The same shell over the embedded engine (`--local` on a terminal): no
/// cluster, no query ids, no cancel; the session context comes from the
/// engine's catalog.
pub fn run_local(engine: LocalEngine, options: &mut Options) -> Result<(), String> {
    let (catalog, schema) = engine.context();
    options.catalog = catalog;
    options.schema = schema;
    options.context_explicit = true;
    let description = engine.description();
    let app = App::new(Backend::Local(Arc::new(Mutex::new(engine))), options);
    if !options.no_header {
        let header = render::cluster::header(
            &render::cluster::HeaderFacts {
                cli_version: env!("CARGO_PKG_VERSION"),
                server: &options.server,
                cluster: None,
                whoami: None,
                auth_mode: &options.auth,
                insecure_development: false,
                user: &options.user,
                now_unix: now_unix(),
                embedded: Some(description),
            },
            &app.theme,
        );
        print!("{}", render::to_ansi(&header));
    }
    start(app, options, "embedded".to_owned())
}

fn start(mut app: App, options: &mut Options, host: String) -> Result<(), String> {
    enable_raw_mode().map_err(|error| format!("cannot enter raw mode: {error}"))?;
    // Bracketed paste where the event reader understands it; the Windows
    // console reader does not, and a paste there arrives as a burst of keys
    // (see `is_paste_burst`).
    #[cfg(not(windows))]
    let _ = crossterm::execute!(io::stdout(), event::EnableBracketedPaste);
    let result = event_loop(&mut app, options, &host);
    #[cfg(not(windows))]
    let _ = crossterm::execute!(io::stdout(), event::DisableBracketedPaste);
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
fn emit(terminal: &mut Screen, lines: Vec<Line<'static>>) -> Result<(), String> {
    terminal.emit(lines)
}

fn emit_text(terminal: &mut Screen, text: &str, theme: &Theme, dim: bool) -> Result<(), String> {
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

/// Any error, as the panel. Messages the shell composed itself (a
/// resolved missing table, a refused limit) are already for people and
/// keep their wording under a "Not found" or "Shell" heading.
fn emit_error(terminal: &mut Screen, message: &str, theme: &Theme) -> Result<(), String> {
    let error = error_from_message(message, None);
    emit(terminal, render::error::panel(&error, theme))
}

fn error_from_message(message: &str, sql: Option<&str>) -> CliError {
    if message.starts_with("table '")
        || message.starts_with("no catalog.schema")
        || message.starts_with("catalog '")
        || message.starts_with("schema '")
    {
        return CliError {
            kind: ErrorKind::NotFound,
            message: message.to_owned(),
            query_id: None,
            workers: Vec::new(),
            position: None,
            sql: None,
        };
    }
    CliError::from_message(message, sql)
}

/// The embedded engine's errors, which it words as `SQL error: …`,
/// `Planning error: …` and `Execution error: …`.
fn local_error(message: &str, sql: Option<&str>) -> CliError {
    const PREFIXES: [(&str, ErrorKind); 3] = [
        ("SQL error: ", ErrorKind::Parse),
        ("Planning error: ", ErrorKind::Planning),
        ("Execution error: ", ErrorKind::Execution),
    ];
    for (prefix, kind) in PREFIXES {
        if let Some(rest) = message.strip_prefix(prefix) {
            let mut error = error_from_message(rest.trim(), sql);
            if error.kind == ErrorKind::Coordinator {
                error.kind = kind;
            }
            if error.sql.is_none() {
                error.sql = sql.map(str::to_owned);
            }
            return error;
        }
    }
    let mut error = error_from_message(message, sql);
    if error.kind == ErrorKind::Coordinator {
        error.kind = ErrorKind::Execution;
    }
    error
}

/// The width result tables may use: `--width`, else the terminal's.
fn table_width(options: &Options) -> Option<usize> {
    options.width.map(usize::from).or_else(|| {
        crossterm::terminal::size()
            .ok()
            .map(|(columns, _)| usize::from(columns))
    })
}

fn emit_blank(terminal: &mut Screen) -> Result<(), String> {
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

/// The running line for the embedded engine: elapsed time only, since
/// there is no record to poll and nothing to cancel.
fn local_running_line(elapsed: Duration, tick: usize, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!(
            " {} Running {:.1} s · embedded",
            progress::SPINNER[tick % progress::SPINNER.len()],
            elapsed.as_secs_f64()
        ),
        theme.accent,
    ))
}

fn event_loop(app: &mut App, options: &mut Options, host: &str) -> Result<(), String> {
    let mut screen = Screen::new(app.viewport_rows())?;
    loop {
        let terminal = &mut screen;
        let needed = app.viewport_rows();
        if needed != terminal.rows {
            terminal.resize(needed)?;
        }
        if let Some(error) = terminal.tee_error.take() {
            emit_error(terminal, &error, &app.theme)?;
        }
        if app.running.is_none()
            && app.paging.is_none()
            && app
                .watch
                .as_ref()
                .is_some_and(|watch| Instant::now() >= watch.next_run)
        {
            watch_tick(app, terminal, options)?;
            continue;
        }
        let context = options
            .context_explicit
            .then_some((options.catalog.as_str(), options.schema.as_str()));
        let running_line = app.running.as_ref().map(|running| {
            let elapsed = running.handle.started.elapsed();
            let tick = (elapsed.as_millis() / SPINNER_FRAME_MS) as usize;
            match app.backend {
                Backend::Remote(_) => progress::line(&running.progress, tick, &app.theme),
                Backend::Local(_) => local_running_line(elapsed, tick, &app.theme),
            }
        });
        let hint_line = match (&app.paging, &app.watch) {
            _ if app.editor.search_view().is_some() => app
                .editor
                .search_view()
                .map(|view| search_hint(&view, &app.theme)),
            (Some(paging), _) => Some(Line::styled(format!(" {}", paging.hint()), app.theme.dim)),
            (None, Some(watch)) => Some(Line::styled(
                format!(
                    " watch every {} s · run {} · any key to stop",
                    watch.interval.as_secs(),
                    watch.runs
                ),
                app.theme.dim,
            )),
            (None, None) => None,
        };
        let tee = terminal
            .tee
            .as_ref()
            .map(|tee| format!(" · tee {}", tee.path.display()));
        terminal
            .terminal
            .draw(|frame| {
                let editor_height = app.editor.height(EDITOR_MAX_ROWS);
                let [progress_area, editor_area, hint_area, status_area] = Layout::vertical([
                    Constraint::Length(u16::from(running_line.is_some())),
                    Constraint::Length(editor_height),
                    Constraint::Length(u16::from(hint_line.is_some())),
                    Constraint::Length(1),
                ])
                .areas(frame.area());
                if let Some(line) = running_line.clone() {
                    frame.render_widget(Paragraph::new(line), progress_area);
                }
                if let Some(line) = hint_line.clone() {
                    frame.render_widget(Paragraph::new(line), hint_area);
                }
                let [prompt_area, text_area] =
                    Layout::horizontal([Constraint::Length(prompt_width()), Constraint::Min(1)])
                        .areas(editor_area);
                let running_now =
                    running_line.is_some() || app.paging.is_some() || app.watch.is_some();
                if !running_now && app.editor.line_count() <= usize::from(EDITOR_MAX_LINES) {
                    // Highlighted text with the cursor placed by hand; the
                    // text area keeps the buffer and the cursor.
                    let block = Block::new()
                        .borders(Borders::TOP | Borders::BOTTOM)
                        .border_style(app.theme.dim);
                    let inner = block.inner(text_area);
                    let mut lines =
                        crate::shell::highlight::highlight(&app.editor.lines(), &app.theme);
                    if let Some(rest) = app.editor.suggestion()
                        && let Some(last) = lines.last_mut()
                    {
                        // The rest of the matching history entry, dimmed
                        // after the cursor; a later line of it is only hinted.
                        let mut shown = rest.lines().next().unwrap_or_default().to_owned();
                        if rest.contains('\n') {
                            shown.push_str(" …");
                        }
                        // Without colour the suggestion still has to read as
                        // a suggestion: the terminal's dim attribute.
                        let ghost = if app.theme.plain {
                            ratatui::style::Style::default()
                                .add_modifier(ratatui::style::Modifier::DIM)
                        } else {
                            app.theme.dim
                        };
                        last.spans.push(Span::styled(shown, ghost));
                    }
                    frame.render_widget(Paragraph::new(lines).block(block), text_area);
                    let (row, column) = app.editor.cursor();
                    let line = app.editor.current_line();
                    let prefix: String = line.chars().take(column).collect();
                    let x = inner.x + prefix.width() as u16;
                    let y = inner.y + row as u16;
                    if y < inner.bottom() {
                        frame.set_cursor_position((x.min(inner.right().saturating_sub(1)), y));
                    }
                } else {
                    frame.render_widget(app.editor.widget(&app.theme, running_now), text_area);
                }
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
                let mut status = status_line(&app.status_facts(context, host), &app.theme);
                if let Some(tee) = tee.clone() {
                    status.spans.push(Span::styled(tee, app.theme.dim));
                }
                frame.render_widget(Paragraph::new(status), status_area);
            })
            .map_err(|error| error.to_string())?;

        if app.running.is_some() {
            poll_running(app, terminal, options)?;
        }
        if app
            .paging
            .as_ref()
            .is_some_and(|paging| paging.waiting.is_some_and(|due| Instant::now() >= due))
        {
            next_page(app, terminal, options)?;
        }

        if !event::poll(Duration::from_millis(66)).map_err(|error| error.to_string())? {
            if app.last_cluster_poll.elapsed() >= CLUSTER_POLL {
                refresh_cluster(app, options);
            }
            continue;
        }
        let first = match event::read().map_err(|error| error.to_string())? {
            Event::Key(key) if key.kind == KeyEventKind::Press => key,
            Event::Paste(text) => {
                if app.is_idle() {
                    app.editor.paste(&text);
                }
                continue;
            }
            _ => continue,
        };
        // Keys that follow within a few milliseconds belong to the same
        // paste: a terminal that does not bracket a paste (the Windows
        // console) delivers its text as key records a moment apart, so the
        // gap that closes a burst is longer than the console's, shorter
        // than any keystroke.
        let mut batch = vec![first];
        while event::poll(PASTE_GAP).map_err(|error| error.to_string())? {
            if let Event::Key(key) = event::read().map_err(|error| error.to_string())?
                && key.kind == KeyEventKind::Press
            {
                batch.push(key);
            }
        }
        if app.is_idle() && is_paste_burst(&batch) {
            app.editor.paste(&burst_text(&batch));
            continue;
        }
        for key in batch {
            if let Flow::Quit = handle_key(app, terminal, options, key)? {
                return Ok(());
            }
        }
    }
}

enum Flow {
    Continue,
    Quit,
}

/// One key while the shell is idle, paging, watching or running.
fn handle_key(
    app: &mut App,
    terminal: &mut Screen,
    options: &mut Options,
    key: event::KeyEvent,
) -> Result<Flow, String> {
    {
        if app.watch.take().is_some() {
            // Any key ends the watch; a running statement finishes (or is
            // cancelled by Ctrl-C below) and nothing follows it.
            emit_text(terminal, "watch stopped", &app.theme, true)?;
            if app.running.is_none() {
                emit_blank(terminal)?;
                return Ok(Flow::Continue);
            }
        }
        if app.paging.is_some() {
            // Pages are read while the statement may still run: Ctrl-C then
            // cancels it (the failure ends the paging), otherwise it stops
            // the paging and drops what was pending.
            if is_ctrl_c(&key) {
                if app.running.is_some() {
                    interrupt_running(app, terminal, options)?;
                } else {
                    app.pending.clear();
                    stop_paging(app, terminal, options)?;
                }
            } else {
                match key.code {
                    KeyCode::Char(' ') | KeyCode::Enter => next_page(app, terminal, options)?,
                    KeyCode::Char('q') | KeyCode::Esc => stop_paging(app, terminal, options)?,
                    _ => {}
                }
            }
            return Ok(Flow::Continue);
        }
        if app.running.is_some() {
            // Only Ctrl-C means anything while a statement runs.
            if is_ctrl_c(&key) {
                interrupt_running(app, terminal, options)?;
            }
            return Ok(Flow::Continue);
        }
        if key.code == KeyCode::Tab {
            complete_at_cursor(app, terminal, options)?;
            return Ok(Flow::Continue);
        }
        // `\G` ends a statement like `;` does, which the editor does not
        // know; Enter on such a line submits here.
        let action = if key.code == KeyCode::Enter
            && key.modifiers.is_empty()
            && ends_with_vertical_marker(&app.editor.lines())
        {
            let text = app.editor.lines().trim().to_owned();
            app.editor.clear();
            EditorAction::Submit(text)
        } else {
            app.editor.handle(&key)
        };
        match action {
            EditorAction::None => {}
            EditorAction::Quit => {
                terminal
                    .terminal
                    .clear()
                    .map_err(|error| error.to_string())?;
                return Ok(Flow::Quit);
            }
            EditorAction::Clear => {
                terminal.reset(app.viewport_rows())?;
            }
            EditorAction::Interrupt => {
                if app.editor.is_empty() {
                    emit_text(terminal, "Ctrl-D or exit to leave", &app.theme, true)?;
                } else {
                    app.editor.clear();
                }
            }
            EditorAction::Submit(text) => {
                if !is_quit(&text) && !is_edit(&text) {
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
                    terminal
                        .terminal
                        .clear()
                        .map_err(|error| error.to_string())?;
                    return Ok(Flow::Quit);
                }
                let (text, vertical) = split_vertical_marker(&text);
                app.vertical_once = vertical;
                submit(app, terminal, options, &text)?;
            }
        }
    }
    Ok(Flow::Continue)
}

/// Keys that arrived together and read like text — characters, Enter, Tab
/// — are a paste from a terminal that does not bracket one: inserted as
/// they are, never run, never completed.
fn is_paste_burst(batch: &[event::KeyEvent]) -> bool {
    if batch.len() < 2 {
        return false;
    }
    // A bare line feed reaches the Windows console as Ctrl-Enter: inside a
    // burst that is a line break, not the force-submit key.
    let textual = batch.iter().all(|key| {
        matches!(key.code, KeyCode::Enter)
            || (!key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                && matches!(key.code, KeyCode::Char(_) | KeyCode::Tab))
    });
    let breaks = batch
        .iter()
        .any(|key| matches!(key.code, KeyCode::Enter | KeyCode::Tab));
    textual && (breaks || batch.len() >= 8)
}

fn burst_text(batch: &[event::KeyEvent]) -> String {
    batch
        .iter()
        .filter_map(|key| match key.code {
            KeyCode::Char(ch) => Some(ch),
            KeyCode::Enter => Some('\n'),
            KeyCode::Tab => Some('\t'),
            _ => None,
        })
        .collect()
}

/// The line under the editor while Ctrl-R searches the history.
fn search_hint(view: &crate::shell::editor::SearchView, theme: &Theme) -> Line<'static> {
    let mut spans = vec![
        Span::styled(" reverse search ", theme.dim),
        Span::styled(format!("‹{}›", view.query), theme.accent),
    ];
    if view.failing {
        spans.push(Span::styled(" no match", theme.warning));
    }
    spans.push(Span::styled(
        " · Ctrl-R older · Enter keep · Esc cancel",
        theme.dim,
    ));
    Line::from(spans)
}

/// `SELECT … \G`: the statement ends with the vertical marker instead of
/// `;`, outside any string, on the last non-blank line.
fn ends_with_vertical_marker(text: &str) -> bool {
    let trimmed = text.trim_end();
    let Some(body) = trimmed.strip_suffix("\\G") else {
        return false;
    };
    // The marker is not part of a string or identifier still open: the
    // tokenizer of everything before it must end cleanly.
    !body.trim().is_empty() && Tokenizer::new(&GenericDialect {}, body).tokenize().is_ok()
}

/// The text without a trailing `\G`, and whether there was one.
fn split_vertical_marker(text: &str) -> (String, bool) {
    if ends_with_vertical_marker(text) {
        let body = text.trim_end();
        (body[..body.len() - 2].trim_end().to_owned(), true)
    } else {
        (text.to_owned(), false)
    }
}

fn is_edit(text: &str) -> bool {
    text.trim().trim_end_matches(';').trim() == ".edit"
}

/// A `.watch` run: the screen cleared, one line saying what runs and how
/// often, then the statement through the normal submit path.
fn watch_tick(app: &mut App, terminal: &mut Screen, options: &mut Options) -> Result<(), String> {
    let Some(watch) = app.watch.as_mut() else {
        return Ok(());
    };
    watch.runs += 1;
    watch.next_run = Instant::now() + watch.interval;
    let statement = watch.statement.clone();
    let title = format!(
        " every {} s · {} · run {} at {} · any key to stop",
        watch.interval.as_secs(),
        statement.replace('\n', " "),
        watch.runs,
        seconds(watch.started.elapsed())
    );
    terminal.reset(app.viewport_rows())?;
    emit(terminal, vec![Line::styled(title, app.theme.dim)])?;
    emit_blank(terminal)?;
    let (statement, vertical) = split_vertical_marker(&statement);
    app.vertical_once = vertical;
    submit(app, terminal, options, &statement)
}

/// `.edit`: the editor's text, or the last statement, in `$VISUAL` /
/// `$EDITOR` (`notepad` on Windows, `vi` elsewhere, when neither is set);
/// the file comes back into the editor without running.
fn edit_in_editor(app: &mut App, terminal: &mut Screen) -> Result<(), String> {
    let text = if app.editor.is_empty() {
        app.editor.history().last().cloned().unwrap_or_default()
    } else {
        app.editor.lines()
    };
    let path = std::env::temp_dir().join(format!("kaveon-edit-{}.sql", std::process::id()));
    if let Err(error) = std::fs::write(&path, &text) {
        return emit_error(
            terminal,
            &format!("cannot write {}: {error}", path.display()),
            &app.theme,
        );
    }
    let editor = editor_command();
    // Leave the viewport and raw mode to the editor, then come back.
    terminal
        .terminal
        .clear()
        .map_err(|error| error.to_string())?;
    let _ = disable_raw_mode();
    let status = editor_process(&editor, &path).status();
    enable_raw_mode().map_err(|error| format!("cannot re-enter raw mode: {error}"))?;
    terminal.remake(app.viewport_rows())?;
    let outcome = match status {
        Ok(status) if status.success() => match std::fs::read_to_string(&path) {
            Ok(contents) => {
                app.editor.set_text(contents.trim_end());
                emit_text(
                    terminal,
                    &format!("loaded from {editor} · Enter to run"),
                    &app.theme,
                    true,
                )
            }
            Err(error) => emit_error(
                terminal,
                &format!("cannot read {} back: {error}", path.display()),
                &app.theme,
            ),
        },
        Ok(status) => emit_error(
            terminal,
            &format!("{editor} exited with {status}; the editor is unchanged"),
            &app.theme,
        ),
        Err(error) => emit_error(
            terminal,
            &format!("cannot start {editor}: {error}; set VISUAL or EDITOR"),
            &app.theme,
        ),
    };
    let _ = std::fs::remove_file(&path);
    outcome
}

/// `$VISUAL`, else `$EDITOR`, else the platform's editor.
fn editor_command() -> String {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .map(|value| value.trim().to_owned())
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| if cfg!(windows) { "notepad" } else { "vi" }.to_owned())
}

/// The editor as the platform shell runs it, so `code --wait` and a
/// quoted program path both work; the file is the last argument.
fn editor_process(editor: &str, path: &std::path::Path) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("cmd");
        command.raw_arg(format!("/C \"{editor} \"{}\"\"", path.display()));
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = std::process::Command::new("sh");
        command
            .arg("-c")
            .arg(format!("{editor} \"$1\""))
            .arg("kaveon")
            .arg(path);
        command
    }
}

/// Dot commands, `.limit`, `help` and `clear` run here and now; SQL is
/// split into statements and started through the worker thread.
fn submit(
    app: &mut App,
    terminal: &mut Screen,
    options: &mut Options,
    text: &str,
) -> Result<(), String> {
    if let Some(question) = text.strip_prefix(".ask") {
        ask(app, terminal, options, question.trim())?;
        return emit_blank(terminal);
    }
    match commands::parse(text) {
        Some(Ok(command)) => {
            run_command(app, terminal, options, command)?;
            return emit_blank(terminal);
        }
        Some(Err(error)) => {
            emit_error(terminal, &error, &app.theme)?;
            return emit_blank(terminal);
        }
        None => {}
    }
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
                    &crate::shell::rowlimit::report(options.row_limit),
                    &app.theme,
                    false,
                )?;
            }
            Err(error) => emit_error(terminal, &error, &app.theme)?,
        }
        return emit_blank(terminal);
    }
    if text.starts_with('.') {
        match &app.backend {
            Backend::Remote(session) => {
                let output = crate::remote::meta_command_to_string(&lock(session), options, text);
                match output {
                    Ok(output) => {
                        emit_text(terminal, output.trim_end_matches('\n'), &app.theme, false)?
                    }
                    Err(error) => emit_error(terminal, &error, &app.theme)?,
                }
                return emit_blank(terminal);
            }
            Backend::Local(_) => {
                return match local_dot_command_sql(text) {
                    Ok(sql) => {
                        if !run_local_metadata(app, terminal, options, &sql)? {
                            emit_error(terminal, "unknown command; type .help", &app.theme)?;
                            emit_blank(terminal)?;
                        }
                        Ok(())
                    }
                    Err(error) => {
                        emit_error(terminal, &error, &app.theme)?;
                        emit_blank(terminal)
                    }
                };
            }
        }
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

/// The metadata dot commands as the SQL the embedded catalog answers.
fn local_dot_command_sql(text: &str) -> Result<String, String> {
    let text = text.trim().trim_end_matches(';').trim();
    let parts: Vec<&str> = text.split_whitespace().collect();
    match parts.as_slice() {
        [".catalogs"] => Ok("SHOW CATALOGS".to_owned()),
        [".schemas"] => Ok("SHOW SCHEMAS".to_owned()),
        [".schemas", catalog] => Ok(format!("SHOW SCHEMAS IN {catalog}")),
        [".tables"] => Ok("SHOW TABLES".to_owned()),
        [".tables", target] => Ok(format!("SHOW TABLES IN {target}")),
        [".describe" | ".desc", table] => Ok(format!("DESCRIBE {table}")),
        [".use", target] => Ok(format!("USE {target}")),
        [".catalogs", ..] => Err("usage: .catalogs".to_owned()),
        [".schemas", ..] => Err("usage: .schemas [catalog]".to_owned()),
        [".tables", ..] => Err("usage: .tables [[catalog.]schema]".to_owned()),
        [".describe" | ".desc", ..] => Err("usage: .describe <table>".to_owned()),
        [".use", ..] => Err("usage: .use <catalog[.schema]>".to_owned()),
        [unknown, ..] => Err(format!(
            "unknown command '{unknown}'; type .help for commands"
        )),
        [] => Err("type .help for commands".to_owned()),
    }
}

/// Runs pending statements in order: SHOW, USE and DESCRIBE finish on this
/// thread; the first SQL statement goes to a worker thread and the rest
/// wait for it. An error drops what is left.
fn start_next(app: &mut App, terminal: &mut Screen, options: &mut Options) -> Result<(), String> {
    let outcome = start_pending(app, terminal, options);
    if app.running.is_none() && app.paging.is_none() {
        app.vertical_once = false;
    }
    outcome
}

fn start_pending(
    app: &mut App,
    terminal: &mut Screen,
    options: &mut Options,
) -> Result<(), String> {
    use crate::shell::rowlimit::{Limited, inspect};
    while let Some(statement) = app.pending.pop_front() {
        let (statement, explain) = match strip_explain(&statement) {
            Some((inner, explain)) => (inner, Some(explain)),
            None => (statement, None),
        };
        let (sql, preview_limit) = match inspect(&statement, options.row_limit) {
            Limited::Appended(sql) => (sql, options.row_limit),
            Limited::Explicit | Limited::Unchanged => (statement.clone(), None),
        };
        match &app.backend {
            Backend::Remote(session) => {
                let session = Arc::clone(session);
                let before = (options.catalog.clone(), options.schema.clone());
                let metadata =
                    crate::remote::run_metadata_statement(&lock(&session), options, &sql);
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
                        let mut request = StatementRequest::new(&sql, options);
                        let mut settings = app.settings.as_map().unwrap_or_default();
                        if explain.is_some() {
                            settings.insert("result_cache".into(), serde_json::Value::Bool(false));
                        }
                        request.settings = (!settings.is_empty()).then_some(settings);
                        let handle = statement::submit(
                            session,
                            options.server.clone(),
                            request,
                            options.timeout,
                        );
                        set_running(app, handle, statement, explain, preview_limit);
                        return Ok(());
                    }
                    Err(error) => {
                        app.pending.clear();
                        let message =
                            crate::remote::explain_missing_table(&lock(&session), options, &error)
                                .unwrap_or(error);
                        emit_error(terminal, &message, &app.theme)?;
                        return emit_blank(terminal);
                    }
                }
            }
            Backend::Local(engine) => {
                if explain.is_some() {
                    app.pending.clear();
                    emit_error(terminal, &format!("EXPLAIN is {EMBEDDED_ONLY}"), &app.theme)?;
                    return emit_blank(terminal);
                }
                let engine = Arc::clone(engine);
                if parse_catalog_command(&sql).is_some() {
                    if !run_local_metadata(app, terminal, options, &sql)? {
                        app.pending.clear();
                        return Ok(());
                    }
                    continue;
                }
                let handle = submit_local(engine, sql);
                set_running(app, handle, statement, None, preview_limit);
                return Ok(());
            }
        }
    }
    Ok(())
}

fn set_running(
    app: &mut App,
    handle: Handle,
    statement: String,
    explain: Option<Explain>,
    preview_limit: Option<usize>,
) {
    app.editor.set_text(&statement);
    app.running = Some(Running {
        handle,
        progress: Progress::default(),
        sql: statement,
        explain,
        preview_limit,
        polls: 0,
        last_poll: Instant::now(),
        cancel_requested: false,
        stream: Stream::Off,
    });
}

/// The embedded engine on a worker thread, reporting through the same
/// channel a coordinator statement does. The engine is `Send`, so the UI
/// thread keeps drawing the elapsed time while it plans and runs.
fn submit_local(engine: SharedEngine, sql: String) -> Handle {
    let (sender, events) = mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let outcome = lock_engine(&engine).execute(&sql);
        let event = match outcome {
            Ok(result) => StatementEvent::Finished(StatementResult {
                id: String::new(),
                state: "FINISHED".to_owned(),
                columns: result
                    .columns
                    .into_iter()
                    .map(|(name, data_type)| Column { name, data_type })
                    .collect(),
                data: result.rows,
                error: None,
                elapsed_ms: result.elapsed_ms,
                next_uri: None,
            }),
            Err(message) => StatementEvent::Failed(CliHttp::local(message)),
        };
        let _ = sender.send(event);
    });
    Handle {
        tag: String::new(),
        events,
        started,
    }
}

/// SHOW, DESCRIBE and USE answered by the embedded catalog on this thread,
/// rendered like any result. `Ok(false)` when `sql` is not one of them or
/// it failed (the error is already shown).
fn run_local_metadata(
    app: &mut App,
    terminal: &mut Screen,
    options: &mut Options,
    sql: &str,
) -> Result<bool, String> {
    let Some(command) = parse_catalog_command(sql) else {
        return Ok(false);
    };
    let Some(engine) = app.engine().map(Arc::clone) else {
        return Ok(false);
    };
    match command {
        CatalogCommand::Use { target } => {
            let outcome = lock_engine(&engine).use_context(&target);
            match outcome {
                Ok((catalog, schema)) => {
                    options.catalog = catalog;
                    options.schema = schema;
                    options.context_explicit = true;
                    app.names.invalidate();
                    emit(
                        terminal,
                        vec![Line::from(vec![
                            Span::styled(" ✓ ", app.theme.ok),
                            Span::raw(format!("session is {}.{}", options.catalog, options.schema)),
                        ])],
                    )?;
                }
                Err(error) => {
                    emit_error(terminal, &error, &app.theme)?;
                    emit_blank(terminal)?;
                    return Ok(false);
                }
            }
        }
        _ => {
            let outcome = lock_engine(&engine).execute(sql);
            match outcome {
                Ok(result) => {
                    let names: Vec<String> =
                        result.columns.into_iter().map(|(name, _)| name).collect();
                    let format = app.result_format(options);
                    render_rows(
                        app,
                        terminal,
                        options,
                        Some(sql),
                        &names,
                        &result.rows,
                        format,
                    )?;
                    if app.timing && is_human_format(format) {
                        let summary = render::summary::Summary {
                            ok: true,
                            elapsed_ms: result.elapsed_ms,
                            rows: result.rows.len(),
                            noun: "rows",
                            ..render::summary::Summary::default()
                        };
                        emit(terminal, render::summary::lines(&summary, &app.theme))?;
                    }
                    app.last_elapsed_ms = Some(result.elapsed_ms);
                    app.last_scanned_rows = None;
                }
                Err(error) => {
                    emit(
                        terminal,
                        render::error::panel(&local_error(&error, None), &app.theme),
                    )?;
                    emit_blank(terminal)?;
                    return Ok(false);
                }
            }
        }
    }
    emit_blank(terminal)?;
    Ok(true)
}

fn is_human_format(format: OutputFormat) -> bool {
    matches!(
        format,
        OutputFormat::Table
            | OutputFormat::Aligned
            | OutputFormat::Vertical
            | OutputFormat::Auto
            | OutputFormat::Markdown
    )
}

/// Every `STATE_POLL`: the record by tag until it is found, then by id.
/// Then whatever the worker thread has reported. The embedded engine has
/// no record; only the elapsed time moves.
fn poll_running(app: &mut App, terminal: &mut Screen, options: &mut Options) -> Result<(), String> {
    let Some(running) = app.running.as_mut() else {
        return Ok(());
    };
    let elapsed = running.handle.started.elapsed();
    running.progress.elapsed = elapsed;
    if let Backend::Remote(session) = &app.backend
        && running.last_poll.elapsed() >= STATE_POLL
    {
        running.last_poll = Instant::now();
        let record = match running.progress.query_id.clone() {
            Some(id) => api::fetch_query(&lock(session), &options.server, &id).ok(),
            None if running.polls < TAG_POLLS => {
                running.polls += 1;
                api::find_query_by_tag(&lock(session), &options.server, &running.handle.tag)
                    .ok()
                    .flatten()
            }
            None => None,
        };
        if let Some(record) = record {
            let mut progress = Progress::from_record(&record, elapsed);
            progress.rows_so_far = running.progress.rows_so_far;
            if running.cancel_requested {
                progress.phase = Phase::Cancelling;
            }
            if progress.phase == Phase::Queued {
                if app.last_cluster_poll.elapsed() >= QUEUED_CLUSTER_POLL {
                    refresh_cluster_fields(
                        session,
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
            // A paged statement's record offers its pages once planned:
            // read them from here on. EXPLAIN renders the plan, not rows,
            // and a watch shows its first page once the run is done.
            if matches!(running.stream, Stream::Off)
                && running.explain.is_none()
                && app.watch.is_none()
                && !record.columns.is_empty()
                && let Some(next_uri) = record.next_uri.as_deref()
            {
                running.stream = match PageCursor::new(&options.server, next_uri) {
                    Ok(cursor) => Stream::Pending {
                        cursor,
                        names: record
                            .columns
                            .iter()
                            .map(|column| column.name.clone())
                            .collect(),
                        next_try: Instant::now(),
                    },
                    Err(failure) => {
                        emit_error(terminal, &failure.message, &app.theme)?;
                        Stream::Abandoned
                    }
                };
            }
        }
    }
    let page_due = matches!(
        &running.stream,
        Stream::Pending { next_try, .. } if Instant::now() >= *next_try
    );
    if page_due {
        try_first_page(app, terminal, options)?;
    }
    let Some(running) = app.running.as_mut() else {
        return Ok(());
    };
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

/// Page 0 of the running statement, tried once: shown under the running
/// line when the coordinator has written it, with the paging taking over
/// from there; the running line's `rows so far` otherwise.
fn try_first_page(
    app: &mut App,
    terminal: &mut Screen,
    options: &mut Options,
) -> Result<(), String> {
    let Some(session) = app.session().map(Arc::clone) else {
        return Ok(());
    };
    let Some(running) = app.running.as_mut() else {
        return Ok(());
    };
    let Stream::Pending {
        cursor, next_try, ..
    } = &mut running.stream
    else {
        return Ok(());
    };
    let fetched = cursor.fetch_next_within(&lock(&session), STREAM_TIMEOUT);
    match page_step(fetched, true) {
        PageStep::Show(page) => {
            let started = Stream::Started {
                shown: page.rows.len(),
                total: cursor.total_rows,
                truncated: false,
            };
            let Stream::Pending { cursor, names, .. } =
                std::mem::replace(&mut running.stream, started)
            else {
                unreachable!("the stream was pending a moment ago");
            };
            running.progress.rows_so_far = Some(page.row_count);
            let statement = running.sql.clone();
            let format = app.result_format(options);
            let (format, cut) = render_rows(
                app,
                terminal,
                options,
                Some(&statement),
                &names,
                &page.rows,
                format,
            )?;
            if cursor.exhausted() {
                // Page 0 was the whole result; the summary follows the POST.
                if let Some(Running {
                    stream: Stream::Started { truncated, .. },
                    ..
                }) = app.running.as_mut()
                {
                    *truncated = cut;
                }
            } else {
                let mut paging = Paging::new(cursor, names, page.rows.len(), format);
                paging.truncated = cut;
                app.paging = Some(paging);
            }
            Ok(())
        }
        PageStep::Wait { rows_so_far } => {
            running.progress.rows_so_far = Some(rows_so_far);
            *next_try = Instant::now() + STREAM_POLL;
            Ok(())
        }
        PageStep::Retry => {
            *next_try = Instant::now() + STREAM_POLL;
            Ok(())
        }
        PageStep::Done | PageStep::Gone => {
            running.stream = Stream::Abandoned;
            Ok(())
        }
        PageStep::Fail(failure) => {
            running.stream = Stream::Abandoned;
            emit_error(terminal, &failure_message(&failure), &app.theme)
        }
    }
}

/// The worker thread reported: the result and its summary, or the error,
/// into scrollback; then the next pending statement.
fn finish(
    app: &mut App,
    terminal: &mut Screen,
    options: &mut Options,
    running: Running,
    event: StatementEvent,
) -> Result<(), String> {
    app.editor.clear();
    match event {
        StatementEvent::Finished(mut result) => {
            let names = result.column_names();
            let mut truncated = false;
            let mut rows = result.data.len();
            let mut paging = None;
            let streamed = !renders_rows_on_finish(&running.stream);
            if streamed {
                // Page 0 and whatever followed are on screen already; the
                // paging, when still live, keeps going and shows the
                // summary once it ends.
                rows = streamed_rows(&running.stream, app.paging.as_ref());
                if let (None, Stream::Started { truncated: cut, .. }) =
                    (app.paging.as_ref(), &running.stream)
                {
                    truncated = *cut;
                }
            } else if let Some(explain) = running.explain {
                let session = app.session().expect("EXPLAIN runs against a coordinator");
                let record = api::fetch_query(&lock(session), &options.server, &result.id).ok();
                let analyze = explain == Explain::Analyze;
                let plan = record
                    .as_ref()
                    .map(|record| render::plan::plan_of(record, analyze))
                    .unwrap_or(serde_json::Value::Null);
                emit(terminal, render::plan::tree(&plan, &app.theme))?;
                if analyze {
                    match record.as_ref() {
                        Some(record) => {
                            let width = table_width(options).map(|width| width.saturating_sub(2));
                            emit(terminal, render::plan::analyzed(record, width, &app.theme))?;
                        }
                        None => emit_text(
                            terminal,
                            "the query record could not be read; nothing to analyze",
                            &app.theme,
                            true,
                        )?,
                    }
                }
            } else {
                // Paged delivery: the response carries no rows, the first
                // page does. Any rows sent inline lead it.
                let mut cursor = match (result.next_uri.as_deref(), app.session()) {
                    (Some(next_uri), Some(_)) => match PageCursor::new(&options.server, next_uri) {
                        Ok(cursor) => Some(cursor),
                        Err(failure) => {
                            emit_error(terminal, &failure.message, &app.theme)?;
                            None
                        }
                    },
                    _ => None,
                };
                if let (Some(cursor), Some(session)) = (cursor.as_mut(), app.session()) {
                    // The statement is finished, so page 0 is written; a
                    // coordinator still saying otherwise leaves it to the
                    // paging, which asks again.
                    match cursor.fetch_next(&lock(session)) {
                        Ok(Fetched::Page(page)) => result.data.extend(page.rows),
                        Ok(Fetched::NotYet { .. } | Fetched::Exhausted) => {}
                        Err(failure) => {
                            emit_error(terminal, &failure_message(&failure), &app.theme)?
                        }
                    }
                }
                let (format, cut) = render_rows(
                    app,
                    terminal,
                    options,
                    Some(&running.sql),
                    &names,
                    &result.data,
                    app.result_format(options),
                )?;
                truncated = cut;
                rows = result.data.len();
                if let Some(cursor) = cursor {
                    if let Some(total) = cursor.total_rows {
                        rows = total.max(rows);
                    }
                    if !cursor.exhausted() {
                        paging = Some(Paging::new(
                            cursor,
                            names.clone(),
                            result.data.len(),
                            format,
                        ));
                    }
                }
            }
            let summary_format = app.result_format(options);
            let mut summary = match &app.backend {
                Backend::Remote(session) => crate::remote::statement_summary(
                    &lock(session),
                    options,
                    &result.id,
                    rows,
                    result.elapsed_ms,
                    running.preview_limit,
                ),
                Backend::Local(_) => is_human_format(summary_format).then(|| {
                    let mut summary = render::summary::Summary {
                        ok: true,
                        elapsed_ms: result.elapsed_ms,
                        rows,
                        noun: "rows",
                        ..render::summary::Summary::default()
                    };
                    if let Some(limit) = running.preview_limit
                        && rows >= limit
                    {
                        summary.message = Some(crate::shell::rowlimit::note(limit));
                    }
                    summary
                }),
            };
            if truncated && let Some(summary) = summary.as_mut() {
                add_truncation_note(summary);
            }
            app.last_elapsed_ms = Some(result.elapsed_ms);
            app.last_scanned_rows = summary.as_ref().and_then(|summary| summary.rows_scanned);
            app.history_log.push(HistoryEntry {
                statement: running.sql.clone(),
                elapsed_ms: Some(result.elapsed_ms),
                ok: true,
            });
            let timing = app.timing;
            if streamed && let Some(live) = app.paging.as_mut() {
                // The reader is still on the pages: the summary waits.
                live.summary = summary.filter(|_| timing);
                return Ok(());
            }
            if app.timing
                && let Some(summary) = summary
            {
                emit(terminal, render::summary::lines(&summary, &app.theme))?;
            }
            if let Some(paging) = paging {
                if app.watch.is_some() {
                    // A watch shows the first page each run; nothing waits.
                    let total = paging
                        .cursor
                        .total_rows
                        .map_or("more".to_owned(), |total| render::thousands(total as i128));
                    emit(
                        terminal,
                        vec![Line::styled(
                            format!(
                                "   {} of {total} rows · a watch shows the first page",
                                render::thousands(paging.shown as i128)
                            ),
                            app.theme.dim,
                        )],
                    )?;
                } else {
                    emit(
                        terminal,
                        vec![Line::styled(format!("   {}", paging.hint()), app.theme.dim)],
                    )?;
                    app.paging = Some(paging);
                    return Ok(());
                }
            }
            emit_blank(terminal)?;
            start_next(app, terminal, options)
        }
        StatementEvent::Failed(failure) => {
            app.pending.clear();
            // Pages read while it ran end here; the failure is the verdict.
            app.paging = None;
            if app.watch.take().is_some() {
                emit_text(terminal, "watch stopped", &app.theme, true)?;
            }
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
                let error = match &app.backend {
                    Backend::Remote(session) => {
                        let message = failure_message(&failure);
                        match crate::remote::explain_missing_table(
                            &lock(session),
                            options,
                            &message,
                        ) {
                            Some(resolved) => error_from_message(&resolved, None),
                            None => {
                                let mut error =
                                    CliError::from_message(&message, Some(&running.sql));
                                if error.query_id.is_none() {
                                    error.query_id = running.progress.query_id.clone();
                                }
                                error
                            }
                        }
                    }
                    Backend::Local(_) => local_error(&failure.message, Some(&running.sql)),
                };
                emit(terminal, render::error::panel(&error, &app.theme))?;
            }
            app.history_log.push(HistoryEntry {
                statement: running.sql.clone(),
                elapsed_ms: None,
                ok: false,
            });
            emit_blank(terminal)
        }
    }
}

/// One page of rows in `format`: the styled table for the table formats
/// (AUTO falls back to VERTICAL when even narrowed columns do not fit),
/// the plain renderer otherwise. Returns the format actually used and
/// whether the table narrowed a column. `statement` is the SQL the rows
/// answer, when known: `SHOW STATS FOR t` names its table from it.
fn render_rows(
    app: &App,
    terminal: &mut Screen,
    options: &Options,
    statement: Option<&str>,
    names: &[String],
    rows: &[Vec<serde_json::Value>],
    format: OutputFormat,
) -> Result<(OutputFormat, bool), String> {
    let boxed = matches!(
        format,
        OutputFormat::Table | OutputFormat::Aligned | OutputFormat::Auto
    );
    if boxed && let Some(text) = single_text_cell(names, rows) {
        // `SHOW CREATE TABLE` and the like: the statement itself, not a
        // one-cell table with escaped newlines.
        emit(
            terminal,
            crate::shell::highlight::highlight(&text, &app.theme),
        )?;
        return Ok((format, false));
    }
    if boxed && let Some(kind) = render::stats::kind(names) {
        // `SHOW STATS FOR` and `DESCRIBE DETAIL`: humanised, with the
        // table's totals over the columns.
        let lines = match kind {
            render::stats::Kind::Stats => {
                let table = statement.and_then(|sql| {
                    render::stats::table_reference(sql, &options.catalog, &options.schema)
                });
                render::stats::stats(
                    table.as_deref(),
                    names,
                    rows,
                    table_width(options),
                    &app.theme,
                )
            }
            render::stats::Kind::Detail => {
                render::stats::detail(names, rows, table_width(options), &app.theme)
            }
        };
        emit(terminal, lines)?;
        return Ok((format, false));
    }
    match format {
        OutputFormat::Table | OutputFormat::Aligned | OutputFormat::Auto => {
            let (lines, cut) = render::table::styled(names, rows, table_width(options), &app.theme);
            if cut && format == OutputFormat::Auto {
                let text = crate::output::format_rows(names, rows, OutputFormat::Vertical);
                emit_text(terminal, text.trim_end_matches('\n'), &app.theme, false)?;
                Ok((OutputFormat::Vertical, false))
            } else {
                emit(terminal, lines)?;
                Ok((format, cut))
            }
        }
        format => {
            let text = crate::output::format_rows(names, rows, format);
            emit_text(terminal, text.trim_end_matches('\n'), &app.theme, false)?;
            Ok((format, false))
        }
    }
}

/// Space or Enter while paging (or the retry after `waiting for the next
/// page…`): the next page, rendered with its header; the last page ends
/// the paging and runs whatever statement is pending. While the statement
/// still runs, a page the coordinator has not written yet is asked for
/// again every `STREAM_POLL` until it arrives or the reader stops.
fn next_page(app: &mut App, terminal: &mut Screen, options: &mut Options) -> Result<(), String> {
    let Some(session) = app.session().map(Arc::clone) else {
        return finish_paging(app, terminal, options);
    };
    let running = app.running.is_some();
    let Some(paging) = app.paging.as_mut() else {
        return Ok(());
    };
    paging.waiting = None;
    let timeout = if running {
        STREAM_TIMEOUT
    } else {
        METADATA_TIMEOUT
    };
    let fetched = paging.cursor.fetch_next_within(&lock(&session), timeout);
    match page_step(fetched, running) {
        PageStep::Show(page) => {
            let names = paging.names.clone();
            let format = paging.format;
            paging.shown += page.rows.len();
            let exhausted = paging.cursor.exhausted();
            let (_, cut) = render_rows(app, terminal, options, None, &names, &page.rows, format)?;
            if let Some(paging) = app.paging.as_mut() {
                paging.truncated |= cut;
            }
            if let Some(running) = app.running.as_mut() {
                running.progress.rows_so_far = Some(page.row_count);
            }
            if exhausted {
                finish_paging(app, terminal, options)
            } else {
                Ok(())
            }
        }
        PageStep::Wait { rows_so_far } => {
            paging.waiting = Some(Instant::now() + STREAM_POLL);
            if let Some(running) = app.running.as_mut() {
                running.progress.rows_so_far = Some(rows_so_far);
            }
            Ok(())
        }
        PageStep::Retry => {
            paging.waiting = Some(Instant::now() + STREAM_POLL);
            Ok(())
        }
        PageStep::Done => finish_paging(app, terminal, options),
        PageStep::Gone => {
            // The statement failed or was cancelled: its POST says so.
            if let (Some(paging), Some(running)) = (app.paging.take(), app.running.as_mut()) {
                running.stream = Stream::Started {
                    shown: paging.shown,
                    total: paging.cursor.total_rows,
                    truncated: paging.truncated,
                };
            }
            Ok(())
        }
        PageStep::Fail(failure) => {
            emit_error(terminal, &failure_message(&failure), &app.theme)?;
            stop_paging(app, terminal, options)
        }
    }
}

/// Every page shown: `all N rows shown`, then the next pending statement.
fn finish_paging(
    app: &mut App,
    terminal: &mut Screen,
    options: &mut Options,
) -> Result<(), String> {
    let Some(paging) = app.paging.take() else {
        return Ok(());
    };
    let note = format!("all {} rows shown", render::thousands(paging.shown as i128));
    end_paging(app, terminal, options, paging, &note)
}

/// `q`, Esc or Ctrl-C while paging: `stopped after N rows`. The remaining
/// pages stay on the coordinator until they expire.
fn stop_paging(app: &mut App, terminal: &mut Screen, options: &mut Options) -> Result<(), String> {
    let Some(paging) = app.paging.take() else {
        return Ok(());
    };
    let note = format!(
        "stopped after {} rows",
        render::thousands(paging.shown as i128)
    );
    end_paging(app, terminal, options, paging, &note)
}

/// The paging's closing line, then what follows it: the statement's
/// summary when it finished while its pages were read, and the next
/// pending statement. A statement still running keeps the counts and does
/// both once its POST returns.
fn end_paging(
    app: &mut App,
    terminal: &mut Screen,
    options: &mut Options,
    paging: Paging,
    note: &str,
) -> Result<(), String> {
    emit(
        terminal,
        vec![Line::styled(format!("   {note}"), app.theme.dim)],
    )?;
    if let Some(running) = app.running.as_mut() {
        running.stream = Stream::Started {
            shown: paging.shown,
            total: paging.cursor.total_rows,
            truncated: paging.truncated,
        };
        return Ok(());
    }
    let rows = paging.rows();
    if let Some(mut summary) = paging.summary {
        summary.rows = rows;
        if paging.truncated {
            add_truncation_note(&mut summary);
        }
        emit(terminal, render::summary::lines(&summary, &app.theme))?;
    }
    emit_blank(terminal)?;
    start_next(app, terminal, options)
}

fn add_truncation_note(summary: &mut render::summary::Summary) {
    let note = "some columns truncated · .format VERTICAL to see them whole";
    summary.message = Some(match summary.message.take() {
        Some(existing) => format!("{existing} · {note}"),
        None => note.to_owned(),
    });
}

/// Ctrl-C while a statement runs: cancel it on the coordinator when its
/// id is known; a second press, or no id to cancel, abandons the wait.
/// The embedded engine cannot be interrupted; the statement runs on.
fn interrupt_running(
    app: &mut App,
    terminal: &mut Screen,
    options: &mut Options,
) -> Result<(), String> {
    let Some(running) = app.running.as_mut() else {
        return Ok(());
    };
    let Backend::Remote(session) = &app.backend else {
        return emit_text(
            terminal,
            "embedded statements cannot be cancelled; waiting for it to finish",
            &app.theme,
            true,
        );
    };
    if !running.cancel_requested {
        if running.progress.query_id.is_none() {
            // One more look before giving up on a cancel: the record may
            // have appeared since the last poll.
            let found =
                api::find_query_by_tag(&lock(session), &options.server, &running.handle.tag)
                    .ok()
                    .flatten();
            if let Some(record) = found {
                running.progress = Progress::from_record(&record, running.handle.started.elapsed());
            }
        }
        if let Some(id) = running.progress.query_id.clone() {
            let _ = api::cancel_query(&lock(session), &options.server, &id);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Explain {
    /// The plan.
    Plan,
    /// The optimized plan and the run's cost.
    Analyze,
}

/// `EXPLAIN [ANALYZE] <statement>` → the statement and which, when the
/// first word is EXPLAIN.
pub(crate) fn strip_explain(statement: &str) -> Option<(String, Explain)> {
    let trimmed = statement.trim_start();
    let mut words = trimmed.splitn(2, char::is_whitespace);
    let first = words.next()?;
    if !first.eq_ignore_ascii_case("EXPLAIN") {
        return None;
    }
    let rest = words.next()?.trim();
    let mut words = rest.splitn(2, char::is_whitespace);
    if words
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("ANALYZE"))
    {
        let rest = words.next()?.trim();
        return (!rest.is_empty()).then(|| (rest.to_owned(), Explain::Analyze));
    }
    (!rest.is_empty()).then(|| (rest.to_owned(), Explain::Plan))
}

/// Tab: complete the word before the cursor from keywords and the
/// catalog's names; several candidates are listed above the editor.
fn complete_at_cursor(
    app: &mut App,
    terminal: &mut Screen,
    options: &Options,
) -> Result<(), String> {
    let line = app.editor.current_line();
    let (_, column) = app.editor.cursor();
    let byte_cursor = line
        .char_indices()
        .nth(column)
        .map_or(line.len(), |(index, _)| index);
    let head = &line[..byte_cursor];
    let word_start = head
        .rfind(|c: char| c.is_whitespace() || c == '(' || c == ',')
        .map_or(0, |index| index + 1);
    let prefix = head[word_start..]
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_owned();
    let names = if prefix.is_empty() {
        Vec::new()
    } else {
        match &app.backend {
            Backend::Remote(session) => app.names.candidates(
                &lock(session),
                &options.server,
                &options.catalog,
                &options.schema,
                &prefix,
            ),
            Backend::Local(engine) => local_candidates(
                &lock_engine(engine),
                &options.catalog,
                &options.schema,
                &prefix,
            ),
        }
    };
    let (from, candidates) = crate::shell::complete::complete(&line, byte_cursor, &names);
    let typed_chars = line[from..byte_cursor].chars().count();
    match candidates.as_slice() {
        [] => Ok(()),
        [only] => {
            app.editor.replace_before_cursor(typed_chars, only);
            Ok(())
        }
        many => {
            let common = crate::shell::complete::common_prefix(many);
            if common.chars().count() > typed_chars {
                app.editor.replace_before_cursor(typed_chars, &common);
            }
            let shown: Vec<&str> = many.iter().take(12).map(String::as_str).collect();
            let more = many.len().saturating_sub(shown.len());
            let mut text = format!("  {}", shown.join("  "));
            if more > 0 {
                text.push_str(&format!("  … {more} more"));
            }
            emit(terminal, vec![Line::styled(text, app.theme.dim)])
        }
    }
}

/// The embedded catalog's names that start with `prefix`, in the order
/// `NameCache::candidates` offers them: tables and columns of the session's
/// schema, then its catalog's schemas, then the catalogs.
fn local_candidates(
    engine: &LocalEngine,
    catalog: &str,
    schema: &str,
    prefix: &str,
) -> Vec<String> {
    let prefix = prefix.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    let mut push = |names: Vec<String>| {
        let mut matching: Vec<String> = names
            .into_iter()
            .filter(|name| name.to_lowercase().starts_with(&prefix))
            .filter(|name| !out.iter().any(|seen| seen == name))
            .collect();
        matching.sort_unstable();
        matching.dedup();
        out.extend(matching);
    };
    let tables = engine.tables(catalog, schema).unwrap_or_default();
    let columns: Vec<String> = tables
        .iter()
        .filter_map(|table| engine.describe(&format!("{catalog}.{schema}.{table}")).ok())
        .flatten()
        .map(|(name, _, _)| name)
        .collect();
    push(tables);
    push(columns);
    push(engine.schemas(catalog).unwrap_or_default());
    push(engine.catalogs());
    out
}

fn run_command(
    app: &mut App,
    terminal: &mut Screen,
    options: &mut Options,
    command: Command,
) -> Result<(), String> {
    let coordinator_only = |terminal: &mut Screen, what: &str, theme: &Theme| {
        emit_text(terminal, &format!("{what} is {EMBEDDED_ONLY}"), theme, true)
    };
    match command {
        Command::Cluster => {
            if app.session().is_none() {
                return coordinator_only(terminal, ".cluster", &app.theme);
            }
            refresh_cluster(app, options);
            match &app.cluster {
                Some(cluster) => emit(
                    terminal,
                    render::cluster::panel(cluster, now_unix(), &app.theme),
                ),
                None => emit_error(terminal, "the cluster payload is unavailable", &app.theme),
            }
        }
        Command::Settings(_) | Command::SettingsReset if app.session().is_none() => {
            coordinator_only(terminal, ".settings", &app.theme)
        }
        Command::Settings(None) => {
            if app.settings.is_empty() {
                emit_text(
                    terminal,
                    "no session settings; .settings <key> <value> with memory, parallelism, cache, admission_wait",
                    &app.theme,
                    true,
                )
            } else {
                emit(terminal, app.settings.lines(&app.theme))
            }
        }
        Command::Settings(Some((key, value))) => match app.settings.set(&key, &value) {
            Ok(()) => emit(terminal, app.settings.lines(&app.theme)),
            Err(error) => emit_error(terminal, &error, &app.theme),
        },
        Command::SettingsReset => {
            app.settings.reset();
            emit_text(terminal, "session settings cleared", &app.theme, true)
        }
        Command::Format(name) => match crate::output::OutputFormat::parse(&name) {
            Ok(format) => {
                options.output_format = format;
                emit_text(terminal, &format!("result format {name}"), &app.theme, true)
            }
            Err(error) => emit_error(terminal, &error, &app.theme),
        },
        Command::History(count) => {
            let entries: Vec<&HistoryEntry> =
                app.history_log.iter().rev().take(count).collect::<Vec<_>>();
            if entries.is_empty() {
                return emit_text(terminal, "no statements yet", &app.theme, true);
            }
            let lines = entries
                .into_iter()
                .rev()
                .map(|entry| {
                    let (glyph, style) = if entry.ok {
                        ("✓", app.theme.ok)
                    } else {
                        ("✗", app.theme.error)
                    };
                    let when = entry.elapsed_ms.map_or("      —".to_owned(), |ms| {
                        format!("{:>7}", seconds(Duration::from_millis(ms)))
                    });
                    let statement = entry.statement.replace('\n', " ");
                    let statement: String = statement.chars().take(100).collect();
                    Line::from(vec![
                        Span::styled(format!(" {glyph} "), style),
                        Span::styled(format!("{when}  "), app.theme.dim),
                        Span::raw(statement),
                    ])
                })
                .collect();
            emit(terminal, lines)
        }
        Command::Queries => {
            let Some(session) = app.session() else {
                return coordinator_only(terminal, ".queries", &app.theme);
            };
            let records: Vec<api::QueryRecord> =
                api::get(&lock(session), &api::endpoint(&options.server, "/v1/query"))
                    .map_err(|failure| failure.message)?;
            let live: Vec<&api::QueryRecord> = records
                .iter()
                .filter(|record| matches!(record.state.as_str(), "QUEUED" | "RUNNING"))
                .collect();
            if live.is_empty() {
                return emit_text(
                    terminal,
                    "no statements running on the coordinator",
                    &app.theme,
                    true,
                );
            }
            let lines = live
                .into_iter()
                .map(|record| {
                    Line::from(vec![
                        Span::styled(
                            format!(" {}  ", &record.id[..record.id.len().min(8)]),
                            app.theme.accent,
                        ),
                        Span::styled(format!("{:<8} ", record.state), app.theme.dim),
                        Span::styled(
                            format!("{:>8}  ", seconds(Duration::from_millis(record.elapsed_ms))),
                            app.theme.dim,
                        ),
                        Span::raw(
                            record
                                .context
                                .client_tags
                                .iter()
                                .find(|t| !t.starts_with("kaveon-cli:"))
                                .cloned()
                                .unwrap_or_default(),
                        ),
                    ])
                })
                .collect();
            emit(terminal, lines)
        }
        Command::Kill(id) => {
            let Some(session) = app.session() else {
                return coordinator_only(terminal, ".kill", &app.theme);
            };
            match api::cancel_query(&lock(session), &options.server, &id) {
                Ok(()) => emit_text(
                    terminal,
                    &format!("cancel requested for {id}"),
                    &app.theme,
                    true,
                ),
                Err(failure) => emit_error(terminal, &failure.message, &app.theme),
            }
        }
        Command::Timing => {
            app.timing = !app.timing;
            emit_text(
                terminal,
                if app.timing {
                    "summary on"
                } else {
                    "summary off"
                },
                &app.theme,
                true,
            )
        }
        Command::Source(path) => match std::fs::read_to_string(&path) {
            Ok(text) => {
                emit_text(
                    terminal,
                    &format!("source {}", path.display()),
                    &app.theme,
                    true,
                )?;
                submit(app, terminal, options, &text)
            }
            Err(error) => emit_error(
                terminal,
                &format!("cannot read {}: {error}", path.display()),
                &app.theme,
            ),
        },
        Command::Tee(Some(path)) => {
            match std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&path)
            {
                Ok(file) => {
                    terminal.tee = Some(Tee {
                        path: path.clone(),
                        file,
                    });
                    emit_text(
                        terminal,
                        &format!(
                            "tee {} · everything shown is appended there",
                            path.display()
                        ),
                        &app.theme,
                        true,
                    )
                }
                Err(error) => emit_error(
                    terminal,
                    &format!("cannot open {}: {error}", path.display()),
                    &app.theme,
                ),
            }
        }
        Command::Tee(None) => {
            let message = match terminal.tee.take() {
                Some(tee) => format!("tee off · {}", tee.path.display()),
                None => "tee is off".to_owned(),
            };
            emit_text(terminal, &message, &app.theme, true)
        }
        Command::Edit => edit_in_editor(app, terminal),
        Command::Watch {
            interval,
            statement,
        } => {
            app.watch = Some(Watch {
                statement,
                interval,
                next_run: Instant::now(),
                started: Instant::now(),
                runs: 0,
            });
            Ok(())
        }
        Command::Help => emit(terminal, render::help::help(&app.theme)),
        Command::Clear => terminal.reset(app.viewport_rows()),
        Command::Quit => Ok(()),
    }
}

/// `.ask <question>`: the platform's DLM answers in plain language — from
/// its precomputed context, or with SQL the shell then runs on the
/// coordinator when the dataset is a native catalog. `.ask <n>` answers a
/// clarification; a follow-up inherits the previous answer's frame.
fn ask(
    app: &mut App,
    terminal: &mut Screen,
    options: &mut Options,
    question: &str,
) -> Result<(), String> {
    use crate::client::dlm::{AskAnswer, DlmClient};
    if question.is_empty() {
        return emit_text(
            terminal,
            ".ask <question> — a question in plain language, answered through the Kaveon DLM",
            &app.theme,
            true,
        );
    }
    let Some(api_url) = options.api_url.clone() else {
        return emit_error(
            terminal,
            ".ask needs the Kaveon platform API: start with --api <url> (or set KAVEON_API_URL); the DLM runs there, not on the coordinator",
            &app.theme,
        );
    };
    let token = std::env::var("KAVEON_API_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    let client = DlmClient::new(&api_url, token, Duration::from_secs(60))
        .map_err(|failure| failure.message)?;
    // A number answers the pending clarification.
    let mut choices = None;
    let mut asked = question.to_owned();
    if let Ok(number) = question.parse::<usize>()
        && let Some((kind, options_list, resume)) = app.ask_clarify.take()
    {
        match options_list.get(number.wrapping_sub(1)) {
            Some((id, _, _)) => {
                // The original question goes back with the choice pinned
                // on top of what was already chosen; a label alone would
                // be read as a new question inside the last answer's frame.
                let mut map = resume.choices.clone();
                map.insert(kind, serde_json::Value::String(id.clone()));
                choices = Some(map);
                asked = if resume.question.trim().is_empty() {
                    options_list[number - 1].1.clone()
                } else {
                    resume.question.clone()
                };
            }
            None => {
                app.ask_clarify = Some((kind, options_list, resume));
                return emit_error(
                    terminal,
                    &format!("choose a number from 1 to {}", number.max(1)),
                    &app.theme,
                );
            }
        }
    }
    let answer = client
        .ask(&asked, 50, choices.as_ref(), app.ask_frame.as_ref())
        .map_err(|failure| failure.message)?;
    emit(terminal, render::ask::answer_lines(&answer, &app.theme))?;
    match answer {
        AskAnswer::Live {
            catalog,
            schema,
            sql,
            engine,
            frame,
            ..
        } => {
            app.ask_frame = frame;
            app.ask_clarify = None;
            if engine {
                if (options.catalog.as_str(), options.schema.as_str())
                    != (catalog.as_str(), schema.as_str())
                {
                    options.catalog = catalog;
                    options.schema = schema;
                    options.context_explicit = true;
                    emit_text(
                        terminal,
                        &format!("session is now {}.{}", options.catalog, options.schema),
                        &app.theme,
                        true,
                    )?;
                }
                app.pending.push_back(sql);
                start_next(app, terminal, options)?;
            }
            Ok(())
        }
        AskAnswer::Context { frame, .. } => {
            app.ask_frame = frame;
            app.ask_clarify = None;
            Ok(())
        }
        AskAnswer::Clarify {
            kind,
            options: choices,
            frame,
            resume,
            ..
        } => {
            // The clarified question is a fresh one: the last answer's
            // frame must not colour it.
            app.ask_frame = frame;
            app.ask_clarify = Some((kind, choices, resume));
            Ok(())
        }
        AskAnswer::OutOfScope { .. } | AskAnswer::Refused { .. } => Ok(()),
    }
}

/// A one-column, one-row result whose value spans lines: shown as text.
fn single_text_cell(names: &[String], rows: &[Vec<serde_json::Value>]) -> Option<String> {
    if names.len() != 1 || rows.len() != 1 {
        return None;
    }
    let value = rows[0].first()?.as_str()?;
    value.contains('\n').then(|| value.to_owned())
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

    fn cursor(page: usize) -> PageCursor {
        PageCursor::new(
            "http://127.0.0.1:8080",
            &format!("/v1/query/q/results/{page}"),
        )
        .unwrap()
    }

    fn page(rows: usize, row_count: usize, complete: bool) -> Page {
        Page {
            rows: (0..rows).map(|n| vec![serde_json::json!(n)]).collect(),
            next_uri: Some("/v1/query/q/results/1".into()),
            index: 0,
            row_count,
            complete,
        }
    }

    fn http(status: u16) -> CliHttp {
        CliHttp {
            status: Some(status),
            code: None,
            message: format!("HTTP {status}"),
            timed_out: false,
            connect: false,
        }
    }

    #[test]
    fn paging_hint_counts_rows_shown_against_the_total() {
        let mut paging = Paging::new(cursor(1), vec!["n".into()], 1_000, OutputFormat::Table);
        assert_eq!(
            paging.hint(),
            "1,000 rows so far · Space or Enter for more · q to stop"
        );
        assert_eq!(paging.rows(), 1_000);
        paging.waiting = Some(Instant::now());
        assert_eq!(
            paging.hint(),
            "1,000 rows so far · waiting for the next page… · q to stop"
        );
        paging.waiting = None;
        paging.cursor.total_rows = Some(84_312);
        assert_eq!(
            paging.hint(),
            "1,000 of 84,312 rows · Space or Enter for more · q to stop"
        );
        assert_eq!(paging.rows(), 84_312);
    }

    #[test]
    fn a_page_answer_is_shown_waited_for_retried_or_given_up_on() {
        // What the cursor found, regardless of the statement's state.
        assert!(matches!(
            page_step(Ok(Fetched::Page(page(3, 3, true))), true),
            PageStep::Show(page) if page.rows.len() == 3 && page.complete
        ));
        assert!(matches!(
            page_step(
                Ok(Fetched::NotYet {
                    retry_after: Duration::from_secs(1),
                    rows_so_far: 12_000
                }),
                true
            ),
            PageStep::Wait {
                rows_so_far: 12_000
            }
        ));
        assert!(matches!(
            page_step(Ok(Fetched::Exhausted), false),
            PageStep::Done
        ));
        // While the statement runs its POST is the arbiter: a page gone is
        // the failure the POST will report, transport trouble is retried.
        for status in [404, 410] {
            assert!(matches!(page_step(Err(http(status)), true), PageStep::Gone));
            assert!(matches!(
                page_step(Err(http(status)), false),
                PageStep::Fail(failure) if failure.status == Some(status)
            ));
        }
        let timed_out = CliHttp {
            timed_out: true,
            ..CliHttp::local("operation timed out")
        };
        assert!(matches!(
            page_step(Err(timed_out.clone()), true),
            PageStep::Retry
        ));
        assert!(matches!(
            page_step(Err(timed_out), false),
            PageStep::Fail(failure) if failure.timed_out
        ));
        // An unsafe next URI is never retried: the cursor is exhausted.
        assert!(matches!(
            page_step(Err(CliHttp::local(UNSAFE_NEXT_URI)), true),
            PageStep::Fail(failure) if failure.message == UNSAFE_NEXT_URI
        ));
        assert!(matches!(
            page_step(Err(http(500)), true),
            PageStep::Fail(failure) if failure.status == Some(500)
        ));
    }

    #[test]
    fn a_finished_statement_does_not_render_page_zero_again() {
        assert!(renders_rows_on_finish(&Stream::Off));
        assert!(renders_rows_on_finish(&Stream::Abandoned));
        assert!(renders_rows_on_finish(&Stream::Pending {
            cursor: cursor(0),
            names: vec!["n".into()],
            next_try: Instant::now(),
        }));
        assert!(!renders_rows_on_finish(&Stream::Started {
            shown: 1_000,
            total: None,
            truncated: false,
        }));
    }

    #[test]
    fn the_streamed_summary_counts_the_total_once_known_else_what_was_shown() {
        // The reader is still on the pages: the live paging knows best.
        let mut live = Paging::new(cursor(1), vec!["n".into()], 2_000, OutputFormat::Table);
        let started = Stream::Started {
            shown: 1_000,
            total: None,
            truncated: false,
        };
        assert_eq!(streamed_rows(&started, Some(&live)), 2_000);
        live.cursor.total_rows = Some(84_312);
        assert_eq!(streamed_rows(&started, Some(&live)), 84_312);
        // The reader stopped before the POST returned: what the stream kept.
        assert_eq!(streamed_rows(&started, None), 1_000);
        let complete = Stream::Started {
            shown: 3_000,
            total: Some(84_312),
            truncated: false,
        };
        assert_eq!(streamed_rows(&complete, None), 84_312);
        assert_eq!(streamed_rows(&Stream::Off, None), 0);
    }

    #[test]
    fn a_page_not_written_yet_updates_the_running_line_and_waits() {
        // The 202 carries the writer's count; the paging asks again after
        // STREAM_POLL and says so on its hint line.
        let mut paging = Paging::new(cursor(1), vec!["n".into()], 1_000, OutputFormat::Table);
        let mut progress = Progress::default();
        let step = page_step(
            Ok(Fetched::NotYet {
                retry_after: Duration::from_secs(1),
                rows_so_far: 1_500,
            }),
            true,
        );
        if let PageStep::Wait { rows_so_far } = step {
            paging.waiting = Some(Instant::now() + STREAM_POLL);
            progress.rows_so_far = Some(rows_so_far);
        }
        assert!(paging.hint().contains("waiting for the next page…"));
        assert_eq!(progress.rows_so_far, Some(1_500));
        assert!(
            paging
                .waiting
                .is_some_and(|due| due > Instant::now() && due <= Instant::now() + STREAM_POLL)
        );
        let text = render::to_plain(&[progress::line(
            &Progress {
                phase: Phase::Running,
                rows_so_far: Some(1_500),
                ..Progress::default()
            },
            0,
            &Theme::mono(),
        )]);
        assert!(text.contains("1,500 rows so far"), "{text}");
    }

    #[test]
    fn a_trailing_vertical_marker_ends_a_statement() {
        assert!(ends_with_vertical_marker("SELECT 1 \\G"));
        assert!(ends_with_vertical_marker("SELECT *\nFROM t\\G\n"));
        assert!(!ends_with_vertical_marker("SELECT 1;"));
        assert!(!ends_with_vertical_marker("\\G"));
        assert!(!ends_with_vertical_marker("SELECT '\\G"));
        assert_eq!(
            split_vertical_marker("SELECT 1 \\G"),
            ("SELECT 1".to_owned(), true)
        );
        assert_eq!(
            split_vertical_marker("SELECT 1;"),
            ("SELECT 1;".to_owned(), false)
        );
        assert!(is_edit(".edit"));
        assert!(is_edit(" .edit; "));
        assert!(!is_edit(".editor"));
    }

    #[test]
    fn the_editor_command_prefers_visual_then_editor() {
        let command = editor_command();
        let expected = std::env::var("VISUAL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| std::env::var("EDITOR").ok())
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.trim().to_owned())
            .unwrap_or_else(|| if cfg!(windows) { "notepad" } else { "vi" }.to_owned());
        assert_eq!(command, expected);
        let process = editor_process("myeditor --wait", std::path::Path::new("x y.sql"));
        let program = process.get_program().to_string_lossy().into_owned();
        assert!(program == "cmd" || program == "sh", "{program}");
    }

    #[test]
    fn quit_words() {
        assert!(is_quit("exit"));
        assert!(is_quit("QUIT;"));
        assert!(is_quit(".q"));
        assert!(!is_quit("SELECT 1"));
    }

    #[test]
    fn embedded_errors_keep_their_kind() {
        let parse = local_error("SQL error: Expected end of statement", Some("SELEC 1"));
        assert_eq!(parse.kind, ErrorKind::Parse);
        assert_eq!(parse.message, "Expected end of statement");
        assert_eq!(parse.sql.as_deref(), Some("SELEC 1"));
        let planning = local_error(
            "Planning error: table 'kaveon.default.nowhere' not found",
            None,
        );
        assert_eq!(planning.kind, ErrorKind::NotFound);
        let execution = local_error("Execution error: division by zero", None);
        assert_eq!(execution.kind, ErrorKind::Execution);
        assert_eq!(execution.message, "division by zero");
    }

    #[test]
    fn dot_commands_map_to_the_embedded_catalog_statements() {
        assert_eq!(local_dot_command_sql(".catalogs").unwrap(), "SHOW CATALOGS");
        assert_eq!(
            local_dot_command_sql(".schemas lake;").unwrap(),
            "SHOW SCHEMAS IN lake"
        );
        assert_eq!(
            local_dot_command_sql(".tables lake.gold").unwrap(),
            "SHOW TABLES IN lake.gold"
        );
        assert_eq!(
            local_dot_command_sql(".desc events").unwrap(),
            "DESCRIBE events"
        );
        assert_eq!(local_dot_command_sql(".use lake").unwrap(), "USE lake");
        assert!(
            local_dot_command_sql(".describe")
                .unwrap_err()
                .starts_with("usage")
        );
        assert!(
            local_dot_command_sql(".nope")
                .unwrap_err()
                .contains("unknown command")
        );
    }

    #[test]
    fn embedded_completion_offers_tables_columns_schemas_and_catalogs() {
        let dir = crate::local::catalog::tests::parquet_fixture();
        let engine = LocalEngine::from_data_dir(&dir).unwrap();
        assert_eq!(
            local_candidates(&engine, "kaveon", "default", "e"),
            vec!["events".to_owned()]
        );
        assert_eq!(
            local_candidates(&engine, "kaveon", "default", "k"),
            vec!["kind".to_owned(), "kaveon".to_owned()]
        );
        assert_eq!(
            local_candidates(&engine, "kaveon", "default", "d"),
            vec!["default".to_owned()]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn embedded_statements_report_through_the_statement_channel() {
        let dir = crate::local::catalog::tests::parquet_fixture();
        let engine = Arc::new(Mutex::new(LocalEngine::from_data_dir(&dir).unwrap()));
        let handle = submit_local(
            Arc::clone(&engine),
            "SELECT id FROM events ORDER BY id".to_owned(),
        );
        assert!(handle.tag.is_empty());
        match handle.events.recv_timeout(Duration::from_secs(30)).unwrap() {
            StatementEvent::Finished(result) => {
                assert!(result.id.is_empty());
                assert_eq!(result.column_names(), vec!["id".to_owned()]);
                assert_eq!(result.data.len(), 3);
                assert!(result.next_uri.is_none());
            }
            StatementEvent::Failed(failure) => panic!("{failure:?}"),
        }
        let handle = submit_local(engine, "SELECT * FROM nowhere".to_owned());
        match handle.events.recv_timeout(Duration::from_secs(30)).unwrap() {
            StatementEvent::Failed(failure) => {
                assert!(failure.message.contains("nowhere"), "{}", failure.message);
            }
            StatementEvent::Finished(_) => panic!("a missing table is not a result"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_burst_of_text_keys_is_a_paste_and_typing_is_not() {
        let plain = |code| event::KeyEvent::new(code, KeyModifiers::NONE);
        let pasted: Vec<_> = "SELECT 1\n\tFROM t;"
            .chars()
            .map(|ch| match ch {
                '\n' => plain(KeyCode::Enter),
                '\t' => plain(KeyCode::Tab),
                ch => plain(KeyCode::Char(ch)),
            })
            .collect();
        assert!(is_paste_burst(&pasted));
        assert_eq!(burst_text(&pasted), "SELECT 1\n\tFROM t;");
        let two_chars = vec![plain(KeyCode::Char('S')), plain(KeyCode::Char('E'))];
        assert!(
            !is_paste_burst(&two_chars),
            "two quick characters are typing"
        );
        let long_line: Vec<_> = (0..8).map(|_| plain(KeyCode::Char('x'))).collect();
        assert!(is_paste_burst(&long_line));
        let with_ctrl = vec![
            plain(KeyCode::Char('a')),
            plain(KeyCode::Enter),
            event::KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ];
        assert!(!is_paste_burst(&with_ctrl), "a control key is never pasted");
        let line_feed = vec![
            plain(KeyCode::Char('a')),
            event::KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
            plain(KeyCode::Char('b')),
        ];
        assert!(
            is_paste_burst(&line_feed),
            "a line feed is Ctrl-Enter on Windows"
        );
        assert_eq!(
            burst_text(&line_feed),
            "a
b"
        );
        assert!(!is_paste_burst(&[plain(KeyCode::Enter)]));
    }
}
