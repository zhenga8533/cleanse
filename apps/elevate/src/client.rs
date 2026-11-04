//! The window's side: asking a helper it started for system cleanup.

use std::io::{self, BufRead, Write};
use std::sync::{Mutex, PoisonError};

use crate::protocol::{self, MAX_RESPONSE, ProtocolError, Request, Response, VERSION};

/// A connection to a helper: its answers come in on `R`, and requests go out on `W`.
pub struct Client<R, W> {
    answers: Mutex<R>,
    requests: Mutex<W>,
}

impl<R: BufRead, W: Write> Client<R, W> {
    /// Says hello, and checks that the helper speaks this version.
    pub fn connect(answers: R, requests: W) -> Result<Self, ProtocolError> {
        let client = Self {
            answers: Mutex::new(answers),
            requests: Mutex::new(requests),
        };
        match client.ask(&Request::Hello { version: VERSION }, &mut |_| {})? {
            Response::Hello { version } if version == VERSION => Ok(client),
            Response::Error { message } => Err(ProtocolError::Malformed(message)),
            other => Err(ProtocolError::Malformed(format!(
                "the helper answered hello with {other:?}"
            ))),
        }
    }

    /// Sends `request`, passes any progress to `progress`, and returns the final response. One
    /// request is answered at a time, so a second waits for the first.
    pub fn ask(
        &self,
        request: &Request,
        progress: &mut dyn FnMut(Response),
    ) -> Result<Response, ProtocolError> {
        let mut answers = self.answers.lock().unwrap_or_else(PoisonError::into_inner);
        self.send(request).map_err(ProtocolError::Io)?;
        loop {
            let response =
                protocol::read::<Response>(&mut *answers, MAX_RESPONSE)?.ok_or_else(|| {
                    ProtocolError::Io(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the helper stopped",
                    ))
                })?;
            if !response.is_progress() {
                return Ok(response);
            }
            progress(response);
        }
    }

    /// Asks the helper to stop the scan or cleanup it's running, while [`Client::ask`] waits for
    /// it.
    pub fn cancel(&self) -> io::Result<()> {
        self.send(&Request::Cancel {})
    }

    fn send(&self, request: &Request) -> io::Result<()> {
        let mut requests = self.requests.lock().unwrap_or_else(PoisonError::into_inner);
        protocol::write(&mut *requests, request)
    }
}
