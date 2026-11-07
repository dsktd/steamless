//! Report decoding for the 2026 Steam Controller ("Ibex").

use anyhow::anyhow;

use crate::Result;
use crate::state::Battery;
use crate::state::Buttons;
use crate::state::Charge;
use crate::state::ControllerState;
use crate::state::Imu;
use crate::state::Stick;
use crate::state::Trackpad;

pub const VENDOR: u16 = 0x28de;
pub const PRODUCT_BLE: u16 = 0x1303;
pub const PRODUCT_PUCK: u16 = 0x1304;
pub const PRODUCT_WIRED: u16 = 0x1305;
/// `BUS_USB` and `BUS_BLUETOOTH` from `linux/input.h`.
pub const BUS_USB: u32 = 0x03;
pub const BUS_BLUETOOTH: u32 = 0x05;
/// Every (bustype, product) that is a 2026 controller: the puck's slots, the cable, Bluetooth LE.
pub const PRODUCTS: [(u32, u16); 3] = [
    (BUS_USB, PRODUCT_PUCK),
    (BUS_USB, PRODUCT_WIRED),
    (BUS_BLUETOOTH, PRODUCT_BLE),
];

pub const REPORT_ID_FEATURES: u8 = 0x01;
pub const REPORT_ID_INPUT: u8 = 0x42;
pub const REPORT_ID_BATTERY: u8 = 0x43;
pub const REPORT_ID_INPUT2: u8 = 0x45;
pub const REPORT_ID_WIRELESS: u8 = 0x79;
pub const REPORT_ID_HAPTIC_RUMBLE: u8 = 0x80;
pub const REPORT_ID_HAPTIC_PULSE: u8 = 0x81;
pub const REPORT_ID_HAPTIC_TONE: u8 = 0x83;

pub const REPORT_LEN_INPUT: usize = 54;
pub const REPORT_LEN_INPUT2: usize = 46;
pub const REPORT_LEN_BATTERY: usize = 15;
pub const REPORT_LEN_WIRELESS: usize = 2;

pub const ID_CLEAR_DIGITAL_MAPPINGS: u8 = 0x81;
pub const ID_SET_DEFAULT_DIGITAL_MAPPINGS: u8 = 0x85;
pub const ID_SET_SETTINGS_VALUES: u8 = 0x87;
pub const ID_LOAD_DEFAULT_SETTINGS: u8 = 0x8e;
/// Powers the controller down, as holding the Steam button does.
pub const ID_TURN_OFF_CONTROLLER: u8 = 0x9f;

// TODO Unverified: counted from the settings enum in hid-steam 7.3, not read from a build.
/// `SETTING_LIZARD_MODE`, position 9 of the settings enum.
pub const SETTING_LIZARD_MODE: u8 = 9;
/// `SETTING_IMU_MODE`, position 48 of the settings enum.
pub const SETTING_IMU_MODE: u8 = 48;
/// `SETTING_IMU_MODE_SEND_RAW_ACCEL | SETTING_IMU_MODE_SEND_RAW_GYRO`, bits 3 and 4.
pub const IMU_MODE_RAW: u8 = 0x18;
/// `SETTING_STEAM_WATCHDOG_ENABLE`, position 71 of the settings enum.
pub const SETTING_STEAM_WATCHDOG_ENABLE: u8 = 71;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Wireless {
    Disconnect,
    Connect,
    Pair,
    Unknown(u8),
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Report {
    Input(ControllerState),
    Battery(Battery),
    Wireless(Wireless),
    Other(u8),
}

pub fn decode(report: &[u8]) -> Result<Report> {
    let Some(id) = report.first() else {
        return Err(anyhow!("Empty report"));
    };
    match *id {
        REPORT_ID_INPUT => Ok(Report::Input(decode_input(report, REPORT_LEN_INPUT)?)),
        REPORT_ID_INPUT2 => Ok(Report::Input(decode_input(report, REPORT_LEN_INPUT2)?)),
        REPORT_ID_BATTERY => Ok(Report::Battery(decode_battery(report)?)),
        REPORT_ID_WIRELESS => Ok(Report::Wireless(decode_wireless(report)?)),
        id => Ok(Report::Other(id)),
    }
}

fn decode_input(
    report: &[u8],
    len: usize,
) -> Result<ControllerState> {
    if report.len() < len {
        return Err(anyhow!(
            "Short input report {:#04x} of {} bytes",
            report[0],
            report.len()
        ));
    }
    let buttons = Buttons::new(u32_at(report, 2));
    let mut quaternion = [0; 4];
    if len == REPORT_LEN_INPUT {
        quaternion = [
            i16_at(report, 46),
            i16_at(report, 48),
            i16_at(report, 50),
            i16_at(report, 52),
        ];
    }
    Ok(ControllerState {
        seq: report[1],
        buttons,
        lt: u16_at(report, 6),
        rt: u16_at(report, 8),
        lstick: Stick {
            x: i16_at(report, 10),
            y: i16_at(report, 12),
        },
        rstick: Stick {
            x: i16_at(report, 14),
            y: i16_at(report, 16),
        },
        lpad: Trackpad {
            x: i16_at(report, 18),
            y: i16_at(report, 20),
            pressure: u16_at(report, 22),
            touched: buttons.lpad_touch(),
            pressed: buttons.lpad_click(),
        },
        rpad: Trackpad {
            x: i16_at(report, 24),
            y: i16_at(report, 26),
            pressure: u16_at(report, 28),
            touched: buttons.rpad_touch(),
            pressed: buttons.rpad_click(),
        },
        imu: Imu {
            timestamp: u32_at(report, 30),
            accel: [i16_at(report, 34), i16_at(report, 36), i16_at(report, 38)],
            gyro: [i16_at(report, 40), i16_at(report, 42), i16_at(report, 44)],
            quaternion,
        },
    })
}

fn decode_battery(report: &[u8]) -> Result<Battery> {
    if report.len() < REPORT_LEN_BATTERY {
        return Err(anyhow!("Short battery report of {} bytes", report.len()));
    }
    let charge = match report[1] {
        1 => Charge::Discharging,
        2 => Charge::Charging,
        4 => Charge::Done,
        other => Charge::Unknown(other),
    };
    Ok(Battery {
        charge,
        level: report[2],
    })
}

fn decode_wireless(report: &[u8]) -> Result<Wireless> {
    if report.len() < REPORT_LEN_WIRELESS {
        return Err(anyhow!("Short wireless report of {} bytes", report.len()));
    }
    Ok(match report[1] {
        1 => Wireless::Disconnect,
        2 => Wireless::Connect,
        3 => Wireless::Pair,
        other => Wireless::Unknown(other),
    })
}

fn u16_at(
    report: &[u8],
    offset: usize,
) -> u16 {
    u16::from_le_bytes([report[offset], report[offset + 1]])
}

fn i16_at(
    report: &[u8],
    offset: usize,
) -> i16 {
    i16::from_le_bytes([report[offset], report[offset + 1]])
}

fn u32_at(
    report: &[u8],
    offset: usize,
) -> u32 {
    u32::from_le_bytes([
        report[offset],
        report[offset + 1],
        report[offset + 2],
        report[offset + 3],
    ])
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::state::Button;

    /// Idle report from a puck slot, captured 2026-11-01; first 24 bytes, rest zero.
    fn idle() -> [u8; REPORT_LEN_INPUT] {
        let mut report = [0; REPORT_LEN_INPUT];
        report[..24].copy_from_slice(&[
            0x42, 0x13, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x39, 0x01, 0x85, 0xff,
            0x16, 0xff, 0x99, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]);
        report
    }

    #[test]
    fn test_decode_0x42_idle() {
        let Report::Input(state) = decode(&idle()).unwrap() else {
            panic!("not an input report");
        };
        assert_eq!(state.seq, 0x13);
        assert!(state.buttons.pressed().is_empty());
        for button in Button::ALL {
            if !button.is_touch() {
                assert!(!state.buttons.contains(button), "{button}");
            }
        }
        // 5.4 is grip touch, set while the controller is held
        assert!(state.buttons.grip_r_touch());
        assert_eq!(state.lstick.x, 0x0139);
        assert_eq!(state.lstick.y, 0xff85_u16 as i16);
        assert_eq!(state.lstick.y, -123);
        assert_eq!(state.rstick.x, 0xff16_u16 as i16);
        assert_eq!(state.rstick.y, 0x0099);
        assert_eq!(state.lt, 0);
        assert_eq!(state.rt, 0);
        assert!(!state.lpad.touched);
    }

    #[test]
    fn test_decode_0x42_buttons() {
        let mut report = idle();
        report[2] = 0b0000_0001;
        report[3] = 0b0010_0000;
        report[4] = 0b0000_0001;
        report[5] = 0b0000_1000;
        let Report::Input(state) = decode(&report).unwrap() else {
            panic!("not an input report");
        };
        assert!(state.buttons.a());
        assert!(state.buttons.dpad_up());
        assert!(state.buttons.steam());
        assert!(state.buttons.lt_full());
        assert!(!state.buttons.b());
        assert!(!state.buttons.view());
    }

    #[test]
    fn test_decode_0x42_axes() {
        let mut report = idle();
        report[6..8].copy_from_slice(&32767_u16.to_le_bytes());
        report[18..20].copy_from_slice(&(-1000_i16).to_le_bytes());
        report[46..48].copy_from_slice(&16384_i16.to_le_bytes());
        let Report::Input(state) = decode(&report).unwrap() else {
            panic!("not an input report");
        };
        assert_eq!(state.lt, 32767);
        assert_eq!(state.lpad.x, -1000);
        assert_eq!(state.imu.quaternion[0], 16384);
    }

    #[test]
    fn test_decode_0x45() {
        let mut report = [0; REPORT_LEN_INPUT2];
        report[0] = REPORT_ID_INPUT2;
        report[3] = 0b0100_0000;
        report[10..12].copy_from_slice(&(-32768_i16).to_le_bytes());
        let Report::Input(state) = decode(&report).unwrap() else {
            panic!("not an input report");
        };
        assert!(state.buttons.view());
        assert_eq!(state.lstick.x, -32768);
        assert_eq!(state.imu.quaternion, [0; 4]);
    }

    /// Idle report over Bluetooth LE, captured 2026-11-04: `0x45` streams at ~133 Hz with
    /// accel and gyro filled in, no quaternion.
    #[test]
    fn test_decode_0x45_ble() {
        let report = [
            0x45, 0xc7, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xd7, 0x00, 0x13, 0x01,
            0x7f, 0x00, 0xe9, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x07, 0xfb, 0xf0, 0x0a, 0x2d, 0xfc, 0xe6, 0xf8, 0xa3, 0x3f, 0xfd, 0xff,
            0xff, 0xff, 0x00, 0x00,
        ];
        assert_eq!(report.len(), REPORT_LEN_INPUT2);
        let Report::Input(state) = decode(&report).unwrap() else {
            panic!("not an input report");
        };
        assert_eq!(state.seq, 0xc7);
        assert!(state.buttons.pressed().is_empty());
        assert_eq!(state.lstick.x, 0x00d7);
        assert_eq!(state.lstick.y, 0x0113);
        assert_eq!(state.rstick.x, 0x007f);
        assert_eq!(state.rstick.y, -23);
        assert_eq!(state.imu.timestamp, 0x0af0_fb07);
        assert_eq!(state.imu.accel, [-979, -1818, 16291]);
        assert_eq!(state.imu.gyro, [-3, -1, 0]);
        assert_eq!(state.imu.quaternion, [0; 4]);
    }

    /// Battery report over Bluetooth LE, captured 2026-11-04; one every ~3 s.
    #[test]
    fn test_decode_0x43_ble() {
        let report = [
            0x43, 0x01, 0x51, 0xad, 0x0f, 0xb4, 0x0f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xca,
            0x60,
        ];
        assert_eq!(
            decode(&report).unwrap(),
            Report::Battery(Battery {
                charge: Charge::Discharging,
                level: 81,
            })
        );
    }

    #[test]
    fn test_decode_short() {
        assert!(decode(&[REPORT_ID_INPUT, 0, 0]).is_err());
        assert!(decode(&[]).is_err());
    }

    #[test]
    fn test_decode_0x43() {
        let mut report = [0; REPORT_LEN_BATTERY];
        report[0] = REPORT_ID_BATTERY;
        report[1] = 2;
        report[2] = 87;
        assert_eq!(
            decode(&report).unwrap(),
            Report::Battery(Battery {
                charge: Charge::Charging,
                level: 87,
            })
        );
    }

    #[test]
    fn test_decode_0x79() {
        assert_eq!(
            decode(&[REPORT_ID_WIRELESS, 2]).unwrap(),
            Report::Wireless(Wireless::Connect)
        );
        assert_eq!(
            decode(&[REPORT_ID_WIRELESS, 1]).unwrap(),
            Report::Wireless(Wireless::Disconnect)
        );
    }
}
