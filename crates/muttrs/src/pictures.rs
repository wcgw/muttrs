//! The Pictures on screen: prepared off the UI thread, sent to the terminal between draws, kept while they're
//! wanted, and forgotten once they're not, freeing the terminal's memory.

use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Arc;

use muttrs_gfx::{CellSize, Ids, Picture, Prepared, Protocol};
use tokio::sync::mpsc;

use crate::ui::Event;

/// Which Picture: one image of one opened Message, at one size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    /// The opened Message, as [`crate::pager::Pager::id`] says.
    pub(crate) message: u64,
    pub(crate) image: usize,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
}

enum Entry {
    Preparing,
    Ready(Picture),
    Broken,
}

pub(crate) struct Pictures {
    protocol: Option<Protocol>,
    cell: CellSize,
    ids: Arc<Ids>,
    entries: HashMap<Key, Entry>,
    /// Pictures no longer wanted, to forget between draws.
    unwanted: Vec<Picture>,
    events: mpsc::UnboundedSender<Event>,
}

impl Pictures {
    /// Pictures drawn as `protocol` says, on cells of `cell` pixels; none if `protocol` is `None`.
    pub(crate) fn new(
        protocol: Option<Protocol>,
        cell: CellSize,
        events: mpsc::UnboundedSender<Event>,
    ) -> Self {
        Self {
            protocol,
            cell,
            ids: Arc::new(Ids::default()),
            entries: HashMap::new(),
            unwanted: Vec::new(),
            events,
        }
    }

    /// Whether Pictures are drawn through kitty's placeholders, which live in the terminal's memory.
    pub(crate) fn is_kitty(&self) -> bool {
        self.protocol == Some(Protocol::Kitty)
    }

    /// Whether Pictures are drawn at all.
    pub(crate) fn enabled(&self) -> bool {
        self.protocol.is_some()
    }

    pub(crate) fn cell(&self) -> CellSize {
        self.cell
    }

    /// Notes the terminal's cells are now `cell` pixels: Pictures already sized for cells stay as they are.
    pub(crate) fn set_cell(&mut self, cell: CellSize) {
        self.cell = cell;
    }

    /// The Picture `key` names, if it's ready; if it's not asked for yet, it's prepared from `bytes` in the
    /// background, and [`Event::Picture`] says when it's ready.
    pub(crate) fn get(
        &mut self,
        key: Key,
        bytes: impl FnOnce() -> Option<Arc<Vec<u8>>>,
    ) -> Option<&Picture> {
        let protocol = self.protocol?;
        if !self.entries.contains_key(&key) {
            let Some(bytes) = bytes() else {
                self.entries.insert(key, Entry::Broken);
                return None;
            };
            self.entries.insert(key, Entry::Preparing);
            let (cell, ids, events) = (self.cell, Arc::clone(&self.ids), self.events.clone());
            tokio::task::spawn_blocking(move || {
                let prepared =
                    muttrs_gfx::prepare(&bytes, key.cols, key.rows, cell, protocol, &ids)
                        .map_err(|error| error.to_string());
                let _ = events.send(Event::Picture(key, prepared));
            });
        }
        match self.entries.get(&key) {
            Some(Entry::Ready(picture)) => Some(picture),
            _ => None,
        }
    }

    /// Takes a Picture that's been prepared, sending it to the terminal: call it between draws.
    ///
    /// # Errors
    ///
    /// If writing to the terminal fails.
    pub(crate) fn prepared(
        &mut self,
        key: Key,
        prepared: Result<Prepared, String>,
        out: &mut impl Write,
    ) -> io::Result<()> {
        // A Picture no longer wanted while it was prepared isn't sent.
        if !matches!(self.entries.get(&key), Some(Entry::Preparing)) {
            return Ok(());
        }
        let entry = match prepared {
            Ok(prepared) => Entry::Ready(prepared.send(out)?),
            Err(error) => {
                tracing::debug!(%error, ?key, "a Picture couldn't be prepared");
                Entry::Broken
            }
        };
        self.entries.insert(key, entry);
        Ok(())
    }

    /// Keeps only the Pictures of `message` that `wanted` says, and those of other Messages; when `message` is
    /// `None`, only those `wanted` says.
    pub(crate) fn keep(&mut self, message: Option<u64>, wanted: impl Fn(&Key) -> bool) {
        let unwanted: Vec<Key> = self
            .entries
            .keys()
            .filter(|key| message.is_none_or(|message| key.message == message) && !wanted(key))
            .copied()
            .collect();
        for key in unwanted {
            if let Some(Entry::Ready(picture)) = self.entries.remove(&key) {
                self.unwanted.push(picture);
            }
        }
    }

    /// Forgets the Pictures no longer wanted, in the terminal: call it between draws.
    ///
    /// # Errors
    ///
    /// If writing to the terminal fails.
    pub(crate) fn forget_unwanted(&mut self, out: &mut impl Write) -> io::Result<()> {
        for picture in self.unwanted.drain(..) {
            picture.forget(out)?;
        }
        Ok(())
    }
}
