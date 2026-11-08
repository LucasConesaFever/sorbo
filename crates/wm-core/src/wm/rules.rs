//! Window rules (RULE-2, RULE-4): `WindowInfo.rule` decides whether a new window floats.

#[cfg(test)]
mod tests {
    use crate::testing::*;
    use crate::*;

    fn ruled(id: u64, kind: WindowKind, floating: bool) -> WindowInfo {
        let mut info = window(id, 100 + id as i32, kind, r(100., 100., 300., 200.));
        info.rule = RuleEffect {
            floating: Some(floating),
        };
        info
    }

    #[test]
    fn rule_2_floating_true_floats_a_new_window() {
        let mut wm = started(vec![main_monitor()], vec![]);
        wm.handle(Event::WindowCreated(ruled(1, WindowKind::Normal, true)));
        let window = wm.window(w(1)).unwrap();
        assert!(window.floating);
        assert_eq!(window.floating_frame, Some(r(100., 100., 300., 200.)));
        assert_eq!(wm.focused_window(), Some(w(1)));
    }

    #[test]
    fn rule_2_floating_false_tiles_a_floating_kind() {
        let mut wm = started(vec![main_monitor()], vec![]);
        let cmds = wm.handle(Event::WindowCreated(ruled(1, WindowKind::Floating, false)));
        assert!(!wm.window(w(1)).unwrap().floating);
        assert!(cmds.contains(&Command::SetFrame {
            id: w(1),
            frame: r(5., 5., 990., 790.)
        }));
    }

    #[test]
    fn rule_2_discovered_window_floats() {
        let wm = started(
            vec![main_monitor()],
            vec![normal(1), ruled(2, WindowKind::Normal, true)],
        );
        assert!(!wm.window(w(1)).unwrap().floating);
        assert!(wm.window(w(2)).unwrap().floating);
    }

    #[test]
    fn rule_4_popup_ignores_rule() {
        let mut wm = started(vec![main_monitor()], vec![normal(1)]);
        let focused = wm.focused_window();
        let cmds = wm.handle(Event::WindowCreated(ruled(2, WindowKind::Popup, true)));
        assert!(!wm.window(w(2)).unwrap().floating);
        assert_eq!(wm.focused_window(), focused);
        assert_eq!(cmds, vec![]);
    }
}
