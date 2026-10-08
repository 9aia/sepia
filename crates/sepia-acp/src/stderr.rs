//! Bounded tail of an agent's stderr — prefixed, truncated, kept in a
//! fixed ring, forwarded to the process only under `SEPIA_DEBUG=1`.

use std::collections::VecDeque;
use std::sync::Mutex;

const RING_LIMIT: usize = 100;
const LINE_LIMIT: usize = 500;

pub struct StderrTail {
    lines: Mutex<VecDeque<String>>,
    prefix: String,
    debug: bool,
    remainder: Mutex<String>,
}

impl StderrTail {
    pub fn new(id: &str, debug: bool) -> Self {
        Self {
            lines: Mutex::new(VecDeque::new()),
            prefix: format!("[agent:{id}]"),
            debug,
            remainder: Mutex::new(String::new()),
        }
    }

    pub fn push(&self, chunk: &str) {
        let mut remainder = self
            .remainder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let joined = format!("{remainder}{chunk}");
        let mut parts: Vec<&str> = joined.split('\n').collect();
        *remainder = parts.pop().unwrap_or_default().to_string();
        drop(remainder);
        for part in parts {
            self.append(part);
        }
    }

    pub fn recent(&self) -> Vec<String> {
        self.lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    fn append(&self, line: &str) {
        let body = if line.chars().count() > LINE_LIMIT {
            line.chars().take(LINE_LIMIT).collect::<String>()
        } else {
            line.to_string()
        };
        let formatted = format!("{} {body}", self.prefix);
        let mut lines = self
            .lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lines.push_back(formatted.clone());
        while lines.len() > RING_LIMIT {
            lines.pop_front();
        }
        drop(lines);
        if self.debug {
            eprintln!("{formatted}");
        }
    }
}
