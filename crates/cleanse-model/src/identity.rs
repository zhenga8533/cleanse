/// Identifies a file or directory independently of its path, so Cleanse can tell whether the
/// item it scanned is still the one at that path when it comes to remove it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    /// The device (Unix) or volume serial number (Windows).
    pub volume: u64,
    /// The inode (Unix) or file ID (Windows, 128 bits wide on ReFS).
    pub file: u128,
}
