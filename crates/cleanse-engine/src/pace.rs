//! Progress for front ends that show it at a steady pace.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

/// Runs `work`, passing it a function to report progress with, and hands `send` the newest
/// report at most every `interval`, from a thread of its own, then the last one once `work`
/// returns. Reports that come faster are dropped, but never the newest: a front end that skipped
/// a report because the one before it was too recent would go on showing the older one through
/// a slow step, such as removing a folder of many thousands of files.
pub fn paced<T: Send, R>(
    interval: Duration,
    send: impl Fn(T) + Sync,
    work: impl FnOnce(&(dyn Fn(T) + Sync)) -> R,
) -> R {
    let newest = Mutex::new(None);
    let take = || newest.lock().unwrap_or_else(PoisonError::into_inner).take();
    let (finished, wait) = mpsc::channel::<()>();
    let (take, send) = (&take, &send);
    thread::scope(|scope| {
        scope.spawn(move || {
            loop {
                // Ends without waiting out the interval once `work` returns and `finished`
                // is dropped.
                let done = !matches!(wait.recv_timeout(interval), Err(RecvTimeoutError::Timeout));
                if let Some(report) = take() {
                    send(report);
                }
                if done {
                    break;
                }
            }
        });
        let result = work(&|report| {
            *newest.lock().unwrap_or_else(PoisonError::into_inner) = Some(report);
        });
        drop(finished);
        result
    })
}
