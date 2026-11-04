//! The helper's side: system cleanup's session, driven by requests from a window it doesn't trust.

use std::io::{BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use cleanse_desktop_views::{self as views, HistoryView, QuarantineView, ScanProgress};
use cleanse_engine::{ScanId, Session, SessionError};
use cleanse_model::{CandidateId, RuleId};

use crate::protocol::{self, MAX_REQUEST, ProtocolError, Request, Response, VERSION};

/// How long the helper waits for a request before it stops: the window it was started for may
/// have gone without closing its end.
pub const IDLE: Duration = Duration::from_mins(10);

/// Progress is sent at most this often, so a fast scan doesn't flood the window.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Why the helper stopped serving.
#[derive(Debug, PartialEq, Eq)]
pub enum Ending {
    /// The window closed its end.
    Closed,
    /// Nothing was asked for in the time allowed.
    Idle,
    /// The window sent something that isn't a request, so the helper stops listening to it.
    Malformed(String),
    /// Answers couldn't be sent, so the window is gone.
    Gone,
}

/// Answers the requests on `input` with `session`, writing to `output`, until the window closes
/// its end, sends anything but a request, or asks for nothing for `idle`. The first request must
/// be [`Request::Hello`].
pub fn serve(
    session: &Arc<Session>,
    input: impl BufRead + Send + 'static,
    output: impl Write + Send,
    idle: Duration,
) -> Ending {
    let (sender, requests) = mpsc::channel();
    let reading = Arc::clone(session);
    // Not joined: it may be waiting on input when serving ends, and goes when the input does.
    thread::spawn(move || read_requests(input, &reading, &sender));
    let output = Mutex::new(output);
    let mut greeted = false;
    loop {
        let request = match requests.recv_timeout(idle) {
            Ok(Ok(request)) => request,
            Ok(Err(ending)) => return ending,
            Err(RecvTimeoutError::Timeout) => return Ending::Idle,
            Err(RecvTimeoutError::Disconnected) => return Ending::Closed,
        };
        let response = match (greeted, request) {
            (false, Request::Hello { version }) if version == VERSION => {
                greeted = true;
                Response::Hello { version: VERSION }
            }
            (false, Request::Hello { version }) => {
                let message = format!("this helper speaks version {VERSION}, not {version}");
                let _ = send(
                    &output,
                    &Response::Error {
                        message: message.clone(),
                    },
                );
                return Ending::Malformed(message);
            }
            (false, _) => return Ending::Malformed("the first request wasn't hello".into()),
            (true, Request::Hello { .. }) => {
                return Ending::Malformed("hello came twice".into());
            }
            (true, request) => handle(session, request, &output),
        };
        if send(&output, &response).is_err() {
            return Ending::Gone;
        }
    }
}

/// Reads requests and passes them on, apart from [`Request::Cancel`], which can't wait behind the
/// work it stops. The first that isn't a request ends serving.
fn read_requests(
    mut input: impl BufRead,
    session: &Session,
    requests: &Sender<Result<Request, Ending>>,
) {
    loop {
        let request = match protocol::read::<Request>(&mut input, MAX_REQUEST) {
            Ok(Some(request)) => request.check().map(|()| request),
            Ok(None) | Err(ProtocolError::Io(_)) => {
                let _ = requests.send(Err(Ending::Closed));
                return;
            }
            Err(error) => Err(error.to_string()),
        };
        match request {
            Ok(Request::Cancel {}) => session.cancel(),
            Ok(request) => {
                if requests.send(Ok(request)).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = requests.send(Err(Ending::Malformed(error)));
                return;
            }
        }
    }
}

/// Carries out `request`, sending any progress on the way, and returns its final response.
fn handle(session: &Session, request: Request, output: &Mutex<impl Write + Send>) -> Response {
    let answer = match request {
        Request::Scan {} => {
            let throttle = Throttle::default();
            session
                .scan(&[], &|update| {
                    if throttle.due() {
                        let progress = ScanProgress::from(update);
                        // Only progress: if it can't be sent, the final answer can't either.
                        let _ = send(output, &Response::ScanProgress { progress });
                    }
                })
                .and_then(|scan| views::results(session, scan))
                .map(|results| Response::Results { results })
        }
        Request::Summary { scan } => {
            views::results(session, ScanId(scan)).map(|results| Response::Results { results })
        }
        Request::Candidates {
            scan,
            section,
            rule,
            offset,
            limit,
        } => {
            let rule = rule.map(RuleId);
            views::item_page(session, ScanId(scan), section, rule.as_ref(), offset, limit)
                .map(|page| Response::Page { page })
        }
        Request::Explain { scan, item } => {
            session
                .explain(ScanId(scan), CandidateId(item))
                .map(|explained| Response::Details {
                    details: views::Details::new(&explained, session.engine().rules()),
                })
        }
        Request::Clean { scan, items } => {
            let ids: Vec<CandidateId> = items.into_iter().map(CandidateId).collect();
            let throttle = Throttle::default();
            session
                .clean(ScanId(scan), &ids, &|done| {
                    if throttle.due() {
                        let _ = send(output, &Response::CleanProgress { done });
                    }
                })
                .and_then(|cleaned| views::clean_report(session, ScanId(scan), cleaned))
                .map(|report| Response::Cleaned { report })
        }
        Request::Quarantined {} => session
            .quarantined()
            .map_err(SessionError::Io)
            .map(|listing| Response::Quarantine {
                quarantine: QuarantineView::new(&listing, session.engine().rules()),
            }),
        Request::Restore { ids } => session.restore(&ids).map(|handled| Response::Handled {
            handled: handled.into(),
        }),
        Request::Purge { ids } => session.purge(&ids).map(|handled| Response::Handled {
            handled: handled.into(),
        }),
        Request::History {} => {
            session
                .history()
                .map_err(SessionError::Io)
                .map(|history| Response::History {
                    history: HistoryView::from(&history),
                })
        }
        Request::Finish {} => session.finish().map(|finished| Response::Handled {
            handled: finished.into(),
        }),
        // `serve` answers hello, and cancel never gets this far.
        Request::Hello { .. } | Request::Cancel {} => {
            unreachable!("serve and read_requests handle these")
        }
    };
    answer.unwrap_or_else(|error| Response::Error {
        message: error.to_string(),
    })
}

fn send(output: &Mutex<impl Write + Send>, response: &Response) -> std::io::Result<()> {
    let mut output = output.lock().unwrap_or_else(PoisonError::into_inner);
    protocol::write(&mut *output, response)
}

/// Lets an update through at most every [`PROGRESS_INTERVAL`].
#[derive(Default)]
struct Throttle(Mutex<Option<Instant>>);

impl Throttle {
    fn due(&self) -> bool {
        let mut last = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        if last.is_some_and(|last| now.duration_since(last) < PROGRESS_INTERVAL) {
            return false;
        }
        *last = Some(now);
        true
    }
}
