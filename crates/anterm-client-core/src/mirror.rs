use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use alacritty_terminal::event::Event as AlacEvent;
use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::term::{Term, TermMode};

/// Lock-free atomic snapshot of lightweight terminal state published whenever
/// `Term` mutates (at the end of each reader feed/resize/reset and after UI
/// scroll/selection mutations).
///
/// UI hot paths (`render`, `prepaint`, `paint`, `mouse_move`, keymap queries,
/// composer gating, search rebasing) read from this mirror instead of blocking
/// on `FairMutex<Term>` while an agent is streaming output.
pub struct TermStateMirror {
    grid_revision: AtomicU64,
    mode_bits: AtomicU32,
    display_offset: AtomicUsize,
    history_size: AtomicUsize,
    cols: AtomicUsize,
    rows: AtomicUsize,
    has_selection: AtomicBool,
}

impl TermStateMirror {
    pub fn from_term<T>(term: &Term<T>) -> Self {
        let grid = term.grid();
        Self {
            grid_revision: AtomicU64::new(1),
            mode_bits: AtomicU32::new(term.mode().bits()),
            display_offset: AtomicUsize::new(grid.display_offset()),
            history_size: AtomicUsize::new(grid.history_size()),
            cols: AtomicUsize::new(term.columns()),
            rows: AtomicUsize::new(term.screen_lines()),
            has_selection: AtomicBool::new(term.selection.is_some()),
        }
    }

    /// Updates all mirrored fields from `term` and bumps `grid_revision`.
    #[inline]
    pub fn publish<T>(&self, term: &Term<T>) {
        self.sync_from_term(term);
        self.grid_revision.fetch_add(1, Ordering::AcqRel);
    }

    /// Refreshes mirrored fields from `term` without bumping `grid_revision`.
    #[inline]
    pub fn sync_from_term<T>(&self, term: &Term<T>) {
        let grid = term.grid();
        self.mode_bits.store(term.mode().bits(), Ordering::Release);
        self.display_offset
            .store(grid.display_offset(), Ordering::Release);
        self.history_size
            .store(grid.history_size(), Ordering::Release);
        self.cols.store(term.columns(), Ordering::Release);
        self.rows.store(term.screen_lines(), Ordering::Release);
        self.has_selection
            .store(term.selection.is_some(), Ordering::Release);
    }

    #[inline]
    pub fn store_mode(&self, mode: TermMode) {
        self.mode_bits.store(mode.bits(), Ordering::Release);
    }

    #[inline]
    pub fn store_has_selection(&self, has: bool) {
        self.has_selection.store(has, Ordering::Release);
    }

    #[inline]
    pub fn store_history_size(&self, history: usize) {
        self.history_size.store(history, Ordering::Release);
    }

    #[inline]
    pub fn grid_revision(&self) -> u64 {
        self.grid_revision.load(Ordering::Acquire)
    }

    #[inline]
    pub fn mode(&self) -> TermMode {
        TermMode::from_bits_truncate(self.mode_bits.load(Ordering::Acquire))
    }

    #[inline]
    pub fn alt_screen(&self) -> bool {
        self.mode().contains(TermMode::ALT_SCREEN)
    }

    #[inline]
    pub fn display_offset(&self) -> usize {
        self.display_offset.load(Ordering::Acquire)
    }

    #[inline]
    pub fn history_size(&self) -> usize {
        self.history_size.load(Ordering::Acquire)
    }

    #[inline]
    pub fn cols(&self) -> usize {
        self.cols.load(Ordering::Acquire)
    }

    #[inline]
    pub fn rows(&self) -> usize {
        self.rows.load(Ordering::Acquire)
    }

    #[inline]
    pub fn has_selection(&self) -> bool {
        self.has_selection.load(Ordering::Acquire)
    }
}

/// Receiving end of an event proxy that clears the 1-in-flight wakeup
/// coalescing flag as soon as an [`AlacEvent::Wakeup`] is dequeued.
#[derive(Clone)]
pub struct EventReceiver {
    rx: smol::channel::Receiver<AlacEvent>,
    wakeup_queued: Arc<AtomicBool>,
}

impl EventReceiver {
    pub fn new(rx: smol::channel::Receiver<AlacEvent>, wakeup_queued: Arc<AtomicBool>) -> Self {
        Self { rx, wakeup_queued }
    }

    pub async fn recv(&self) -> Result<AlacEvent, smol::channel::RecvError> {
        let ev = self.rx.recv().await?;
        if matches!(ev, AlacEvent::Wakeup) {
            self.wakeup_queued.store(false, Ordering::Release);
        }
        Ok(ev)
    }

    pub fn try_recv(&self) -> Result<AlacEvent, smol::channel::TryRecvError> {
        let ev = self.rx.try_recv()?;
        if matches!(ev, AlacEvent::Wakeup) {
            self.wakeup_queued.store(false, Ordering::Release);
        }
        Ok(ev)
    }

    pub fn clear_wakeup_flag(&self) {
        self.wakeup_queued.store(false, Ordering::Release);
    }
}
