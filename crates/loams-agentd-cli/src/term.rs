//! The terminal a command talks to: its output, and the questions it asks.
//! [`StdTerminal`] is the real one; tests script their own.

use std::io::{self, BufRead as _, IsTerminal as _, Write};

/// Where a command writes and how it asks.
pub trait Terminal {
    /// The command's result (text or JSON).
    fn out(&mut self) -> &mut dyn Write;
    /// Notes and warnings.
    fn err(&mut self) -> &mut dyn Write;
    /// Whether a person can answer questions.
    fn interactive(&self) -> bool;
    /// Ask and read one line, echoed.
    fn ask(&mut self, question: &str) -> io::Result<String>;
    /// Ask and read one line without echo.
    fn ask_secret(&mut self, question: &str) -> io::Result<String>;
}

/// stdout, stderr and the controlling terminal.
#[derive(Debug)]
pub struct StdTerminal {
    out: io::Stdout,
    err: io::Stderr,
}

impl StdTerminal {
    pub fn new() -> Self {
        Self {
            out: io::stdout(),
            err: io::stderr(),
        }
    }
}

impl Default for StdTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl Terminal for StdTerminal {
    fn out(&mut self) -> &mut dyn Write {
        &mut self.out
    }

    fn err(&mut self) -> &mut dyn Write {
        &mut self.err
    }

    fn interactive(&self) -> bool {
        io::stdin().is_terminal() && io::stderr().is_terminal()
    }

    fn ask(&mut self, question: &str) -> io::Result<String> {
        self.out.flush()?;
        write!(self.err, "{question}")?;
        self.err.flush()?;
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }

    fn ask_secret(&mut self, question: &str) -> io::Result<String> {
        self.out.flush()?;
        rpassword::prompt_password(question)
    }
}
