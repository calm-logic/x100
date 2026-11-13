//! The loop: keys, messages from background work and changes of the client's state, each applied
//! and followed by a redraw.

use std::time::Duration;

use ratatui::crossterm::event::{self, Event};
use tokio::sync::{broadcast, mpsc};
use x100_client::{Change, Client};

use crate::app::App;
use crate::msg::Msg;
use crate::remember::Remember;

/// An interface on a client: the state of the screen, and the streams that change it.
pub struct Tui {
    pub app: App,
    msgs: mpsc::UnboundedReceiver<Msg>,
    changes: broadcast::Receiver<Change>,
}

impl Tui {
    pub fn new(client: Client, handle: tokio::runtime::Handle, remember: Remember) -> Tui {
        let (tx, msgs) = mpsc::unbounded_channel();
        let changes = client.changes();
        Tui {
            app: App::new(client, handle, tx, remember),
            msgs,
            changes,
        }
    }

    /// Applies everything that has arrived, without waiting. True if anything did.
    pub fn drain(&mut self) -> bool {
        let mut any = false;
        loop {
            match self.changes.try_recv() {
                Ok(change) => self.app.on_change(&change),
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    self.app.on_change(&Change::Reset)
                }
                Err(_) => break,
            }
            any = true;
        }
        while let Ok(msg) = self.msgs.try_recv() {
            self.app.on_msg(msg);
            any = true;
        }
        any
    }

    /// Waits for the next change or message and applies it.
    pub async fn next(&mut self) {
        tokio::select! {
            change = self.changes.recv() => match change {
                Ok(c) => self.app.on_change(&c),
                Err(broadcast::error::RecvError::Lagged(_)) => self.app.on_change(&Change::Reset),
                Err(broadcast::error::RecvError::Closed) => std::future::pending().await,
            },
            Some(msg) = self.msgs.recv() => self.app.on_msg(msg),
        }
        self.drain();
    }
}

/// Runs the full screen until the builder quits.
pub async fn run(client: Client, remember: Remember) -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::EnableBracketedPaste
    )?;
    let result = event_loop(&mut terminal, client, remember).await;
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::DisableBracketedPaste
    );
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    client: Client,
    remember: Remember,
) -> std::io::Result<()> {
    let (keys_tx, mut keys) = mpsc::unbounded_channel();
    // The terminal's events are read on a thread of their own: reading blocks.
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if keys_tx.send(ev).is_err() {
                return;
            }
        }
    });
    let mut tui = Tui::new(client, tokio::runtime::Handle::current(), remember);
    loop {
        terminal.draw(|f| tui.app.draw(f))?;
        if tui.app.quit {
            return Ok(());
        }
        tokio::select! {
            Some(ev) = keys.recv() => {
                match ev {
                    Event::Key(k) => tui.app.key(k),
                    Event::Paste(text) => tui.app.paste(&text),
                    _ => {}
                }
                // Typing is answered before anything else arrives.
                while let Ok(ev) = keys.try_recv() {
                    match ev {
                        Event::Key(k) => tui.app.key(k),
                        Event::Paste(text) => tui.app.paste(&text),
                        _ => {}
                    }
                }
            }
            () = tui.next() => {}
            () = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
        tui.drain();
    }
}
