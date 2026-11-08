//! The one trait between core and the OS, and the run loop that drives `Wm` with it.

use crate::{Command, Event, Wm};

pub trait Platform {
    /// Blocks until the next event. Returns `None` when the platform shuts down.
    fn next_event(&mut self) -> Option<Event>;
    fn execute(&mut self, command: Command);
}

/// Feeds every platform event to a fresh `Wm` and executes the commands it returns, in order,
/// followed by the menu bar row when that event changed it.
pub fn run(platform: &mut impl Platform) {
    let mut wm = Wm::new();
    while let Some(event) = platform.next_event() {
        for command in wm.handle(event) {
            platform.execute(command);
        }
        // The menu bar row goes out after the event's own commands, only when it changed (BAR-3).
        if let Some(row) = wm.take_row_change() {
            platform.execute(Command::ShowWorkspaces(row));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;
    use crate::{Command, Event, PointerTarget, WorkspaceState, WorkspaceStatus};

    #[test]
    fn run_executes_commands_from_handle() {
        let mut fake = FakePlatform::new(vec![
            Event::MonitorsChanged(vec![main_monitor()]),
            Event::WindowsDiscovered(vec![normal(1)]),
        ]);
        run(&mut fake);
        let without_row: Vec<_> = fake
            .executed
            .into_iter()
            .filter(|c| !matches!(c, Command::ShowWorkspaces(_)))
            .collect();
        assert_eq!(
            without_row,
            vec![Command::SetFrame {
                id: w(1),
                frame: r(5., 5., 990., 790.)
            }]
        );
    }

    fn row_reports(executed: &[Command]) -> usize {
        executed
            .iter()
            .filter(|c| matches!(c, Command::ShowWorkspaces(_)))
            .count()
    }

    #[test]
    fn bar_3_run_reports_row_after_the_events_commands() {
        let mut fake = FakePlatform::new(vec![
            Event::MonitorsChanged(vec![main_monitor()]),
            Event::WindowsDiscovered(vec![normal(1)]),
        ]);
        run(&mut fake);
        // MonitorsChanged alone emits no commands, so its row report comes first; the
        // discovered window doesn't change the row, so it isn't reported again.
        assert_eq!(
            fake.executed[0],
            Command::ShowWorkspaces(vec![WorkspaceStatus {
                number: ws(1),
                state: WorkspaceState::Focused
            }])
        );
        assert_eq!(row_reports(&fake.executed), 1);
    }

    #[test]
    fn bar_3_run_reports_row_only_when_changed() {
        let mut fake = FakePlatform::new(vec![
            Event::MonitorsChanged(vec![main_monitor()]),
            Event::PointerMoved {
                position: p(10., 10.),
                target: PointerTarget::Other,
            },
        ]);
        run(&mut fake);
        assert_eq!(row_reports(&fake.executed), 1);
    }
}
