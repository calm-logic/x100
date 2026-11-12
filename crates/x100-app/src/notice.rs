//! What a control says beside itself once its request has gone: that it is working, that the daemon
//! has not made this method yet, or why it was refused. A control never claims to have worked
//! before the daemon says so; its effect shows when the state changes.

use std::future::Future;

use gpui::{Context, Div, div, prelude::*, px};
use x100_client::ClientError;
use x100_proto::ErrorCode;

use crate::theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Working,
    /// The method belongs to a later milestone. Carries the sentence to show.
    Unavailable(String),
    Failed(String),
}

impl Outcome {
    pub fn of(error: &ClientError) -> Outcome {
        match error {
            ClientError::Rpc(e) if e.code == ErrorCode::Unavailable => {
                match e.message.split_once("arrives with ") {
                    Some((_, milestone)) => Outcome::Unavailable(format!(
                        "Not available yet: it arrives with {milestone}."
                    )),
                    None => Outcome::Failed(e.message.clone()),
                }
            }
            ClientError::Rpc(e) => Outcome::Failed(capital(&e.message)),
            other => Outcome::Failed(capital(&other.to_string())),
        }
    }

    pub fn is_unavailable(&self) -> bool {
        matches!(self, Outcome::Unavailable(_))
    }
}

fn capital(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Sends a request from a view and hands its result back to the view, an error as an [`Outcome`].
pub fn run<V: 'static, T: 'static>(
    cx: &mut Context<V>,
    request: impl Future<Output = Result<T, ClientError>> + 'static,
    done: impl FnOnce(&mut V, Result<T, Outcome>, &mut Context<V>) + 'static,
) {
    cx.spawn(async move |this, cx| {
        let result = request.await.map_err(|e| Outcome::of(&e));
        let _ = this.update(cx, |view, cx| done(view, result, cx));
    })
    .detach();
}

/// The line a control shows beside itself.
pub fn line(outcome: &Outcome) -> Div {
    let (text, colour) = match outcome {
        Outcome::Working => ("Working…".to_owned(), theme::muted()),
        Outcome::Unavailable(t) => (t.clone(), theme::ask()),
        Outcome::Failed(t) => (t.clone(), theme::error()),
    };
    div()
        .text_size(px(theme::TINY))
        .text_color(colour)
        .child(text)
}
