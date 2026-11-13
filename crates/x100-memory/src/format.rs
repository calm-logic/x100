//! The files' text: front matter, notes, the index, and what a skill says about itself. Plain
//! Markdown with a small `key: value` front matter, so a person or another program reads it
//! without x100.

use x100_core::event::Seq;
use x100_core::memory::NoteName;
use x100_core::time::Timestamp;

use crate::error::{Error, Result};

/// The fields between the first two `---` lines, and what follows them.
pub(crate) fn split(text: &str) -> (Vec<(&str, &str)>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (Vec::new(), text);
    };
    let (head, body) = match rest.find("\n---\n") {
        Some(i) => (&rest[..i], &rest[i + 5..]),
        None => match rest.strip_suffix("\n---") {
            Some(head) => (head, ""),
            None => return (Vec::new(), text),
        },
    };
    let fields = head
        .lines()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect();
    (fields, body)
}

pub(crate) fn field<'a>(fields: &[(&'a str, &'a str)], key: &str) -> Option<&'a str> {
    fields.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

fn unquote(v: &str) -> &str {
    let v = v.trim();
    for q in ['"', '\''] {
        if let Some(inner) = v.strip_prefix(q).and_then(|v| v.strip_suffix(q)) {
            return inner;
        }
    }
    v
}

/// A note as stored.
pub(crate) struct Stored {
    pub written: Timestamp,
    pub sources: Vec<Seq>,
    pub body: String,
}

pub(crate) fn encode_note(written: Timestamp, sources: &[Seq], body: &str) -> String {
    let list: Vec<String> = sources.iter().map(|s| s.0.to_string()).collect();
    format!(
        "---\nwritten: {}\nsources: [{}]\n---\n\n{}\n",
        written.as_millis(),
        list.join(", "),
        body.trim_end()
    )
}

/// A file that is not in the form `encode_note` writes, such as one the builder wrote by hand,
/// reads as a note with no front matter: its whole text is the body and it counts as the oldest.
pub(crate) fn decode_note(text: &str) -> Stored {
    let (fields, body) = split(text);
    let written = field(&fields, "written")
        .and_then(|v| v.parse().ok())
        .map_or(Timestamp::from_millis(0), Timestamp::from_millis);
    let sources = field(&fields, "sources")
        .map(|v| {
            v.trim_matches(['[', ']'])
                .split(',')
                .filter_map(|n| n.trim().parse().ok().map(Seq))
                .collect()
        })
        .unwrap_or_default();
    Stored {
        written,
        sources,
        body: body.trim().to_owned(),
    }
}

/// The first line of the text that says something, without Markdown's marks, cut to `max`
/// characters.
pub(crate) fn summary(body: &str, max: usize) -> String {
    let line = body
        .lines()
        .map(|l| l.trim().trim_start_matches(['#', '-', '*', '>', ' ']))
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut out: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        out.push_str("...");
    }
    out
}

/// A note's file name: lower-case letters, digits, `.`, `-` and `_`, ending in `.md`, which is
/// added if missing. `INDEX.md` is the index's.
pub(crate) fn note_name(name: &NoteName) -> Result<NoteName> {
    let raw = name.0.trim().to_lowercase();
    let raw = raw.as_str();
    let full = if raw.ends_with(".md") {
        raw.to_owned()
    } else {
        format!("{raw}.md")
    };
    let stem = &full[..full.len() - 3];
    let ok = !stem.is_empty()
        && full.len() <= crate::NAME_LIMIT
        && stem.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && full
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c))
        && !full.contains("..");
    if !ok {
        return Err(Error::Invalid(format!(
            "`{raw}` is not a note name: use lower-case letters, digits, `.`, `-` and `_`, \
             starting with a letter or digit, at most {} characters",
            crate::NAME_LIMIT
        )));
    }
    if full.eq_ignore_ascii_case(crate::INDEX) {
        return Err(Error::Invalid(
            "INDEX.md is the index, which x100 keeps from the notes".into(),
        ));
    }
    Ok(NoteName(full))
}

/// What the index shows of one note.
pub(crate) struct Entry {
    pub name: String,
    pub written: Timestamp,
    pub summary: String,
}

/// The index: a short header, then the newest notes with a line each, as many as fit in `limit`
/// bytes, and a last line saying how many are left out. Bounded however many notes exist, which is
/// what keeps a start pack small. The notes it leaves out are found with `search_notes`.
pub(crate) fn render_index(mut entries: Vec<Entry>, limit: usize) -> String {
    entries.sort_by(|a, b| b.written.cmp(&a.written).then_with(|| a.name.cmp(&b.name)));
    let mut out = String::from(
        "# Memory index\n\nOne line per note, newest first. `read_note` opens one; \
         `search_notes` finds the ones not listed.\n\n",
    );
    // Room for the line that counts what is left out.
    let reserve = 96;
    let total = entries.len();
    let mut shown = 0;
    for e in &entries {
        let line = format!("- [{0}]({0}): {1}\n", e.name, e.summary);
        if out.len() + line.len() + reserve > limit {
            break;
        }
        out.push_str(&line);
        shown += 1;
    }
    if shown < total {
        out.push_str(&format!(
            "- ... and {} more notes, not listed here\n",
            total - shown
        ));
    }
    out
}

/// What a skill's front matter says.
pub(crate) struct SkillMeta {
    pub name: String,
    pub description: String,
    pub version: u32,
}

pub(crate) fn skill_meta(text: &str) -> SkillMeta {
    let (fields, _) = split(text);
    SkillMeta {
        name: field(&fields, "name").map(unquote).unwrap_or("").to_owned(),
        description: field(&fields, "description")
            .map(unquote)
            .unwrap_or("")
            .to_owned(),
        version: field(&fields, "version")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    }
}

/// `text` with its `version` field set, added to the front matter if it has none.
pub(crate) fn with_version(text: &str, version: u32) -> String {
    let line = format!("version: {version}");
    let Some(rest) = text.strip_prefix("---\n") else {
        return text.to_owned();
    };
    let end = rest.find("\n---").unwrap_or(rest.len());
    let (head, tail) = rest.split_at(end);
    let mut done = false;
    let mut lines: Vec<String> = head
        .lines()
        .map(|l| {
            if l.split_once(':')
                .is_some_and(|(k, _)| k.trim() == "version")
            {
                done = true;
                line.clone()
            } else {
                l.to_owned()
            }
        })
        .collect();
    if !done {
        lines.push(line);
    }
    format!("---\n{}{tail}", lines.join("\n"))
}

/// A skill's name: lower-case words joined by single hyphens, as the Agent Skills format asks.
pub(crate) fn skill_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.split('-').all(|w| {
            !w.is_empty()
                && w.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}
