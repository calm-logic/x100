//! The Preview tab: a link to the development build, for the builder's own browser.

use super::Ctx;
use crate::page::{DIM, Page};

pub fn page(c: &Ctx) -> Page {
    let mut page = Page::new();
    match c.p.project().and_then(|p| p.preview.as_deref()) {
        Some(url) => {
            page.text("The development build is at:");
            page.text(format!("  {url}"));
            page.styled("Open it in your browser.", DIM);
        }
        None => {
            page.text("No development build is running yet. When an agent starts one, its address is here.");
        }
    }
    page
}
