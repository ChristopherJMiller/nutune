//! TUI-aware logging utilities
//!
//! When the TUI is active, we need to suppress stdout/stderr output to prevent
//! corrupting the terminal display. This module provides utilities for TUI-safe logging.

use std::sync::atomic::{AtomicBool, Ordering};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// Global flag to indicate whether TUI mode is active
static TUI_MODE: AtomicBool = AtomicBool::new(false);

/// Set TUI mode on or off
pub fn set_tui_mode(enabled: bool) {
    TUI_MODE.store(enabled, Ordering::SeqCst);
}

/// Check if TUI mode is active
pub fn is_tui_mode() -> bool {
    TUI_MODE.load(Ordering::SeqCst)
}

/// A conditional layer that only logs when TUI mode is NOT active
pub struct ConditionalStderrLayer<L> {
    inner: L,
}

impl<L> ConditionalStderrLayer<L> {
    pub fn new(inner: L) -> Self {
        Self { inner }
    }
}

impl<S, L> Layer<S> for ConditionalStderrLayer<L>
where
    S: tracing::Subscriber,
    L: Layer<S>,
{
    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        // Only pass through to inner layer if TUI mode is NOT active
        if !is_tui_mode() {
            self.inner.on_event(event, ctx);
        }
    }

    fn on_enter(&self, id: &tracing::span::Id, ctx: Context<'_, S>) {
        if !is_tui_mode() {
            self.inner.on_enter(id, ctx);
        }
    }

    fn on_exit(&self, id: &tracing::span::Id, ctx: Context<'_, S>) {
        if !is_tui_mode() {
            self.inner.on_exit(id, ctx);
        }
    }
}
