//! The Infra tab. The infrastructure map is not in v1, so it says so and shows nothing more.

use gpui::{Context, Window, div, prelude::*, px};

use crate::theme;

pub struct InfraView;

impl Render for InfraView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(24.))
            .text_size(px(theme::TEXT))
            .text_color(theme::dim())
            .child("The infrastructure map is not in this version of x100.")
    }
}
