//! The controller's Bluetooth link, through `bluetoothctl`: the HID node names only the host
//! adapter (`HID_PHYS`), so the controller is found by its name among the connected devices.

use std::process::Command;

use anyhow::anyhow;
use log::info;
use log::warn;

use anyhow::Result;

/// The addresses of the Steam Controllers in `bluetoothctl devices Connected` output:
/// `Device C5:4D:9F:F4:C2:4E Steam Ctrl (BT) FXA9962402F19`, one a line.
pub fn controllers(connected: &str) -> Vec<String> {
    connected
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("Device ")?;
            let (address, name) = rest.split_once(' ')?;
            name.starts_with("Steam Ctrl").then(|| address.to_string())
        })
        .collect()
}

/// Drop the link of every connected Steam Controller, so it sleeps. A failed disconnect (the
/// controller may already have dropped the link) is logged and the others still go.
pub fn disconnect_controllers() -> Result<()> {
    let listed = Command::new("bluetoothctl")
        .args(["devices", "Connected"])
        .output()?;
    if !listed.status.success() {
        return Err(anyhow!(
            "bluetoothctl devices: {}: {}",
            listed.status,
            String::from_utf8_lossy(&listed.stderr).trim()
        ));
    }
    for address in controllers(&String::from_utf8_lossy(&listed.stdout)) {
        match Command::new("bluetoothctl")
            .args(["disconnect", &address])
            .output()
        {
            Ok(out) if out.status.success() => info!("Bluetooth {address} disconnected"),
            Ok(out) => warn!(
                "bluetoothctl disconnect {address}: {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stdout).trim()
            ),
            Err(err) => warn!("bluetoothctl disconnect {address}: {err}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_controllers() {
        let out = "Device C5:4D:9F:F4:C2:4E Steam Ctrl (BT) FXA9962402F19\n\
                   Device AA:BB:CC:DD:EE:FF Keyboard K380\n\
                   Device 11:22:33:44:55:66 Steam Ctrl (BT) ABC\n";
        assert_eq!(controllers(out), ["C5:4D:9F:F4:C2:4E", "11:22:33:44:55:66"]);
        assert!(controllers("").is_empty());
        assert!(controllers("No default controller available\n").is_empty());
    }
}
