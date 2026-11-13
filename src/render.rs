//! Kernel rendering extension point.
//!
//! A registered [`RenderingHook`] is the kernel's standardized interface for
//! rendering extensions: it observes or decorates the display list before the
//! kernel hands it to the renderer. Without a hook, or with a hook that keeps
//! the default methods, the list is the parsed IR tree exactly, so the
//! default path is a byte-identical no-op.

use crate::ir::Node;

/// Rendering extension point for the kernel display list.
///
/// Both methods are pure functions of the list, so the same IR always
/// produces the same output. The defaults leave the parsed tree untouched, so
/// a hook implements only the observation or decoration it needs.
pub trait RenderingHook: Send + Sync + std::fmt::Debug {
    /// Observes the display list before it is handed to the renderer. The
    /// default does nothing.
    fn observe(&self, _list: &Node) {}

    /// Returns the display list to hand to the renderer; the default returns
    /// the observed list unchanged.
    fn decorate(&self, list: &Node) -> Node {
        list.clone()
    }
}
