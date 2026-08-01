use alloc::{collections::VecDeque, format, string::String};
use embassy_time::Instant;

const MAX_LOG_LINES: usize = 120;

pub struct LogBuffer {
    lines: VecDeque<String>,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self {
            lines: VecDeque::with_capacity(MAX_LOG_LINES),
        }
    }

    pub fn info(&mut self, message: &str) {
        self.push("INFO", message);
    }

    pub fn warn(&mut self, message: &str) {
        self.push("WARN", message);
    }

    pub fn error(&mut self, message: &str) {
        self.push("ERROR", message);
    }

    pub fn render(&self) -> String {
        let mut output = String::new();
        for line in &self.lines {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(line);
        }
        output
    }

    fn push(&mut self, level: &str, message: &str) {
        if self.lines.len() == MAX_LOG_LINES {
            self.lines.pop_front();
        }
        self.lines
            .push_back(format!("[{uptime:>6}s] {level:<5} {message}", uptime = Instant::now().as_secs()));
    }
}
