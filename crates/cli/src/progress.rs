//! `cmd/dstore/tui.go`: the latest report, the rate meter, the plain status line, the TUI model, its
//! inline renderer, the TUI log handler and colours.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dstore_client::{Progress, ProgressReport};
use dstore_gocli::{CliError, Context};
use dstore_gocompat::ctx::Ctx;
use dstore_gocompat::slog::{Attr, Handler, Level, Logger, Record};
use dstore_gocompat::time::{GoTime, Zone};

/// The latest progress report.
pub struct Latest {
    inner: Mutex<ProgressReport>,
}

impl Latest {
    pub fn new() -> Arc<Latest> {
        todo!()
    }

    pub fn get(&self) -> ProgressReport {
        todo!()
    }

    /// The callback given to the client.
    pub fn progress(self: &Arc<Self>) -> Progress {
        todo!()
    }
}

/// `rateMeter`.
pub struct RateMeter {
    window: Duration,
    samples: Vec<(Instant, i64)>,
}

impl RateMeter {
    pub fn new(window: Duration) -> RateMeter {
        todo!()
    }

    pub fn add(&mut self, t: Instant, n: i64) -> f64 {
        todo!()
    }
}

/// `statusLine`.
pub fn status_line(r: &ProgressReport, rate: f64) -> String {
    todo!()
}

/// `fraction`.
pub fn fraction(r: &ProgressReport) -> f64 {
    todo!()
}

/// Messages to the TUI model.
pub enum UiMsg {
    Tick(Instant),
    Resize(u16),
    CtrlC,
    Event {
        at: GoTime,
        level: Level,
        text: String,
    },
    Done(Option<String>),
}

/// `uiModel`.
pub struct UiModel {
    title: String,
    latest: Arc<Latest>,
    cancel: Ctx,
    zone: Arc<dyn Zone>,
    width: u16,
    rate: RateMeter,
    events: Vec<(GoTime, Level, String)>,
    cancelled: bool,
    done: Option<Option<String>>,
}

impl UiModel {
    pub fn new(title: String, latest: Arc<Latest>, cancel: Ctx, zone: Arc<dyn Zone>) -> UiModel {
        todo!()
    }

    /// true = quit.
    pub fn update(&mut self, msg: UiMsg) -> bool {
        todo!()
    }

    /// Byte-identical to the content of Go `View()`.
    pub fn view(&self) -> String {
        todo!()
    }
}

/// `teaHandler`: "msg key=value…", bytes humanised for Int64.
pub struct TeaHandler {
    level: Level,
    send: tokio::sync::mpsc::UnboundedSender<UiMsg>,
}

impl Handler for TeaHandler {
    fn enabled(&self, level: Level) -> bool {
        todo!()
    }

    fn handle(&self, handler_attrs: &[Attr], r: &Record) {
        todo!()
    }
}

/// lipgloss `Blend1D` / go-colorful `BlendLab`.
pub fn blend1d(steps: usize, a: [u8; 3], b: [u8; 3]) -> Vec<[u8; 3]> {
    todo!()
}

/// `runTransfer`: plain mode unless stderr is a character device and there is no `--no-tui`; runs `f`
/// with the logger and the progress callback.
pub async fn run_transfer<T: Send + 'static>(
    c: &Context,
    ctx: &Ctx,
    title: String,
    f: impl FnOnce(Ctx, Logger, Progress) -> Pin<Box<dyn Future<Output = Result<T, CliError>> + Send>>
    + Send
    + 'static,
) -> Result<T, CliError> {
    todo!()
}
