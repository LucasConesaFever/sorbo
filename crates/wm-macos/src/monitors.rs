//! Monitor identity and coordinate conversion (spec 5.1).

use std::collections::HashMap;

use wm_core::{MonitorId, Rect};

/// Turns display UUID strings into monitor IDs, in the same order. Two monitors that report
/// the same UUID (identical models without serial numbers) get `#2`, `#3`, ... appended to
/// the repeats, in display-list order.
pub fn unique_ids(uuids: &[String]) -> Vec<MonitorId> {
    let mut seen: HashMap<&str, u32> = HashMap::new();
    uuids
        .iter()
        .map(|uuid| {
            let count = seen.entry(uuid.as_str()).or_insert(0);
            *count += 1;
            if *count == 1 {
                MonitorId(uuid.clone())
            } else {
                MonitorId(format!("{uuid}#{count}"))
            }
        })
        .collect()
}

/// What CoreGraphics tells us about one connected display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayKeys {
    pub vendor: u32,
    pub model: u32,
    pub serial: u32,
    /// `CGDisplayCreateUUIDFromDisplayID`, the fallback identity.
    pub uuid: String,
}

/// One display's entry in the IOKit registry: the same vendor/model/serial numbers, plus
/// the port it's plugged into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortEntry {
    pub vendor: u32,
    pub model: u32,
    pub serial: u32,
    pub port: u32,
}

/// Monitor IDs by port (MON-2): a display matched to a registry entry (same vendor, model
/// and serial) is `port-<PortID>`; otherwise its UUID. Displays with the same numbers are
/// paired with entries in order. Duplicate IDs get `#2`, `#3`, ... appended.
pub fn monitor_ids(displays: &[DisplayKeys], ports: &[PortEntry]) -> Vec<MonitorId> {
    let mut used = vec![false; ports.len()];
    let ids: Vec<String> = displays
        .iter()
        .map(|d| {
            let found = ports.iter().enumerate().position(|(i, p)| {
                !used[i] && p.vendor == d.vendor && p.model == d.model && p.serial == d.serial
            });
            match found {
                Some(i) => {
                    used[i] = true;
                    format!("port-{}", ports[i].port)
                }
                None => d.uuid.clone(),
            }
        })
        .collect();
    unique_ids(&ids)
}

/// Converts an AppKit frame (origin bottom-left of the main display, y up) into core's
/// coordinates (origin top-left of the main display, y down).
pub fn to_top_left(frame: Rect, main_height: f64) -> Rect {
    Rect::new(
        frame.x,
        main_height - (frame.y + frame.height),
        frame.width,
        frame.height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mon_2_duplicate_uuids_get_suffix() {
        let ids = unique_ids(&["A".into(), "B".into(), "A".into(), "A".into()]);
        assert_eq!(
            ids,
            vec![
                MonitorId::from("A"),
                MonitorId::from("B"),
                MonitorId::from("A#2"),
                MonitorId::from("A#3")
            ]
        );
    }

    fn display(vendor: u32, model: u32, serial: u32, uuid: &str) -> DisplayKeys {
        DisplayKeys {
            vendor,
            model,
            serial,
            uuid: uuid.to_string(),
        }
    }

    fn port(vendor: u32, model: u32, serial: u32, port: u32) -> PortEntry {
        PortEntry {
            vendor,
            model,
            serial,
            port,
        }
    }

    #[test]
    fn mon_2_external_identified_by_port() {
        // Built-in (no PortID) + the AOC on HDMI, values measured on the author's Mac.
        let displays = [
            display(1552, 41053, 4251086178, "UUID-BUILTIN"),
            display(1507, 9217, 75497, "UUID-AOC"),
        ];
        let ports = [port(1507, 9217, 75497, 48)];
        assert_eq!(
            monitor_ids(&displays, &ports),
            vec![MonitorId::from("UUID-BUILTIN"), MonitorId::from("port-48")]
        );
    }

    #[test]
    fn mon_2_different_monitor_on_same_port_is_same_monitor() {
        let displays = [display(4268, 1234, 999, "UUID-OTHER")];
        let ports = [port(4268, 1234, 999, 48)];
        assert_eq!(
            monitor_ids(&displays, &ports),
            vec![MonitorId::from("port-48")]
        );
    }

    #[test]
    fn mon_2_no_port_falls_back_to_uuid() {
        let displays = [display(1507, 9217, 75497, "UUID-AOC")];
        assert_eq!(
            monitor_ids(&displays, &[]),
            vec![MonitorId::from("UUID-AOC")]
        );
    }

    #[test]
    fn mon_2_identical_models_get_their_own_ports() {
        // Same model, no serial numbers: paired in order, each keeps its own port.
        let displays = [display(1, 2, 0, "UUID-SAME"), display(1, 2, 0, "UUID-SAME")];
        let ports = [port(1, 2, 0, 0), port(1, 2, 0, 32)];
        assert_eq!(
            monitor_ids(&displays, &ports),
            vec![MonitorId::from("port-0"), MonitorId::from("port-32")]
        );
    }

    #[test]
    fn visible_frame_converts_to_top_left() {
        // Built-in 1728x1117 with a 33-point menu bar: AppKit visibleFrame is (0, 0, 1728, 1084).
        assert_eq!(
            to_top_left(Rect::new(0.0, 0.0, 1728.0, 1084.0), 1117.0),
            Rect::new(0.0, 33.0, 1728.0, 1084.0)
        );
    }
}
