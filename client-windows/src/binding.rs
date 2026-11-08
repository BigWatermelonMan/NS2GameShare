//! Physical controller capture, independent of the current game-button mapping.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Physical {
    pub buttons: u128,
    pub hat: u8,
    pub connected: bool,
    pub vendor: u16,
}
impl Physical {
    pub fn neutral(self) -> bool {
        self.buttons == 0 && self.hat == 0
    }
    pub fn merge(&mut self, other: Self) {
        if self.vendor == 0 || !other.neutral() {
            self.vendor = other.vendor;
        }
        self.buttons |= other.buttons;
        self.hat |= other.hat;
        self.connected |= other.connected;
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Button {
    Button(usize),
    Hat(usize),
}
#[derive(Default)]
pub struct Tracker {
    armed: bool,
    previous: Physical,
}
impl Tracker {
    pub fn sample(&mut self, current: Physical) -> Option<Button> {
        if !current.connected {
            self.armed = false;
            self.previous = current;
            return None;
        }
        if !self.armed {
            self.armed = current.neutral();
            self.previous = current;
            return None;
        }
        let buttons = current.buttons & !self.previous.buttons;
        let hat = current.hat & !self.previous.hat;
        self.previous = current;
        if current.buttons.count_ones() + current.hat.count_ones() != 1 {
            return None;
        }
        if buttons != 0 {
            Some(Button::Button(buttons.trailing_zeros() as usize))
        } else if hat != 0 {
            Some(Button::Hat(hat.trailing_zeros() as usize))
        } else {
            None
        }
    }
}
#[derive(Clone, Copy, Default)]
pub struct Snapshot {
    pub directinput: Physical,
    pub xinput: Physical,
}

pub fn direct_defaults(sony: bool) -> Vec<Option<u8>> {
    let table = if sony {
        [3, 1, 0, 2, 6, 7, 8, 9, 11, 10, 4, 5]
    } else {
        [1, 0, 3, 2, 6, 7, 8, 9, 11, 10, 4, 5]
    };
    (0..128).map(|i| table.get(i).copied()).collect()
}
pub fn assign(
    config: &mut crate::input::Config,
    xinput: bool,
    target: usize,
    source: Option<Button>,
    sony: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(target < 16, "Invalid game button");
    if xinput {
        anyhow::ensure!(
            source.is_none_or(|b| matches!(b,Button::Button(i) if i<16)),
            "Invalid XInput source"
        );
        let m = config
            .xinput_buttons
            .get_or_insert(crate::input::XINPUT_DEFAULT.map(Some));
        for v in m.iter_mut() {
            if *v == Some(target as u8) {
                *v = None;
            }
        }
        if let Some(Button::Button(i)) = source {
            m[i] = Some(target as u8);
        }
    } else {
        anyhow::ensure!(
            source.is_none_or(|b| match b {
                Button::Button(i) => i < 128,
                Button::Hat(i) => i < 4,
            }),
            "Invalid DirectInput source"
        );
        let m = config
            .directinput
            .buttons
            .get_or_insert_with(|| direct_defaults(sony));
        m.resize(128, None);
        for v in m
            .iter_mut()
            .chain(config.directinput.pov_buttons.iter_mut())
        {
            if *v == Some(target as u8) {
                *v = None;
            }
        }
        match source {
            Some(Button::Button(i)) => m[i] = Some(target as u8),
            Some(Button::Hat(i)) => config.directinput.pov_buttons[i] = Some(target as u8),
            None => {}
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_requires_release_and_single_new_press() {
        let mut t = Tracker::default();
        let mut p = Physical {
            buttons: 1,
            connected: true,
            ..Default::default()
        };
        assert_eq!(t.sample(p), None);
        p.buttons = 0;
        t.sample(p);
        p.buttons = 4;
        assert_eq!(t.sample(p), Some(Button::Button(2)));
        assert_eq!(t.sample(p), None);
        p.buttons = 0;
        t.sample(p);
        p.hat = 3;
        assert_eq!(t.sample(p), None);
    }
    #[test]
    fn assigning_hat_preserves_other_bindings_and_clear_is_reversible() {
        let mut c = crate::input::Config::default();
        assign(&mut c, false, 0, Some(Button::Hat(0)), true).unwrap();
        assert_eq!(c.directinput.pov_buttons[0], Some(0));
        assert_eq!(c.directinput.buttons.as_ref().unwrap()[2], None);
        assert_eq!(c.directinput.buttons.as_ref().unwrap()[1], Some(1));
        assign(&mut c, false, 0, None, true).unwrap();
        assert_eq!(c.directinput.pov_buttons[0], None);
        assign(&mut c, true, 0, Some(Button::Button(14)), false).unwrap();
        assert_eq!(c.xinput_buttons.unwrap()[14], Some(0));
    }
}
