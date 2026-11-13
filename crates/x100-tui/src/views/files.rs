//! The Files tab: what the project holds outside its code.

use x100_client::words;
use x100_core::party::Party;

use super::Ctx;
use crate::page::{DIM, Page, Target, cell, span, spans};

pub fn page(c: &Ctx) -> Page {
    let mut page = Page::new();
    page.styled("u adds a file, x removes the selected one.", DIM);
    page.styled(
        format!(
            "{}{}{}{}",
            cell("NAME", 30),
            cell("KIND", 14),
            cell("ADDED BY", 14),
            "SIZE"
        ),
        DIM,
    );
    let mut files: Vec<_> = c.p.files().collect();
    files.sort_by_key(|f| (f.added, f.file.id));
    for f in files {
        let by = match f.file.added_by {
            Party::Builder => "You".to_owned(),
            Party::Agent(a) => super::role(c.p, a),
        };
        page.row(
            spans(vec![
                span(cell(&f.file.name, 30), Default::default()),
                span(cell(&words::kind(&f.file.media_type), 14), DIM),
                span(cell(&by, 14), DIM),
                span(words::size(f.file.size), DIM),
            ]),
            Target::File(f.file.id),
        );
    }
    if page.targets().is_empty() {
        page.text("The project holds no files.");
    }
    page
}
