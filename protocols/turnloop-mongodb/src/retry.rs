//! retryable-reads/retryable-reads.md §§ Supported Read Operations, Retryable Error;
//! retryable-writes/retryable-writes.md §§ Supported Write Operations, RetryableWriteError Labels.
use crate::{Error, ErrorKind};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryKind {
    Read,
    Write,
    Never,
}
#[derive(Clone, Copy, Debug)]
pub struct Retry {
    pub kind: RetryKind,
    pub enabled: bool,
    pub attempts: u8,
    pub in_transaction: bool,
    pub wire_version: i32,
    pub sessions_supported: bool,
    pub standalone: bool,
    pub acknowledged: bool,
}
impl Retry {
    pub fn allowed(&self, error: &Error) -> bool {
        if !self.enabled || self.attempts >= 1 || self.in_transaction {
            return false;
        }
        let network = matches!(error.kind, ErrorKind::Network | ErrorKind::Timeout);
        let code = error.code.is_some_and(|c| {
            matches!(
                c,
                6 | 7 | 89 | 91 | 189 | 9001 | 10107 | 11600 | 11602 | 13435 | 13436
            )
        });
        match self.kind {
            RetryKind::Never => false,
            RetryKind::Read => self.wire_version >= 6 && (network || code),
            RetryKind::Write => {
                self.sessions_supported
                    && !self.standalone
                    && self.acknowledged
                    && self.wire_version >= 6
                    && (network
                        || error.has_label("RetryableWriteError")
                        || (self.wire_version < 9 && code))
            }
        }
    }
    pub fn retry(&mut self, error: &Error) -> bool {
        if self.allowed(error) {
            self.attempts += 1;
            true
        } else {
            false
        }
    }
    /// Command names alone are insufficient: multi writes, $out/$merge and
    /// getMore must be excluded by command inspection before calling this policy.
    pub fn read_command(name: &str) -> bool {
        matches!(
            name,
            "find"
                | "aggregate"
                | "count"
                | "distinct"
                | "listDatabases"
                | "listCollections"
                | "listIndexes"
        )
    }
}
