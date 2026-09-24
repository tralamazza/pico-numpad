//! Single-destination keyboard routing with release-to-arm on handover.

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Destination {
    #[default]
    None,
    Usb,
    Ble,
}

#[derive(Default)]
pub struct Router {
    destination: Destination,
    blocked: u16,
}

impl Router {
    /// Returns (USB keys, BLE keys). Held keys never cross to a new host.
    pub fn update(&mut self, keys: u16, usb: bool, ble: bool) -> (u16, u16) {
        let destination = if usb {
            Destination::Usb
        } else if ble {
            Destination::Ble
        } else {
            Destination::None
        };
        if destination != self.destination {
            self.blocked = keys;
            self.destination = destination;
        }
        self.blocked &= keys;
        let keys = keys & !self.blocked;
        match destination {
            Destination::Usb => (keys, 0),
            Destination::Ble => (0, keys),
            Destination::None => (0, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usb_wins_without_duplicate_ble_input() {
        let mut router = Router::default();
        router.update(0, true, true);
        assert_eq!(router.update(1, true, true), (1, 0));
        assert_eq!(router.update(0, true, true), (0, 0));
    }

    #[test]
    fn handover_releases_old_host_and_requires_release_on_new_host() {
        let mut router = Router::default();
        router.update(0, false, true);
        assert_eq!(router.update(1, false, true), (0, 1));
        assert_eq!(router.update(1, true, true), (0, 0));
        assert_eq!(router.update(3, true, true), (2, 0));
        router.update(0, true, true);
        assert_eq!(router.update(1, true, true), (1, 0));
        assert_eq!(router.update(1, false, true), (0, 0));
        router.update(0, false, true);
        assert_eq!(router.update(1, false, true), (0, 1));
    }

    #[test]
    fn disconnected_input_is_not_replayed_on_connect() {
        let mut router = Router::default();
        assert_eq!(router.update(1, false, false), (0, 0));
        assert_eq!(router.update(1, true, false), (0, 0));
        router.update(0, true, false);
        assert_eq!(router.update(1, true, false), (1, 0));
    }
}
