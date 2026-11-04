//! The desktop app's elevated helper: system cleanup for an app whose window never runs elevated
//! (ADR 0010). The window starts it as an administrator or root and asks it for scans and
//! changes, naming everything by an ID the helper's own session gave out. The helper treats the
//! window as untrusted: it checks every request, re-checks every item before changing it, and
//! stops at the first message that isn't a request, or after a while with none.

pub mod client;
pub mod protocol;
pub mod server;

#[cfg(test)]
mod tests;
