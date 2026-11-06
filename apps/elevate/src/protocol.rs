//! What the window and the helper say to each other: one JSON object per line, each way. The
//! window sends [`Request`]s one at a time, apart from [`Request::Cancel`]. The helper answers each
//! with any progress, then one final [`Response`]. No request carries a path: scans, items, and
//! quarantine entries are named by the IDs the helper's session gave out.

use std::error::Error;
use std::fmt;
use std::io::{self, BufRead, Read, Write};

use cleanse_desktop_views::{
    CleanProgress, CleanReport, Details, Handled, HistoryView, ItemPage, QuarantineView, Results,
    ScanProgress, Section,
};
use serde::{Deserialize, Serialize};

/// The window and the helper are installed together, so they must speak the same version.
pub const VERSION: u32 = 2;
/// The longest request the helper reads. A cleanup naming [`MAX_IDS`] items fits well within it.
pub const MAX_REQUEST: usize = 1024 * 1024;
/// The longest response the window reads: a page of items, or a long history.
pub const MAX_RESPONSE: usize = 64 * 1024 * 1024;
/// The most items or quarantine entries one request may name.
pub const MAX_IDS: usize = 10_000;
/// The longest quarantine entry ID. The quarantine's IDs are digits and dashes, far shorter.
pub const MAX_ID_LENGTH: usize = 64;

// Every request is a struct, even one with no fields: serde doesn't refuse unknown fields beside
// the tag of a unit variant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// Always first: the protocol version the window speaks.
    Hello {
        version: u32,
    },
    /// Scans the system's folders. The results replace the last scan's, and its IDs stop working.
    Scan {},
    /// Stops the scan or cleanup running now. It has no response of its own.
    Cancel {},
    /// A scan's results as they are now, after any cleanups.
    Summary {
        scan: u64,
    },
    /// The items in `section`, and of `rule` if given, largest first.
    Candidates {
        scan: u64,
        section: Section,
        rule: Option<String>,
        offset: usize,
        limit: usize,
    },
    Explain {
        scan: u64,
        item: u64,
    },
    /// Removes the items `items` names, each the way its tier says, after checking each again.
    Clean {
        scan: u64,
        items: Vec<u64>,
    },
    Quarantined {},
    Restore {
        ids: Vec<String>,
    },
    Purge {
        ids: Vec<String>,
    },
    History {},
    /// Removes what's left of removals that didn't finish.
    Finish {},
}

impl Request {
    /// Checks what parsing doesn't: how many IDs a request names, and how long each is.
    pub fn check(&self) -> Result<(), String> {
        let count = match self {
            Self::Clean { items, .. } => items.len(),
            Self::Restore { ids } | Self::Purge { ids } => {
                if ids.iter().any(|id| id.len() > MAX_ID_LENGTH) {
                    return Err(format!(
                        "a quarantine ID is longer than {MAX_ID_LENGTH} characters"
                    ));
                }
                ids.len()
            }
            Self::Candidates { rule, .. } => {
                if rule.as_ref().is_some_and(|rule| rule.len() > MAX_ID_LENGTH) {
                    return Err(format!(
                        "a rule ID is longer than {MAX_ID_LENGTH} characters"
                    ));
                }
                0
            }
            Self::Hello { .. }
            | Self::Scan {}
            | Self::Cancel {}
            | Self::Summary { .. }
            | Self::Explain { .. }
            | Self::Quarantined {}
            | Self::History {}
            | Self::Finish {} => 0,
        };
        if count > MAX_IDS {
            return Err(format!("a request names more than {MAX_IDS} items"));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "snake_case")]
pub enum Response {
    Hello {
        version: u32,
    },
    /// Progress of a scan, before its [`Response::Results`].
    ScanProgress {
        progress: ScanProgress,
    },
    /// How far a cleanup has got, before its [`Response::Cleaned`].
    CleanProgress {
        progress: CleanProgress,
    },
    Results {
        results: Results,
    },
    Page {
        page: ItemPage,
    },
    Details {
        details: Details,
    },
    Cleaned {
        report: CleanReport,
    },
    Quarantine {
        quarantine: QuarantineView,
    },
    Handled {
        handled: Handled,
    },
    History {
        history: HistoryView,
    },
    /// The request couldn't be carried out, and why, for the window to show.
    Error {
        message: String,
    },
}

impl Response {
    /// Whether more responses follow this one for the same request.
    pub fn is_progress(&self) -> bool {
        matches!(self, Self::ScanProgress { .. } | Self::CleanProgress { .. })
    }
}

/// Why a message couldn't be read.
#[derive(Debug)]
pub enum ProtocolError {
    Io(io::Error),
    /// The line is longer than either side accepts.
    TooLong,
    /// The line isn't UTF-8, or the input ended partway through it.
    NotText,
    /// The line isn't a message, or breaks a limit.
    Malformed(String),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::TooLong => f.write_str("a message is too long"),
            Self::NotText => f.write_str("a message isn't a whole line of text"),
            Self::Malformed(error) => write!(f, "a message isn't one: {error}"),
        }
    }
}

impl Error for ProtocolError {}

/// Reads one message of at most `max` bytes, or `None` where the input ends between messages.
pub fn read<T: for<'de> Deserialize<'de>>(
    input: &mut impl BufRead,
    max: usize,
) -> Result<Option<T>, ProtocolError> {
    let mut line = Vec::new();
    // Taking one byte more than `max` tells a line that's exactly `max` long from a longer one.
    let read = input
        .by_ref()
        .take(u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1))
        .read_until(b'\n', &mut line)
        .map_err(ProtocolError::Io)?;
    if read == 0 {
        return Ok(None);
    }
    if line.pop() != Some(b'\n') {
        return Err(if read > max {
            ProtocolError::TooLong
        } else {
            ProtocolError::NotText
        });
    }
    let line = String::from_utf8(line).map_err(|_| ProtocolError::NotText)?;
    serde_json::from_str(&line)
        .map(Some)
        .map_err(|error| ProtocolError::Malformed(error.to_string()))
}

/// Writes `message` as one line, and sends it on now.
pub fn write(output: &mut impl Write, message: &impl Serialize) -> io::Result<()> {
    let mut line = serde_json::to_vec(message).map_err(io::Error::other)?;
    line.push(b'\n');
    output.write_all(&line)?;
    output.flush()
}
