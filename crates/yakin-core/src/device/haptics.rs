//! Haptic output reports of the 2026 Steam Controller: trackpad pulses and the rumble motors.
//! Layouts from hid-steam 7.3, `steam_haptic_pulse` and `steam_haptic_rumble`.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use std::time::Instant;

use log::trace;
use log::warn;

use crate::Result;
use crate::device::ibex;
use crate::state::Side;

pub const REPORT_LEN_PULSE: usize = 8;
pub const REPORT_LEN_RUMBLE: usize = 10;
pub const REPORT_LEN_TONE: usize = 10;

/// Least time between two rumble reports; Steam and the kernel both throttle to 20 Hz, and the
/// kernel repeats the last one at this rate while the motors run.
pub const RUMBLE_EVERY: Duration = Duration::from_millis(50);

/// One pulse train is this long; strength is the share of it the actuator is on.
// TODO Unverified: a guess at a felt duty cycle, tune on hardware
const PERIOD_US: u16 = 2000;

/// Least time between two pulses on one side.
pub const PULSE_GAP: Duration = Duration::from_millis(15);

/// `[0x81, side, on_us, off_us, repeat]`, le16 each. The report swaps left and right against
/// the 2015 convention: left is 1, right 0.
pub fn pulse_report(
    side: Side,
    on_us: u16,
    off_us: u16,
    repeat: u16,
) -> [u8; REPORT_LEN_PULSE] {
    let side = match side {
        Side::Left => 1,
        Side::Right => 0,
    };
    let [on_lo, on_hi] = on_us.to_le_bytes();
    let [off_lo, off_hi] = off_us.to_le_bytes();
    let [repeat_lo, repeat_hi] = repeat.to_le_bytes();
    [
        ibex::REPORT_ID_HAPTIC_PULSE,
        side,
        on_lo,
        on_hi,
        off_lo,
        off_hi,
        repeat_lo,
        repeat_hi,
    ]
}

/// `[0x83, side, gain dB, hz, ms, lfo hz, lfo depth]`, le16 each but side, gain and depth:
/// the actuator's own tone generator (SDL's `MsgHapticLfoTone`), without the LFO. Side as in
/// [`pulse_report`].
pub fn tone_report(
    side: Side,
    hz: u16,
    ms: u16,
) -> [u8; REPORT_LEN_TONE] {
    let side = match side {
        Side::Left => 1,
        Side::Right => 0,
    };
    let [hz_lo, hz_hi] = hz.to_le_bytes();
    let [ms_lo, ms_hi] = ms.to_le_bytes();
    [
        ibex::REPORT_ID_HAPTIC_TONE,
        side,
        0,
        hz_lo,
        hz_hi,
        ms_lo,
        ms_hi,
        0,
        0,
        0,
    ]
}

/// `[0x80, type, intensity, left speed, left gain, right speed, right gain]`, le16 each but the
/// gains. Left is the heavy motor; type, intensity and gains are what the kernel's force
/// feedback sends.
pub fn rumble_report(
    left: u16,
    right: u16,
) -> [u8; REPORT_LEN_RUMBLE] {
    let [left_lo, left_hi] = left.to_le_bytes();
    let [right_lo, right_hi] = right.to_le_bytes();
    [
        ibex::REPORT_ID_HAPTIC_RUMBLE,
        0,
        0,
        0,
        left_lo,
        left_hi,
        2,
        right_lo,
        right_hi,
        0,
    ]
}

/// On, off and repeat of a pulse train `ms` long at `strength` of 255.
pub fn pulse_timing(
    strength: u8,
    ms: u16,
) -> (u16, u16, u16) {
    let on = (PERIOD_US as u32 * strength as u32 / 255).max(1) as u16;
    let repeat = (ms as u32 * 1000 / PERIOD_US as u32).clamp(1, u16::MAX as u32) as u16;
    (on, PERIOD_US - on.min(PERIOD_US), repeat)
}

/// Length of each pulse of the lizard cue, at full strength.
const CUE_MS: u16 = 30;

/// When each pulse of the lizard cue starts, in ms: two going into lizard mode, one coming out.
pub fn cue_starts(on: bool) -> &'static [u64] {
    if on { &[0, 90] } else { &[0] }
}

/// Play the lizard cue on the right trackpad of the node at `path`, from a thread of its own so
/// the gap between pulses holds nobody up.
pub fn cue(
    path: &Path,
    on: bool,
) -> Result<()> {
    let mut haptics = Haptics::open(path)?;
    let path = path.to_owned();
    let _handle = std::thread::spawn(move || {
        if let Err(err) = haptics.cue(on) {
            warn!("{} cue: {err}", path.display());
        }
    });
    Ok(())
}

/// A write-only handle on a controller's hidraw node for its haptic reports.
pub struct Haptics {
    file: File,
}
impl Haptics {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::options().write(true).open(path)?;
        Ok(Self { file })
    }

    /// A pulse train on one trackpad.
    pub fn pulse(
        &mut self,
        side: Side,
        strength: u8,
        ms: u16,
    ) -> Result<()> {
        let (on, off, repeat) = pulse_timing(strength, ms);
        self.write(&pulse_report(side, on, off, repeat))
    }

    /// A tone on one trackpad, see [`tone_report`].
    pub fn tone(
        &mut self,
        side: Side,
        hz: u16,
        ms: u16,
    ) -> Result<()> {
        self.write(&tone_report(side, hz, ms))
    }

    /// The lizard cue (see [`cue_starts`]), blocking until its last pulse is sent.
    pub fn cue(
        &mut self,
        on: bool,
    ) -> Result<()> {
        let start = Instant::now();
        for at in cue_starts(on) {
            let at = Duration::from_millis(*at);
            std::thread::sleep(at.saturating_sub(start.elapsed()));
            self.pulse(Side::Right, 255, CUE_MS)?;
        }
        Ok(())
    }

    /// Both motors at once; 0 and 0 stops them.
    pub fn rumble(
        &mut self,
        left: u16,
        right: u16,
    ) -> Result<()> {
        self.write(&rumble_report(left, right))
    }

    fn write(
        &mut self,
        report: &[u8],
    ) -> Result<()> {
        trace!("haptic {report:02x?}");
        self.file.write_all(report)?;
        Ok(())
    }
}

/// At most one pulse per side every [`PULSE_GAP`].
#[derive(Clone, Debug, Default)]
pub struct Limiter {
    last: [Option<Instant>; 2],
}
impl Limiter {
    /// Whether a pulse on `side` may go out at `now`; if so, it counts as sent.
    pub fn allow(
        &mut self,
        side: Side,
        now: Instant,
    ) -> bool {
        let last = &mut self.last[side as usize];
        if last.is_some_and(|last| now < last + PULSE_GAP) {
            return false;
        }
        *last = Some(now);
        true
    }
}

/// One uploaded rumble effect: the heavy and light motor and how long it plays, 0 = until stopped.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Effect {
    pub strong: u16,
    pub weak: u16,
    pub length: Duration,
}

/// Force feedback rumble of the virtual pad, folded into motor speeds: the loudest of the
/// playing effects per motor, sent at most every [`RUMBLE_EVERY`] and repeated at that rate
/// while it runs.
#[derive(Clone, Debug, Default)]
pub struct Rumble {
    effects: BTreeMap<i16, Effect>,
    /// Playing effects and when each ends.
    playing: BTreeMap<i16, Option<Instant>>,
    sent: (u16, u16),
    sent_at: Option<Instant>,
}
impl Rumble {
    pub fn upload(
        &mut self,
        id: i16,
        effect: Effect,
    ) {
        self.effects.insert(id, effect);
    }

    pub fn erase(
        &mut self,
        id: i16,
    ) {
        self.effects.remove(&id);
        self.playing.remove(&id);
    }

    /// `EV_FF` with the effect's id: `count` above 0 plays it, 0 stops it.
    pub fn play(
        &mut self,
        id: i16,
        count: i32,
        now: Instant,
    ) {
        match self.effects.get(&id) {
            Some(effect) if count > 0 => {
                let end = (!effect.length.is_zero()).then(|| now + effect.length);
                self.playing.insert(id, end);
            }
            _ => {
                self.playing.remove(&id);
            }
        }
    }

    /// The motor speeds at `now`, ended effects dropped.
    pub fn level(
        &mut self,
        now: Instant,
    ) -> (u16, u16) {
        self.playing
            .retain(|_, end| end.is_none_or(|end| now < end));
        self.playing
            .keys()
            .filter_map(|id| self.effects.get(id))
            .fold((0, 0), |(strong, weak), effect| {
                (strong.max(effect.strong), weak.max(effect.weak))
            })
    }

    /// The speeds to send at `now`, if a change or a running motor is due.
    pub fn due(
        &mut self,
        now: Instant,
    ) -> Option<(u16, u16)> {
        let level = self.level(now);
        if level == self.sent && level == (0, 0) {
            return None;
        }
        if self.sent_at.is_some_and(|at| now < at + RUMBLE_EVERY) {
            return None;
        }
        self.sent = level;
        self.sent_at = Some(now);
        Some(level)
    }

    /// How long until [`Rumble::due`] may have something, at most `max`.
    pub fn wait(
        &self,
        now: Instant,
        max: Duration,
    ) -> Duration {
        let throttle = self
            .sent_at
            .map(|at| (at + RUMBLE_EVERY).saturating_duration_since(now));
        let end = self
            .playing
            .values()
            .flatten()
            .map(|end| end.saturating_duration_since(now))
            .min();
        let running = self.sent != (0, 0) || !self.playing.is_empty();
        [running.then_some(throttle).flatten(), end, Some(max)]
            .into_iter()
            .flatten()
            .min()
            .unwrap()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_cue_starts() {
        // Two pulses 60 ms apart into lizard mode, one out of it
        assert_eq!(cue_starts(true), &[0, 90]);
        assert_eq!(cue_starts(false), &[0]);
    }

    #[test]
    fn test_pulse_report() {
        // The kernel's mode-switch pulse: steam_haptic_pulse(STEAM_PAD_RIGHT, 0x190, 0, 1, 0)
        assert_eq!(
            pulse_report(Side::Right, 0x190, 0, 1),
            [0x81, 0, 0x90, 0x01, 0, 0, 1, 0]
        );
        // steam_haptic_pulse(STEAM_PAD_LEFT, 0x14D, 0x14D, 0x2D, 0)
        assert_eq!(
            pulse_report(Side::Left, 0x14d, 0x14d, 0x2d),
            [0x81, 1, 0x4d, 0x01, 0x4d, 0x01, 0x2d, 0]
        );
    }

    #[test]
    fn test_rumble_report() {
        // steam_haptic_rumble(steam, 0, left, right, 2, 0) from the force feedback callbacks
        assert_eq!(
            rumble_report(0x1234, 0xabcd),
            [0x80, 0, 0, 0, 0x34, 0x12, 2, 0xcd, 0xab, 0]
        );
        assert_eq!(rumble_report(0, 0), [0x80, 0, 0, 0, 0, 0, 2, 0, 0, 0]);
    }

    #[test]
    fn test_pulse_timing() {
        assert_eq!(pulse_timing(255, 10), (2000, 0, 5));
        assert_eq!(pulse_timing(51, 10), (400, 1600, 5));
        // Never silent, never zero repeats
        assert_eq!(pulse_timing(0, 0), (1, 1999, 1));
    }

    #[test]
    fn test_tone_report() {
        assert_eq!(
            tone_report(Side::Left, 1000, 200),
            [0x83, 1, 0, 0xe8, 0x03, 0xc8, 0, 0, 0, 0]
        );
        assert_eq!(tone_report(Side::Right, 0, 0)[1], 0);
    }

    #[test]
    fn test_limiter() {
        let mut limiter = Limiter::default();
        let now = Instant::now();
        assert!(limiter.allow(Side::Left, now));
        assert!(!limiter.allow(Side::Left, now + Duration::from_millis(14)));
        assert!(limiter.allow(Side::Right, now + Duration::from_millis(14)));
        assert!(limiter.allow(Side::Left, now + Duration::from_millis(15)));
        assert!(!limiter.allow(Side::Left, now + Duration::from_millis(29)));
    }

    #[test]
    fn test_rumble() {
        let ms = Duration::from_millis;
        let now = Instant::now();
        let mut rumble = Rumble::default();
        rumble.upload(
            0,
            Effect {
                strong: 0x8000,
                weak: 0x100,
                length: ms(200),
            },
        );
        rumble.upload(
            1,
            Effect {
                strong: 0x10,
                weak: 0x4000,
                length: Duration::ZERO,
            },
        );
        assert_eq!(rumble.due(now), None);
        rumble.play(0, 1, now);
        rumble.play(1, 1, now);
        assert_eq!(rumble.due(now), Some((0x8000, 0x4000)));
        // Throttled, then repeated while running
        assert_eq!(rumble.due(now + ms(49)), None);
        assert_eq!(rumble.due(now + ms(50)), Some((0x8000, 0x4000)));
        // Effect 0 ran out; 1 plays until stopped
        assert_eq!(rumble.due(now + ms(200)), Some((0x10, 0x4000)));
        rumble.play(1, 0, now + ms(210));
        assert_eq!(rumble.due(now + ms(220)), None);
        assert_eq!(rumble.due(now + ms(250)), Some((0, 0)));
        assert_eq!(rumble.due(now + ms(400)), None);
        // Erased effects do not play
        rumble.erase(1);
        rumble.play(1, 1, now + ms(500));
        assert_eq!(rumble.due(now + ms(500)), None);
    }

    #[test]
    fn test_rumble_wait() {
        let ms = Duration::from_millis;
        let now = Instant::now();
        let mut rumble = Rumble::default();
        assert_eq!(rumble.wait(now, ms(100)), ms(100));
        rumble.upload(
            0,
            Effect {
                strong: 1,
                weak: 1,
                length: ms(30),
            },
        );
        rumble.play(0, 1, now);
        assert_eq!(rumble.wait(now, ms(100)), ms(30));
        rumble.due(now);
        assert_eq!(rumble.wait(now + ms(10), ms(100)), ms(20));
    }
}
