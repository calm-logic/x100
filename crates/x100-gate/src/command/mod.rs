//! Reading a shell command: which rows of the table it matches.
//!
//! The gate does not ask about every command (the sandbox contains those). A harness's permission
//! hook stops only commands that run a program on [`watched_programs`], and the gate reads each
//! one here. A command that matches a pattern is ruled by it. A command that runs a program with
//! no safe default use (a cloud CLI, a database client, `eval`) and matches none is unknown and
//! goes to the classifier. Anything else, `git status` or `cargo test`, is ordinary.
//!
//! The reader splits a command into simple commands (on `;`, `&&`, `||`, `|`, newlines,
//! parentheses and backticks, outside quotes), reads inside `$(...)`, backticks and `sh -c`, steps
//! over `env`, `time`, `sudo` and the like, and then looks at the program and its arguments. It is
//! a deny-list, not a shell: a command written to hide what it does ends up unmatched, and an
//! unmatched watched command is asked about, never waved through.

use crate::rule::{Matched, Rule};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    Matched(Matched),
    /// Runs a program with no safe default use, or one the reader cannot see through, and matches
    /// no pattern.
    Unknown,
    Ordinary,
}

/// One program with the words that make a command match a row. A row matches when the program is
/// listed, one of the first few arguments is in `sub` (if any), and one of `with`, `prefix` or
/// `contains` is satisfied (if any are given).
mod table;

use crate::rule::{CMD_CRED_FILE, CMD_DELETE_OUTSIDE, CMD_PIPE_TO_SHELL, CMD_SUDO};
use table::{ASK_WHEN_UNMATCHED, PATTERNS, Pattern, WATCHED_BESIDES};

const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "fish"];
const DELETERS: &[&str] = &["rm", "rmdir", "unlink", "shred"];
/// Programs that run another program, which the reader steps over.
const WRAPPERS: &[&str] = &[
    "env", "nohup", "time", "command", "exec", "nice", "ionice", "timeout", "xargs", "stdbuf",
    "setsid", "watch", "builtin",
];
const ELEVATORS: &[&str] = &["sudo", "doas", "su", "pkexec"];
/// Files that hold credentials. Any argument naming one is flagged, whatever the program.
const CREDENTIAL_PATHS: &[&str] = &[
    ".ssh",
    ".aws/",
    ".netrc",
    ".npmrc",
    ".git-credentials",
    ".docker/config.json",
    ".kube/config",
    ".config/gcloud",
    ".gnupg",
    ".pypirc",
    ".cargo/credentials",
    "id_rsa",
    "id_ed25519",
];
/// Words that make a deploy a rehearsal.
const NOT_PRODUCTION: &[&str] = &[
    "staging",
    "stage",
    "preview",
    "dev",
    "development",
    "test",
    "testing",
    "sandbox",
];
/// How deep `sh -c` and `$(...)` are followed before the command is called unreadable.
const MAX_DEPTH: usize = 4;

/// The programs a harness's permission hook should stop and put to the gate. Everything else is
/// ordinary, so the hook need not ask.
pub fn watched_programs() -> Vec<&'static str> {
    let mut all: Vec<&'static str> = PATTERNS
        .iter()
        .flat_map(|p| p.programs.iter().copied())
        .chain(WATCHED_BESIDES.iter().copied())
        .collect();
    all.sort_unstable();
    all.dedup();
    all
}

/// Reads `command`. `workspace` is the agent's own directory, so that deleting inside it is not
/// flagged; without it every absolute path counts as outside.
pub fn read(command: &str, workspace: Option<&str>) -> Reading {
    let mut scan = Scan {
        workspace: workspace.map(|w| w.trim_end_matches('/')),
        found: Vec::new(),
        unknown: false,
    };
    scan.command(command, 0);
    let best = scan.found.into_iter().reduce(|best, m| {
        if m.rule.risk > best.rule.risk {
            m
        } else {
            best
        }
    });
    match best {
        Some(m) => Reading::Matched(m),
        None if scan.unknown => Reading::Unknown,
        None => Reading::Ordinary,
    }
}

struct Scan<'a> {
    workspace: Option<&'a str>,
    found: Vec<Matched>,
    unknown: bool,
}

struct Simple {
    words: Vec<String>,
    /// Its input is the output of the command before it.
    piped_into: bool,
}

impl Scan<'_> {
    fn command(&mut self, text: &str, depth: usize) {
        if depth > MAX_DEPTH {
            self.unknown = true;
            return;
        }
        for inner in substitutions(text) {
            self.command(&inner, depth + 1);
        }
        for simple in split(text) {
            self.simple(&simple, depth);
        }
    }

    fn hit(&mut self, rule: &'static Rule, detail: &str) {
        self.found.push(Matched {
            rule,
            detail: shorten(detail, 120),
        });
    }

    fn simple(&mut self, simple: &Simple, depth: usize) {
        let words = &simple.words;
        if let Some(w) = words.iter().find(|w| names_credential_file(w)) {
            self.hit(&CMD_CRED_FILE, w);
        }
        let mut at = 0;
        while words.get(at).is_some_and(|w| is_assignment(w)) {
            at += 1;
        }
        // Step over wrappers; an elevator is itself a hit, and what it runs is still read.
        while let Some(word) = words.get(at) {
            let prog = base(word);
            let elevator = ELEVATORS.contains(&prog.as_str());
            if !elevator && !WRAPPERS.contains(&prog.as_str()) {
                break;
            }
            if elevator {
                self.hit(&CMD_SUDO, &words[at..].join(" "));
            }
            at += 1;
            while words
                .get(at)
                .is_some_and(|w| w.starts_with('-') || is_assignment(w) || is_number(w))
            {
                at += 1;
            }
        }
        let Some(first) = words.get(at) else { return };
        if first.contains('$') {
            // The program is not known until the shell expands it.
            self.unknown = true;
            return;
        }
        let prog = base(first);
        let args = &words[at + 1..];
        let line = words[at..].join(" ");

        if SHELLS.contains(&prog.as_str()) {
            match args
                .iter()
                .position(|a| a.starts_with('-') && !a.starts_with("--") && a.ends_with('c'))
            {
                Some(i) => {
                    if let Some(script) = args.get(i + 1) {
                        self.command(script, depth + 1);
                    }
                }
                None if simple.piped_into => self.hit(&CMD_PIPE_TO_SHELL, &line),
                None => {}
            }
        }
        let lower: Vec<String> = args.iter().map(|a| a.to_lowercase()).collect();
        for p in PATTERNS {
            if p.programs.contains(&prog.as_str()) && matches(p, &prog, args, &lower) {
                let rule = match p.soften {
                    Some(soft) if names_non_production(&lower) => soft,
                    _ => p.rule,
                };
                self.hit(rule, &line);
            }
        }
        if let Some(path) = self.deletes_outside(&prog, args) {
            self.hit(&CMD_DELETE_OUTSIDE, &format!("{prog} {path}"));
        }
        if ASK_WHEN_UNMATCHED.contains(&prog.as_str()) {
            self.unknown = true;
        }
    }

    /// The first path a deleting command would remove that lies outside the workspace.
    fn deletes_outside<'w>(&self, prog: &str, args: &'w [String]) -> Option<&'w str> {
        let paths: Vec<&String> = if DELETERS.contains(&prog) {
            args.iter().filter(|a| !a.starts_with('-')).collect()
        } else if prog == "find" && args.iter().any(|a| a == "-delete" || a == "rm") {
            args.iter()
                .take_while(|a| !a.starts_with('-') && !a.starts_with('(') && *a != "!")
                .collect()
        } else {
            return None;
        };
        paths
            .into_iter()
            .map(String::as_str)
            .find(|p| self.outside(p))
    }

    fn outside(&self, path: &str) -> bool {
        if path.starts_with(['~', '$', '`']) || path.split('/').any(|c| c == "..") {
            return true;
        }
        if !path.starts_with('/') {
            return false;
        }
        match self.workspace {
            Some(ws) => path != ws && !path.starts_with(&format!("{ws}/")),
            None => true,
        }
    }
}

fn matches(p: &Pattern, prog: &str, args: &[String], lower: &[String]) -> bool {
    if !p.sub.is_empty() && !has_sub(p.sub, prog, args) {
        return false;
    }
    let any_given = !(p.with.is_empty() && p.prefix.is_empty() && p.contains.is_empty());
    !any_given
        || p.with.iter().any(|w| has_arg(args, w))
        || p.prefix
            .iter()
            .any(|x| args.iter().any(|a| a.starts_with(x)))
        || p.contains
            .iter()
            .any(|x| lower.iter().any(|a| a.contains(x)))
}

/// Whether the subcommand is one of `subs`. For git it must be the first word after its own
/// options, so that a commit message saying "push" is not a push. Other programs put options
/// before the subcommand too (`kubectl -n x delete`), so the first three words are read.
fn has_sub(subs: &[&str], prog: &str, args: &[String]) -> bool {
    let mut words = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if prog == "git" && matches!(a.as_str(), "-C" | "-c" | "--git-dir" | "--work-tree") {
            it.next();
        } else if !a.starts_with('-') {
            words.push(a.as_str());
        }
    }
    let window = if prog == "git" { 1 } else { 3 };
    words.iter().take(window).any(|w| subs.contains(w))
}

/// Whether `args` has `want` as a word, as `--name=want`, or for a short flag, inside a cluster
/// such as `-rf`.
fn has_arg(args: &[String], want: &str) -> bool {
    let short = want
        .strip_prefix('-')
        .filter(|w| w.len() == 1)
        .and_then(|w| w.chars().next());
    args.iter().any(|a| {
        a == want
            || a.strip_suffix(want).is_some_and(|rest| rest.ends_with('='))
            || short
                .is_some_and(|c| a.starts_with('-') && !a.starts_with("--") && a[1..].contains(c))
    })
}

fn names_non_production(lower: &[String]) -> bool {
    lower
        .iter()
        .flat_map(|a| a.split(|c: char| !c.is_alphanumeric()))
        .any(|w| NOT_PRODUCTION.contains(&w))
}

fn names_credential_file(word: &str) -> bool {
    CREDENTIAL_PATHS.iter().any(|p| word.contains(p))
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn is_number(word: &str) -> bool {
    !word.is_empty() && word.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// The program's name without its directory, and `mkfs.ext4` as `mkfs`.
fn base(word: &str) -> String {
    let name = word.rsplit('/').next().unwrap_or(word).to_lowercase();
    match name.strip_prefix("mkfs.") {
        Some(_) => "mkfs".to_owned(),
        None => name,
    }
}

pub(crate) fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}...")
}

/// Splits a command into simple commands and their words, honouring quotes and backslashes.
fn split(text: &str) -> Vec<Simple> {
    let mut out = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut piped = false;
    let (mut single, mut double) = (false, false);
    let mut chars = text.chars().peekable();

    macro_rules! end_word {
        () => {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
        };
    }
    macro_rules! flush {
        () => {
            end_word!();
            if !words.is_empty() {
                out.push(Simple {
                    words: std::mem::take(&mut words),
                    piped_into: piped,
                });
            }
        };
    }
    macro_rules! end_simple {
        ($next_piped:expr) => {
            flush!();
            piped = $next_piped;
        };
    }

    while let Some(c) = chars.next() {
        if single {
            if c == '\'' {
                single = false;
            } else {
                word.push(c);
            }
            continue;
        }
        if double {
            match c {
                '"' => double = false,
                '\\' => word.extend(chars.next()),
                _ => word.push(c),
            }
            continue;
        }
        match c {
            '\'' => (single, started) = (true, true),
            '"' => (double, started) = (true, true),
            '\\' => {
                word.extend(chars.next());
                started = true;
            }
            ';' | '\n' | '(' | ')' | '`' => {
                end_simple!(false);
            }
            '&' => {
                if chars.peek() == Some(&'&') {
                    chars.next();
                }
                end_simple!(false);
            }
            '|' => {
                let logical = chars.peek() == Some(&'|');
                if logical || chars.peek() == Some(&'&') {
                    chars.next();
                }
                end_simple!(!logical);
            }
            c if c.is_whitespace() => {
                end_word!();
            }
            _ => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    if !words.is_empty() {
        out.push(Simple {
            words,
            piped_into: piped,
        });
    }
    out
}

/// The text inside every `$(...)` and pair of backticks, which the shell runs.
fn substitutions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && chars.get(i + 1) == Some(&'(') {
            let (mut depth, mut j) = (1, i + 2);
            while j < chars.len() && depth > 0 {
                match chars[j] {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            let end = if depth == 0 { j - 1 } else { j };
            out.push(chars[i + 2..end].iter().collect());
            i += 2;
        } else if chars[i] == '`' {
            match chars[i + 1..].iter().position(|c| *c == '`') {
                Some(n) => {
                    out.push(chars[i + 1..i + 1 + n].iter().collect());
                    i += n + 2;
                }
                None => i += 1,
            }
        } else {
            i += 1;
        }
    }
    out
}
