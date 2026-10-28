//! Linux `hidraw` access to USB FIDO2 security keys.
//!
//! FIDO devices are found by scanning `/sys/class/hidraw` and parsing each
//! device's HID report descriptor for the FIDO Alliance usage page
//! (`0xF1D0`) and the CTAPHID usage (`0x01`).  Reading the descriptor from
//! sysfs avoids both `libudev` and the `HIDIOCGRDESC` ioctl.
//!
//! The device node is opened non-blocking and polled with a short sleep, so
//! no `unsafe` code is needed.  HID reports arrive at most every few
//! milliseconds, so the polling interval adds no noticeable latency.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::ctaphid::{PACKET_SIZE, Packet, Transport};
use crate::error::LpError;

const SYSFS_HIDRAW: &str = "/sys/class/hidraw";
const FIDO_USAGE_PAGE: u32 = 0xF1D0;
const FIDO_USAGE_CTAPHID: u32 = 0x01;
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// A FIDO HID device found on the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceInfo {
    /// The device node, e.g. `/dev/hidraw3`.
    pub(crate) path: PathBuf,
    /// The product name reported by the device, if any.
    pub(crate) product: Option<String>,
}

impl DeviceInfo {
    /// A label to show when asking the user to choose a device.
    pub(crate) fn label(&self) -> String {
        format!(
            "{} ({})",
            self.product.as_deref().unwrap_or("Unknown security key"),
            self.path.display()
        )
    }
}

/// List the FIDO HID devices on the system, ordered by device node.
pub(crate) fn list_fido_devices() -> io::Result<Vec<DeviceInfo>> {
    let entries = match std::fs::read_dir(SYSFS_HIDRAW) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    let mut devices: Vec<DeviceInfo> = entries
        .flatten()
        .filter_map(|entry| {
            let sys_path = entry.path();
            let descriptor = std::fs::read(sys_path.join("device/report_descriptor")).ok()?;
            is_fido_descriptor(&descriptor).then(|| DeviceInfo {
                path: Path::new("/dev").join(entry.file_name()),
                product: read_product_name(&sys_path.join("device/uevent")),
            })
        })
        .collect();
    devices.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(devices)
}

/// Read `HID_NAME` from a device's `uevent` file.
fn read_product_name(uevent: &Path) -> Option<String> {
    std::fs::read_to_string(uevent)
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("HID_NAME="))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// Whether a HID report descriptor describes a FIDO CTAPHID interface.
///
/// The descriptor is a sequence of short items: a prefix byte holding the
/// item tag and a 0, 1, 2 or 4 byte little-endian payload.  The first Usage
/// Page and Usage items identify the top-level collection, which for FIDO
/// devices are `0xF1D0` and `0x01`.  Long items (prefix `0xFE`) are skipped.
pub(crate) fn is_fido_descriptor(descriptor: &[u8]) -> bool {
    const LONG_ITEM: u8 = 0xFE;
    const TAG_MASK: u8 = 0xFC;
    const USAGE_PAGE: u8 = 0x04;
    const USAGE: u8 = 0x08;

    let mut usage_page = None;
    let mut usage = None;
    let mut offset = 0;

    while offset < descriptor.len() && (usage_page.is_none() || usage.is_none()) {
        let prefix = descriptor[offset];
        if prefix == LONG_ITEM {
            let Some(&size) = descriptor.get(offset + 1) else {
                break;
            };
            offset += 3 + usize::from(size);
            continue;
        }

        let size = match prefix & 0x03 {
            3 => 4,
            n => usize::from(n),
        };
        let Some(payload) = descriptor.get(offset + 1..offset + 1 + size) else {
            break;
        };
        let value = payload
            .iter()
            .rev()
            .fold(0u32, |acc, &byte| (acc << 8) | u32::from(byte));

        match prefix & TAG_MASK {
            USAGE_PAGE => {
                usage_page.get_or_insert(value);
            }
            USAGE => {
                usage.get_or_insert(value);
            }
            _ => {}
        }
        offset += 1 + size;
    }

    usage_page == Some(FIDO_USAGE_PAGE) && usage == Some(FIDO_USAGE_CTAPHID)
}

/// An open `/dev/hidraw*` device.  The device is closed when dropped.
#[derive(Debug)]
pub(crate) struct HidrawDevice {
    file: File,
}

impl HidrawDevice {
    /// Open a hidraw device node for non-blocking reads and writes.
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?;
        Ok(Self { file })
    }
}

impl Transport for HidrawDevice {
    fn write_packet(&mut self, packet: &Packet) -> io::Result<()> {
        // hidraw expects the report ID first; FIDO devices use report ID 0.
        let mut report = [0u8; PACKET_SIZE + 1];
        report[1..].copy_from_slice(packet);

        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match self.file.write(&report) {
                Ok(n) if n == report.len() => return Ok(()),
                Ok(n) => {
                    return Err(io::Error::other(format!(
                        "short HID write ({n} of {} bytes)",
                        report.len()
                    )));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn read_packet(&mut self, timeout: Duration) -> io::Result<Option<Packet>> {
        let deadline = Instant::now() + timeout;
        let mut packet = [0u8; PACKET_SIZE];
        loop {
            match self.file.read(&mut packet) {
                Ok(n) if n == PACKET_SIZE => return Ok(Some(packet)),
                Ok(n) => {
                    return Err(io::Error::other(format!(
                        "unexpected HID report length {n}"
                    )));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// Explain a failure to enumerate or open a security key.
pub(crate) fn access_error(e: io::Error) -> LpError {
    if e.kind() == io::ErrorKind::PermissionDenied {
        LpError::HardwareKey(
            "Cannot open the security key: permission denied. Check the hidraw device \
             permissions and your distribution's or key vendor's FIDO udev rules."
                .to_string(),
        )
    } else {
        LpError::HardwareKey(format!(
            "Cannot access the security key; check the USB connection and permissions: {e}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The start of a YubiKey 5 FIDO interface report descriptor.
    const YUBIKEY_FIDO: &[u8] = &[
        0x06, 0xD0, 0xF1, // Usage Page (FIDO Alliance)
        0x09, 0x01, // Usage (CTAPHID)
        0xA1, 0x01, // Collection (Application)
        0x09, 0x20, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x40, 0x81, 0x02, 0x09, 0x21,
        0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x40, 0x91, 0x02, 0xC0,
    ];

    /// A boot keyboard descriptor.
    const KEYBOARD: &[u8] = &[0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07, 0xC0];

    #[test]
    fn detects_fido_descriptor() {
        assert!(is_fido_descriptor(YUBIKEY_FIDO));
    }

    #[test]
    fn rejects_other_descriptors() {
        assert!(!is_fido_descriptor(KEYBOARD));
        assert!(!is_fido_descriptor(&[]));
    }

    #[test]
    fn tolerates_truncated_and_long_items() {
        assert!(!is_fido_descriptor(&[0x06, 0xD0]));
        let mut with_long_item = vec![0xFE, 0x02, 0x10, 0xAA, 0xBB];
        with_long_item.extend_from_slice(YUBIKEY_FIDO);
        assert!(is_fido_descriptor(&with_long_item));
    }

    #[test]
    fn device_label_includes_product_and_path() {
        let info = DeviceInfo {
            path: PathBuf::from("/dev/hidraw3"),
            product: Some("Yubico YubiKey FIDO".to_string()),
        };
        assert_eq!(info.label(), "Yubico YubiKey FIDO (/dev/hidraw3)");
    }
}
