//! A plain text box: typing, IME composition, the arrow keys, paste, and Enter to submit. GPUI
//! has no text field of its own, and Zed's editor is GPL, so the app keeps this small one. The
//! caret moves by character; there is no selection.

use std::ops::Range;

use gpui::{
    App, Bounds, Context, CursorStyle, ElementInputHandler, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, HighlightStyle, KeyBinding, Pixels, SharedString, StyledText,
    UTF16Selection, Window, actions, canvas, div, prelude::*, px,
};

use crate::theme;

actions!(
    text_input,
    [
        Backspace, Delete, Left, Right, Home, End, Paste, Submit, Newline
    ]
);

const CONTEXT: &str = "TextInput";

/// The keys a text box answers. Called once when the app starts.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        KeyBinding::new("left", Left, Some(CONTEXT)),
        KeyBinding::new("right", Right, Some(CONTEXT)),
        KeyBinding::new("home", Home, Some(CONTEXT)),
        KeyBinding::new("end", End, Some(CONTEXT)),
        KeyBinding::new("ctrl-v", Paste, Some(CONTEXT)),
        KeyBinding::new("cmd-v", Paste, Some(CONTEXT)),
        KeyBinding::new("enter", Submit, Some(CONTEXT)),
        KeyBinding::new("shift-enter", Newline, Some(CONTEXT)),
    ]);
}

pub enum InputEvent {
    /// Enter was pressed.
    Submit,
}

pub struct TextInput {
    focus: FocusHandle,
    text: String,
    /// A byte offset into `text`, always on a character boundary.
    cursor: usize,
    /// What an input method is composing, in bytes.
    marked: Option<Range<usize>>,
    placeholder: SharedString,
    multiline: bool,
    /// Drawn as dots, for a credential's value.
    masked: bool,
}

impl EventEmitter<InputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextInput {
    pub fn new(
        placeholder: impl Into<SharedString>,
        multiline: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            text: String::new(),
            cursor: 0,
            marked: None,
            placeholder: placeholder.into(),
            multiline,
            masked: false,
        }
    }

    /// Draws what is typed as dots. The text is still held and returned by [`Self::text`].
    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.marked = None;
        cx.notify();
    }

    fn insert(&mut self, range: Range<usize>, new: &str) {
        self.text.replace_range(range.clone(), new);
        self.cursor = range.start + new.len();
    }

    fn previous(&self, at: usize) -> usize {
        self.text[..at].char_indices().last().map_or(0, |(i, _)| i)
    }

    fn next(&self, at: usize) -> usize {
        self.text[at..]
            .chars()
            .next()
            .map_or(at, |c| at + c.len_utf8())
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.cursor > 0 {
            let start = self.previous(self.cursor);
            self.insert(start..self.cursor, "");
            cx.notify();
        }
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.cursor < self.text.len() {
            let end = self.next(self.cursor);
            self.insert(self.cursor..end, "");
            cx.notify();
        }
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.previous(self.cursor);
        cx.notify();
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.next(self.cursor);
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        cx.notify();
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i);
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let pasted = if self.multiline {
                pasted
            } else {
                pasted.replace('\n', " ")
            };
            self.insert(self.cursor..self.cursor, &pasted);
            cx.notify();
        }
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Submit);
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.insert(self.cursor..self.cursor, "\n");
            cx.notify();
        }
    }

    fn utf16_of(&self, offset: usize) -> usize {
        self.text[..offset].encode_utf16().count()
    }

    fn offset_of_utf16(&self, offset: usize) -> usize {
        let mut units = 0;
        for (i, c) in self.text.char_indices() {
            if units >= offset {
                return i;
            }
            units += c.len_utf16();
        }
        self.text.len()
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_of_utf16(range.start)..self.offset_of_utf16(range.end)
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range);
        adjusted.replace(self.utf16_of(range.start)..self.utf16_of(range.end));
        Some(self.text[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let at = self.utf16_of(self.cursor);
        Some(UTF16Selection {
            range: at..at,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|r| self.utf16_of(r.start)..self.utf16_of(r.end))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.range_from_utf16(&r))
            .or(self.marked.clone())
            .unwrap_or(self.cursor..self.cursor);
        let new = if self.multiline {
            new.to_owned()
        } else {
            new.replace('\n', " ")
        };
        self.insert(range, &new);
        self.marked = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.range_from_utf16(&r))
            .or(self.marked.clone())
            .unwrap_or(self.cursor..self.cursor);
        self.insert(range.clone(), new);
        self.marked = (!new.is_empty()).then(|| range.start..range.start + new.len());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // The box itself: an input method places its window near it, which is close enough
        // without laying out each character.
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let empty = self.text.is_empty();
        // The caret is a character drawn into the text, so a wrapped line keeps it in place.
        let (shown, highlights) = if empty {
            (
                format!("{}{}", if focused { "|" } else { "" }, self.placeholder),
                if focused {
                    vec![(0..1, caret())]
                } else {
                    Vec::new()
                },
            )
        } else {
            let (mut shown, at) = if self.masked {
                let before = self.text[..self.cursor].chars().count();
                ("\u{2022}".repeat(self.text.chars().count()), before * 3)
            } else {
                (self.text.clone(), self.cursor)
            };
            if focused {
                shown.insert(at, '|');
                (shown, vec![(at..at + 1, caret())])
            } else {
                (shown, Vec::new())
            }
        };
        let entity = cx.entity();
        let focus = self.focus.clone();
        div()
            .id("text-input")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::newline))
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.focus, cx);
            }))
            .relative()
            .w_full()
            .min_h(px(20.))
            .text_size(px(theme::TEXT))
            .line_height(px(20.))
            .text_color(if empty { theme::dim() } else { theme::text() })
            .when(!self.multiline, |d| d.whitespace_nowrap().overflow_hidden())
            .child(StyledText::new(shown).with_highlights(highlights))
            .child(
                // Registers this box with the platform's text input while it is painted.
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                    },
                )
                .absolute()
                .size_full(),
            )
    }
}

fn caret() -> HighlightStyle {
    HighlightStyle {
        color: Some(theme::text().into()),
        ..Default::default()
    }
}
