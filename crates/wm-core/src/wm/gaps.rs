//! `GapsChanged` (CFG-4): new gaps from the config file.

use super::Wm;
use crate::Gaps;

impl Wm {
    /// Only stores the gaps; reconcile re-lays out and emits the frames that changed.
    pub(super) fn on_gaps_changed(&mut self, gaps: Gaps) {
        self.gaps = gaps;
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::*;
    use crate::*;

    #[test]
    fn cfg_4_gaps_changed_relays_out() {
        let mut wm = started(vec![main_monitor()], vec![normal(1)]);
        assert_eq!(
            wm.handle(Event::GapsChanged(Gaps {
                outer: 10.0,
                inner: 4.0
            })),
            vec![Command::SetFrame {
                id: w(1),
                frame: r(10., 10., 980., 780.)
            }]
        );
    }

    #[test]
    fn cfg_4_same_gaps_emit_nothing() {
        let mut wm = started(vec![main_monitor()], vec![normal(1)]);
        assert_eq!(wm.handle(Event::GapsChanged(Gaps::DEFAULT)), vec![]);
    }

    #[test]
    fn cfg_1_gaps_before_monitors_apply_to_first_layout() {
        let mut wm = Wm::new();
        wm.handle(Event::GapsChanged(Gaps {
            outer: 10.0,
            inner: 4.0,
        }));
        wm.handle(Event::MonitorsChanged(vec![main_monitor()]));
        let commands = wm.handle(Event::WindowsDiscovered(vec![normal(1)]));
        assert!(commands.contains(&Command::SetFrame {
            id: w(1),
            frame: r(10., 10., 980., 780.)
        }));
    }
}
