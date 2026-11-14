// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! What kind of device the app runs on, to pick defaults that suit it.

/// Whether this is a phone or a tablet rather than a computer, going by the
/// chassis type the system reports. That is a default only: when nothing
/// says otherwise, the device counts as a computer.
pub fn is_handheld() -> bool {
    if cfg!(any(target_os = "android", target_os = "ios")) {
        return true;
    }
    matches!(chassis().as_deref(), Some("handset" | "tablet" | "watch"))
}

/// The chassis type, as systemd-hostnamed reports it (`hostnamectl
/// chassis`), read from the same places: the machine's own setting, then
/// the device tree of ARM devices such as Linux phones, then the firmware
/// of PCs.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn chassis() -> Option<String> {
    use std::fs;

    let from_machine_info = || {
        fs::read_to_string("/etc/machine-info")
            .ok()
            .and_then(|info| {
                info.lines()
                    .filter_map(|line| line.trim().strip_prefix("CHASSIS="))
                    .map(|value| value.trim_matches(|c| c == '"' || c == '\'').to_string())
                    .next_back()
            })
    };
    let from_hostnamed = || {
        let bus =
            gtk::gio::bus_get_sync(gtk::gio::BusType::System, gtk::gio::Cancellable::NONE).ok()?;
        let reply = bus
            .call_sync(
                Some("org.freedesktop.hostname1"),
                "/org/freedesktop/hostname1",
                "org.freedesktop.DBus.Properties",
                "Get",
                Some(&("org.freedesktop.hostname1", "Chassis").into()),
                None,
                gtk::gio::DBusCallFlags::NONE,
                500,
                gtk::gio::Cancellable::NONE,
            )
            .ok()?;
        let (value,): (gtk::glib::Variant,) = reply.get()?;
        value.get::<String>()
    };
    let from_device_tree = || {
        fs::read("/sys/firmware/devicetree/base/chassis-type")
            .ok()
            .map(|raw| {
                String::from_utf8_lossy(&raw)
                    .trim_end_matches('\0')
                    .to_string()
            })
    };
    // SMBIOS chassis types: 30 is a tablet, 31 a convertible, 32 a
    // detachable; the last two are laptops for this purpose
    let from_firmware = || {
        let kind = fs::read_to_string("/sys/class/dmi/id/chassis_type").ok()?;
        (kind.trim() == "30").then(|| "tablet".to_string())
    };

    [
        &from_machine_info as &dyn Fn() -> Option<String>,
        &from_hostnamed,
        &from_device_tree,
        &from_firmware,
    ]
    .iter()
    .find_map(|source| source().filter(|c| !c.is_empty()))
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn chassis() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn this_device_can_be_told() {
        println!(
            "chassis: {:?}, handheld: {}",
            super::chassis(),
            super::is_handheld()
        );
    }
}
