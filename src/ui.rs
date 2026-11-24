//! Terminal helpers: colors, interactive prompts and tables. Messages go to
//! stderr; only data goes to stdout.

use crate::failure::{Kind, fail};
use anyhow::Result;
use std::io::IsTerminal;


/// Whether a person is at the terminal: both stdin and stdout are a TTY.
pub fn tty() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

fn colors_on(stream_is_tty: bool) -> bool {
    stream_is_tty && std::env::var_os("NO_COLOR").is_none()
}

/// ANSI painter that does nothing when disabled.
#[derive(Clone, Copy, Debug)]
pub struct Colors {
    enabled: bool,
}

macro_rules! paint {
    ($($name:ident => $code:expr),* $(,)?) => {
        $(pub fn $name(&self, text: &str) -> String { self.wrap($code, text) })*
    };
}

impl Colors {
    pub fn stderr() -> Self {
        Self {
            enabled: colors_on(std::io::stderr().is_terminal()),
        }
    }

    pub fn stdout() -> Self {
        Self {
            enabled: colors_on(std::io::stdout().is_terminal()),
        }
    }

    fn wrap(&self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    paint! {
        bold => "1",
        dim => "2",
        red => "31",
        green => "32",
        yellow => "33",
        cyan => "36",
        gray => "90",
    }
}

pub fn success(message: &str) {
    eprintln!("{} {message}", Colors::stderr().green("✔"));
}

pub fn info(message: &str) {
    eprintln!("{} {message}", Colors::stderr().cyan("●"));
}

pub fn warn(message: &str) {
    eprintln!("{} {message}", Colors::stderr().yellow("▲"));
}

fn cancelled() -> anyhow::Error {
    fail(Kind::Cancelled, "Cancelado.")
}

/// Asks for a line of text on stdin. End of input (Ctrl-D, a closed pipe, a
/// pseudo-terminal with nothing behind it) or a read error is `None`: the
/// caller cancels instead of waiting forever, which is what a key-by-key
/// prompt did on a terminal whose input was at EOF.
pub fn ask_line(message: &str) -> Option<String> {
    use std::io::Write;
    eprint!("{} {message}: ", Colors::stderr().cyan("?"));
    let _ = std::io::stderr().flush();
    read_answer(&mut std::io::stdin().lock())
}

fn read_answer(input: &mut impl std::io::BufRead) -> Option<String> {
    let mut line = String::new();
    match input.read_line(&mut line) {
        Ok(0) | Err(_) => {
            eprintln!();
            None
        }
        Ok(_) => Some(line.trim().to_owned()),
    }
}

/// Asks for a non-empty line of text; EOF cancels.
pub fn ask(message: &str) -> Result<String> {
    loop {
        match ask_line(message) {
            None => return Err(cancelled()),
            Some(v) if v.is_empty() => continue,
            Some(v) => return Ok(v),
        }
    }
}

/// A choice among options; returns the index.
pub fn select(message: &str, items: &[String]) -> Result<usize> {
    dialoguer::Select::new()
        .with_prompt(message)
        .items(items)
        .default(0)
        .interact()
        .map_err(|_| cancelled())
}

/// A secret typed without echo.
pub fn secret(message: &str) -> Result<String> {
    dialoguer::Password::new()
        .with_prompt(message)
        .interact()
        .map_err(|_| cancelled())
}

/// Visible width of a cell (code points).
pub fn width(text: &str) -> usize {
    let mut n = 0;
    let mut in_escape = false;
    for c in text.chars() {
        if in_escape {
            if c == 'm' {
                in_escape = false;
            }
        } else if c == '\x1b' {
            in_escape = true;
        } else {
            n += 1;
        }
    }
    n
}

/// A plain aligned table with a dim header, for terminals.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let c = Colors::stdout();
    let mut widths: Vec<usize> = headers.iter().map(|h| width(h)).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(width(cell));
            }
        }
    }
    let line = |cells: Vec<String>| -> String {
        let last = cells.len().saturating_sub(1);
        cells
            .into_iter()
            .enumerate()
            .map(|(i, cell)| {
                if i == last {
                    cell
                } else {
                    let pad = widths[i].saturating_sub(width(&cell));
                    format!("{cell}{}", " ".repeat(pad))
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
    };
    let mut out = vec![c.dim(&line(headers.iter().map(|h| h.to_uppercase()).collect()))];
    out.extend(rows.iter().map(|r| line(r.clone())));
    out.join("\n")
}

/// Paints a status word: green when healthy, red when failed, yellow otherwise.
pub fn status(text: &str) -> String {
    let c = Colors::stdout();
    match text {
        "live" | "ready" | "ok" | "succeeded" | "success" => c.green(text),
        "failed" | "error" | "cancelled" | "degraded" => c.red(text),
        "" | "-" => c.gray("-"),
        _ => c.yellow(text),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn end_of_input_is_no_answer() {
        assert_eq!(super::read_answer(&mut std::io::Cursor::new(b"")), None);
        assert_eq!(super::read_answer(&mut std::io::Cursor::new(b"  tienda \n")).as_deref(), Some("tienda"));
    }
}
