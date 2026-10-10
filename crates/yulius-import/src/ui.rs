//! What `yulius onboard` shows and asks (SPEC-001 IMP-009): numbered steps, aligned facts,
//! progress bars, choices and a picker.
//!
//! [`TerminalUi`] draws on an interactive terminal with colours (off with `NO_COLOR`,
//! `CLICOLOR=0`, `--no-color` or a pipe), `indicatif` progress bars and `dialoguer` prompts.
//! [`PlainUi`] writes plain lines to any writer and reads answers from any reader (pipes, CI,
//! tests); it never draws bars or escape codes.

use std::io::{self, BufRead, Write};
use std::time::Duration;

use console::{Style, Term, style};
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, Input, MultiSelect, Select};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

/// The tone of a fact line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Neutral.
    Plain,
    /// Good news (done, ready).
    Good,
    /// Needs attention (pending, refused).
    Warn,
    /// Secondary detail.
    Dim,
}

/// What a progress bar counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// Bytes (shown as KiB/MiB, with a rate).
    Bytes,
    /// Items.
    Items,
}

/// The terminal boundary of the onboarding run.
pub trait Ui {
    /// Starts step `index` of `total`.
    fn step(&mut self, index: usize, total: usize, title: &str) -> io::Result<()>;
    /// A `label  value` fact under the current step.
    fn fact(&mut self, label: &str, value: &str, tone: Tone) -> io::Result<()>;
    /// An indented detail line.
    fn detail(&mut self, text: &str) -> io::Result<()>;
    /// A warning.
    fn warn(&mut self, text: &str) -> io::Result<()>;
    /// Starts a progress bar (or spinner when `total` is 0).
    fn progress_start(&mut self, label: &str, total: u64, unit: Unit);
    /// Moves the bar to `done` with a short message.
    fn progress_set(&mut self, done: u64, message: &str);
    /// Finishes the bar with a closing line.
    fn progress_finish(&mut self, message: &str) -> io::Result<()>;
    /// One of `options`; `default` is preselected.
    fn choose(&mut self, prompt: &str, options: &[String], default: usize) -> io::Result<usize>;
    /// Any subset of `items`; `selected` marks the preselected ones.
    fn pick(&mut self, prompt: &str, items: &[String], selected: &[bool]) -> io::Result<Vec<usize>>;
    /// Free text with a default.
    fn ask(&mut self, prompt: &str, default: &str) -> io::Result<String>;
    /// Yes or no.
    fn confirm(&mut self, prompt: &str, default: bool) -> io::Result<bool>;
    /// The closing summary heading.
    fn done(&mut self, title: &str, ok: bool) -> io::Result<()>;
}

/// Plain lines to a writer, answers from a reader.
pub struct PlainUi<R, W> {
    input: R,
    output: W,
    progress_label: String,
    progress_total: u64,
    progress_unit: Unit,
    next_report: u64,
}

impl<R: BufRead, W: Write> PlainUi<R, W> {
    /// A plain UI over `input` and `output`.
    pub fn new(input: R, output: W) -> Self {
        Self {
            input,
            output,
            progress_label: String::new(),
            progress_total: 0,
            progress_unit: Unit::Items,
            next_report: 0,
        }
    }

    /// The writer (tests read what was written).
    pub fn output(&self) -> &W {
        &self.output
    }

    fn read_answer(&mut self) -> io::Result<Option<String>> {
        let mut answer = String::new();
        if self.input.read_line(&mut answer)? == 0 {
            writeln!(self.output)?;
            return Ok(None);
        }
        Ok(Some(answer.trim().to_owned()))
    }
}

/// `1.5 MiB`-style sizes.
#[must_use]
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

impl<R: BufRead, W: Write> Ui for PlainUi<R, W> {
    fn step(&mut self, index: usize, total: usize, title: &str) -> io::Result<()> {
        writeln!(self.output, "\n[{index}/{total}] {title}")
    }

    fn fact(&mut self, label: &str, value: &str, _tone: Tone) -> io::Result<()> {
        writeln!(self.output, "  {label:<12} {value}")
    }

    fn detail(&mut self, text: &str) -> io::Result<()> {
        writeln!(self.output, "               {text}")
    }

    fn warn(&mut self, text: &str) -> io::Result<()> {
        writeln!(self.output, "  ! {text}")
    }

    fn progress_start(&mut self, label: &str, total: u64, unit: Unit) {
        self.progress_label = label.to_owned();
        self.progress_total = total;
        self.progress_unit = unit;
        self.next_report = total / 4;
        let size = match unit {
            Unit::Bytes => human_bytes(total),
            Unit::Items => total.to_string(),
        };
        let _ = writeln!(self.output, "  {label} ({size})");
    }

    fn progress_set(&mut self, done: u64, _message: &str) {
        let total = self.progress_total;
        if total > 0 && done < total && done >= self.next_report && self.next_report > 0 {
            self.next_report = done.saturating_add(total / 4);
            let _ = writeln!(self.output, "    {}%", done.saturating_mul(100) / total);
        }
    }

    fn progress_finish(&mut self, message: &str) -> io::Result<()> {
        writeln!(self.output, "  ✓ {message}")
    }

    fn choose(&mut self, prompt: &str, options: &[String], default: usize) -> io::Result<usize> {
        writeln!(self.output, "  {prompt}")?;
        for (index, option) in options.iter().enumerate() {
            let mark = if index == default { "*" } else { " " };
            writeln!(self.output, "   {mark}{}) {option}", index + 1)?;
        }
        write!(self.output, "  Choose 1-{} [{}]: ", options.len(), default + 1)?;
        self.output.flush()?;
        let Some(answer) = self.read_answer()? else {
            return Ok(default);
        };
        if answer.is_empty() {
            return Ok(default);
        }
        if let Ok(number) = answer.parse::<usize>()
            && (1..=options.len()).contains(&number)
        {
            return Ok(number - 1);
        }
        let lower = answer.to_lowercase();
        Ok(options
            .iter()
            .position(|option| option.to_lowercase().starts_with(&lower))
            .unwrap_or(default))
    }

    fn pick(&mut self, prompt: &str, items: &[String], selected: &[bool]) -> io::Result<Vec<usize>> {
        writeln!(self.output, "  {prompt}")?;
        for (index, item) in items.iter().enumerate() {
            let mark = if selected.get(index) == Some(&true) { "x" } else { " " };
            writeln!(self.output, "   [{mark}] {:>3}. {item}", index + 1)?;
        }
        write!(
            self.output,
            "  Numbers to upload (e.g. 1,3-5), \"all\", \"none\", or Enter to keep the marked ones: "
        )?;
        self.output.flush()?;
        let Some(answer) = self.read_answer()? else {
            return Ok(marked(selected));
        };
        Ok(parse_selection(&answer, items.len()).unwrap_or_else(|| marked(selected)))
    }

    fn ask(&mut self, prompt: &str, default: &str) -> io::Result<String> {
        write!(self.output, "  {prompt} [{default}]: ")?;
        self.output.flush()?;
        Ok(self
            .read_answer()?
            .filter(|answer| !answer.is_empty())
            .unwrap_or_else(|| default.to_owned()))
    }

    fn confirm(&mut self, prompt: &str, default: bool) -> io::Result<bool> {
        let hint = if default { "Y/n" } else { "y/N" };
        write!(self.output, "\n  {prompt} [{hint}]: ")?;
        self.output.flush()?;
        let Some(answer) = self.read_answer()? else {
            return Ok(false);
        };
        Ok(match answer.to_ascii_lowercase().as_str() {
            "" => default,
            "y" | "yes" => true,
            _ => false,
        })
    }

    fn done(&mut self, title: &str, ok: bool) -> io::Result<()> {
        let mark = if ok { "✓" } else { "!" };
        writeln!(self.output, "\n{mark} {title}")
    }
}

fn marked(selected: &[bool]) -> Vec<usize> {
    selected
        .iter()
        .enumerate()
        .filter_map(|(index, on)| on.then_some(index))
        .collect()
}

/// Parses `1,3-5`, `all` or `none` against `len` items; `None` for an empty or invalid answer.
#[must_use]
pub fn parse_selection(answer: &str, len: usize) -> Option<Vec<usize>> {
    let answer = answer.trim().to_ascii_lowercase();
    match answer.as_str() {
        "" => return None,
        "all" => return Some((0..len).collect()),
        "none" => return Some(Vec::new()),
        _ => {}
    }
    let mut chosen = Vec::new();
    for part in answer.split([',', ' ']).filter(|p| !p.is_empty()) {
        let (from, to) = match part.split_once('-') {
            Some((a, b)) => (a.trim().parse::<usize>().ok()?, b.trim().parse::<usize>().ok()?),
            None => {
                let n = part.parse::<usize>().ok()?;
                (n, n)
            }
        };
        if from == 0 || to < from || to > len {
            return None;
        }
        chosen.extend((from - 1)..to);
    }
    chosen.sort_unstable();
    chosen.dedup();
    Some(chosen)
}

/// An interactive terminal: colours, bars and prompts on stderr, facts on stdout.
pub struct TerminalUi {
    out: Term,
    err: Term,
    bar: Option<ProgressBar>,
    theme: ColorfulTheme,
}

impl Default for TerminalUi {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalUi {
    /// A UI on the process's terminal. Call [`set_colors`] first to honour `--no-color`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            out: Term::stdout(),
            err: Term::stderr(),
            bar: None,
            theme: ColorfulTheme::default(),
        }
    }

    /// Whether this process talks to a person (stdin and stderr are terminals).
    #[must_use]
    pub fn attended() -> bool {
        Term::stderr().is_term() && console::user_attended() && std::io::IsTerminal::is_terminal(&io::stdin())
    }

    fn tone(tone: Tone) -> Style {
        match tone {
            Tone::Plain => Style::new(),
            Tone::Good => Style::new().green(),
            Tone::Warn => Style::new().yellow(),
            Tone::Dim => Style::new().dim(),
        }
    }

    fn prompt_error(error: dialoguer::Error) -> io::Error {
        match error {
            dialoguer::Error::IO(error) => error,
        }
    }
}

/// Turns colours off when `no_color` is set or the environment asks for it (`NO_COLOR`,
/// `CLICOLOR=0`; `console` also turns them off when the output is not a terminal).
pub fn set_colors(no_color: bool) {
    let env_off = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    if no_color || env_off {
        console::set_colors_enabled(false);
        console::set_colors_enabled_stderr(false);
    }
}

impl Ui for TerminalUi {
    fn step(&mut self, index: usize, total: usize, title: &str) -> io::Result<()> {
        self.out.write_line("")?;
        self.out.write_line(&format!(
            "{} {}",
            style(format!("{index}/{total}")).cyan().bold(),
            style(title).bold()
        ))
    }

    fn fact(&mut self, label: &str, value: &str, tone: Tone) -> io::Result<()> {
        self.out.write_line(&format!(
            "  {} {}",
            style(format!("{label:<12}")).dim(),
            Self::tone(tone).apply_to(value)
        ))
    }

    fn detail(&mut self, text: &str) -> io::Result<()> {
        self.out
            .write_line(&format!("               {}", style(text).dim()))
    }

    fn warn(&mut self, text: &str) -> io::Result<()> {
        self.out
            .write_line(&format!("  {} {}", style("!").yellow().bold(), style(text).yellow()))
    }

    fn progress_start(&mut self, label: &str, total: u64, unit: Unit) {
        let bar = if total == 0 {
            let spinner = ProgressBar::new_spinner();
            spinner.set_style(
                ProgressStyle::with_template("  {spinner:.cyan} {prefix} {wide_msg:.dim}")
                    .unwrap_or_else(|_| ProgressStyle::default_spinner()),
            );
            spinner.enable_steady_tick(Duration::from_millis(90));
            spinner
        } else {
            let template = match unit {
                Unit::Bytes => {
                    "  {prefix} [{bar:32.cyan/blue}] {bytes}/{total_bytes} · {binary_bytes_per_sec} · {eta} left\n    {wide_msg:.dim}"
                }
                Unit::Items => "  {prefix} [{bar:32.cyan/blue}] {pos}/{len}\n    {wide_msg:.dim}",
            };
            let bar = ProgressBar::new(total);
            bar.set_style(
                ProgressStyle::with_template(template)
                    .unwrap_or_else(|_| ProgressStyle::default_bar())
                    .progress_chars("━╸─"),
            );
            bar
        };
        bar.set_draw_target(ProgressDrawTarget::stderr());
        bar.set_prefix(label.to_owned());
        self.bar = Some(bar);
    }

    fn progress_set(&mut self, done: u64, message: &str) {
        if let Some(bar) = &self.bar {
            bar.set_position(done);
            bar.set_message(message.to_owned());
        }
    }

    fn progress_finish(&mut self, message: &str) -> io::Result<()> {
        if let Some(bar) = self.bar.take() {
            bar.finish_and_clear();
        }
        self.out
            .write_line(&format!("  {} {message}", style("✓").green().bold()))
    }

    fn choose(&mut self, prompt: &str, options: &[String], default: usize) -> io::Result<usize> {
        Select::with_theme(&self.theme)
            .with_prompt(prompt)
            .items(options)
            .default(default)
            .interact_on(&self.err)
            .map_err(Self::prompt_error)
    }

    fn pick(&mut self, prompt: &str, items: &[String], selected: &[bool]) -> io::Result<Vec<usize>> {
        MultiSelect::with_theme(&self.theme)
            .with_prompt(format!("{prompt} (space toggles, a toggles all, enter confirms)"))
            .items(items)
            .defaults(selected)
            .max_length(14)
            .interact_on(&self.err)
            .map_err(Self::prompt_error)
    }

    fn ask(&mut self, prompt: &str, default: &str) -> io::Result<String> {
        Input::<String>::with_theme(&self.theme)
            .with_prompt(prompt)
            .default(default.to_owned())
            .interact_text_on(&self.err)
            .map_err(Self::prompt_error)
    }

    fn confirm(&mut self, prompt: &str, default: bool) -> io::Result<bool> {
        self.out.write_line("")?;
        Confirm::with_theme(&self.theme)
            .with_prompt(prompt)
            .default(default)
            .interact_on(&self.err)
            .map_err(Self::prompt_error)
    }

    fn done(&mut self, title: &str, ok: bool) -> io::Result<()> {
        self.out.write_line("")?;
        let mark = if ok {
            style("✓").green().bold()
        } else {
            style("!").yellow().bold()
        };
        self.out
            .write_line(&format!("{mark} {}", style(title).bold()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections_parse_ranges_lists_and_words() {
        assert_eq!(parse_selection("1,3-4", 5), Some(vec![0, 2, 3]));
        assert_eq!(parse_selection(" 2 2 1 ", 5), Some(vec![0, 1]));
        assert_eq!(parse_selection("all", 3), Some(vec![0, 1, 2]));
        assert_eq!(parse_selection("none", 3), Some(vec![]));
        assert_eq!(parse_selection("", 3), None);
        assert_eq!(parse_selection("0", 3), None);
        assert_eq!(parse_selection("2-9", 3), None);
        assert_eq!(parse_selection("x", 3), None);
    }

    #[test]
    fn plain_prompts_read_answers_and_fall_back_to_defaults() -> io::Result<()> {
        let options = vec!["All".to_owned(), "Recent".to_owned(), "Pick".to_owned()];
        let mut ui = PlainUi::new(io::Cursor::new(b"3\n\nrec\n".to_vec()), Vec::new());
        assert_eq!(ui.choose("Which?", &options, 1)?, 2);
        assert_eq!(ui.choose("Which?", &options, 1)?, 1);
        assert_eq!(ui.choose("Which?", &options, 0)?, 1);
        assert_eq!(ui.choose("Which?", &options, 0)?, 0, "end of input keeps the default");
        let items = vec!["a".to_owned(), "b".to_owned()];
        let mut ui = PlainUi::new(io::Cursor::new(b"2\n\n".to_vec()), Vec::new());
        assert_eq!(ui.pick("Pick", &items, &[true, false])?, vec![1]);
        assert_eq!(ui.pick("Pick", &items, &[true, false])?, vec![0]);
        let mut ui = PlainUi::new(io::Cursor::new(b"\nn\n".to_vec()), Vec::new());
        assert!(ui.confirm("Go?", true)?);
        assert!(!ui.confirm("Go?", true)?);
        assert!(!ui.confirm("Go?", true)?, "no answer is no");
        let text = String::from_utf8_lossy(ui.output()).into_owned();
        assert!(text.contains("Go? [Y/n]"));
        Ok(())
    }

    #[test]
    fn sizes_are_human() {
        assert_eq!(human_bytes(10), "10 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 MiB");
    }
}
