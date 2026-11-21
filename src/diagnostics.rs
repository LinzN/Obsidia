//! Battery health and hardware details of a device in normal mode.
//!
//! Battery values come from the `AppleSmartBattery` entry of the I/O
//! registry (via `idevicediagnostics`), everything else from lockdown
//! (`ideviceinfo`). Both need a paired ("trusted") device.

use crate::device::{self, Device};
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct Battery {
    /// Current charge in percent.
    pub level: Option<u32>,
    pub cycles: Option<u64>,
    /// Capacity the battery was designed for, in mAh.
    pub design_capacity: Option<u64>,
    /// Capacity the battery holds today, in mAh.
    pub full_capacity: Option<u64>,
    pub temperature: Option<f32>,
    pub voltage: Option<f32>,
    pub charging: Option<bool>,
    pub external_power: Option<bool>,
    pub serial: Option<String>,
}

impl Battery {
    /// Maximum capacity relative to the design capacity, in percent.
    pub fn health(&self) -> Option<f32> {
        let design = self.design_capacity? as f32;
        let full = self.full_capacity? as f32;
        (design > 0.0).then(|| (full / design * 100.0).min(100.0))
    }
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub battery: Option<Battery>,
    /// Ordered key/value pairs for display.
    pub hardware: Vec<(&'static str, String)>,
    pub storage_total: Option<u64>,
    pub storage_free: Option<u64>,
}

/// Reads all values. Takes a few seconds.
pub fn read(dev: &Device) -> Result<Report> {
    let _lock = device::shared();
    let info = device::run("ideviceinfo", &["-u", &dev.udid])?.unwrap_or_default();
    let info = device::parse_kv(&info);
    if !info.contains_key("ProductType") {
        bail!("The device did not answer. Unlock it and confirm \"Trust this computer\".");
    }
    let disk = device::run(
        "ideviceinfo",
        &["-u", &dev.udid, "-q", "com.apple.disk_usage"],
    )
    .ok()
    .flatten()
    .map(|s| device::parse_kv(&s))
    .unwrap_or_default();
    let level = device::run(
        "ideviceinfo",
        &[
            "-u",
            &dev.udid,
            "-q",
            "com.apple.mobile.battery",
            "-k",
            "BatteryCurrentCapacity",
        ],
    )
    .ok()
    .flatten()
    .and_then(|s| s.trim().parse().ok());

    let mut battery = read_battery(&dev.udid);
    if let Some(b) = &mut battery {
        b.level = b.level.or(level);
    } else if level.is_some() {
        battery = Some(Battery {
            level,
            ..Default::default()
        });
    }

    Ok(Report {
        battery,
        hardware: hardware(&info),
        storage_total: num(&disk, "TotalDataCapacity").or_else(|| num(&disk, "TotalDiskCapacity")),
        storage_free: num(&disk, "TotalDataAvailable"),
    })
}

fn num(map: &HashMap<String, String>, key: &str) -> Option<u64> {
    map.get(key)?.parse().ok()
}

fn hardware(info: &HashMap<String, String>) -> Vec<(&'static str, String)> {
    let get = |k: &str| info.get(k).filter(|v| !v.is_empty()).cloned();
    let mut rows = Vec::new();
    // "MQ0G3" + "ZD/A" -> the order number printed on the box
    if let Some(model) = get("ModelNumber") {
        rows.push((
            "Model number",
            format!("{model}{}", get("RegionInfo").unwrap_or_default()),
        ));
    }
    for (label, key) in [
        ("IMEI", "InternationalMobileEquipmentIdentity"),
        ("IMEI 2", "InternationalMobileEquipmentIdentity2"),
        ("MEID", "MobileEquipmentIdentifier"),
        ("Activation", "ActivationState"),
        ("Baseband", "BasebandVersion"),
        ("Board", "HardwareModel"),
        ("Bootloader", "FirmwareVersion"),
        ("CPU", "CPUArchitecture"),
        ("Wi-Fi address", "WiFiAddress"),
        ("Bluetooth address", "BluetoothAddress"),
    ] {
        if let Some(v) = get(key) {
            rows.push((label, v));
        }
    }
    rows
}

fn read_battery(udid: &str) -> Option<Battery> {
    let attempts: [&[&str]; 2] = [
        &["-u", udid, "ioregentry", "AppleSmartBattery"],
        &["-u", udid, "diagnostics", "GasGauge"],
    ];
    for args in attempts {
        let Some(out) = device::run("idevicediagnostics", args).ok().flatten() else {
            continue;
        };
        if let Some(b) = parse_battery(&out) {
            return Some(b);
        }
    }
    None
}

/// Finds the dictionary with the battery values in the plist printed by
/// `idevicediagnostics`, wherever it is nested.
fn parse_battery(output: &str) -> Option<Battery> {
    let start = output.find("<?xml").or_else(|| output.find("<plist"))?;
    let end = output.rfind("</plist>")? + "</plist>".len();
    let value = plist::Value::from_reader_xml(&output.as_bytes()[start..end]).ok()?;
    let dict = find_dict(&value)?;
    let int = |k: &str| {
        dict.get(k).and_then(|v| {
            v.as_signed_integer()
                .or_else(|| v.as_unsigned_integer().map(|u| u as i64))
        })
    };
    let uint = |k: &str| int(k).filter(|v| *v >= 0).map(|v| v as u64);
    let bool_ = |k: &str| dict.get(k).and_then(|v| v.as_boolean());

    let full = uint("NominalChargeCapacity")
        .or_else(|| uint("AppleRawMaxCapacity"))
        .or_else(|| uint("FullChargeCapacity"));
    // Temperature is reported in hundredths of a degree Celsius.
    let temperature = int("Temperature").map(|t| t as f32 / 100.0);
    let voltage = int("Voltage").map(|v| v as f32 / 1000.0);
    let level = uint("CurrentCapacity")
        .filter(|c| *c <= 100)
        .map(|c| c as u32);
    let battery = Battery {
        level,
        cycles: uint("CycleCount"),
        design_capacity: uint("DesignCapacity"),
        full_capacity: full,
        temperature: temperature.filter(|t| (-20.0..90.0).contains(t)),
        voltage: voltage.filter(|v| *v > 0.0),
        charging: bool_("IsCharging"),
        external_power: bool_("ExternalConnected"),
        serial: dict
            .get("Serial")
            .or_else(|| dict.get("BatterySerialNumber"))
            .and_then(|v| v.as_string())
            .map(String::from),
    };
    (battery.cycles.is_some() || battery.design_capacity.is_some()).then_some(battery)
}

fn find_dict(value: &plist::Value) -> Option<&plist::Dictionary> {
    let dict = value.as_dictionary()?;
    if dict.contains_key("CycleCount") || dict.contains_key("DesignCapacity") {
        return Some(dict);
    }
    dict.values().find_map(find_dict)
}

/// Plain text report, e.g. for a device handover.
pub fn report_text(dev: &Device, model: &str, report: &Report) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "Obsidia device report");
    let _ = writeln!(s, "=====================");
    let _ = writeln!(s, "Created: {}", now());
    let _ = writeln!(s);
    let _ = writeln!(s, "Device:            {model} ({})", dev.product);
    if let Some(v) = &dev.version {
        let _ = writeln!(
            s,
            "OS version:        {v} {}",
            dev.build.as_deref().unwrap_or("")
        );
    }
    if let Some(sn) = &dev.serial {
        let _ = writeln!(s, "Serial number:     {sn}");
    }
    let _ = writeln!(s, "UDID:              {}", dev.udid);
    for (k, v) in &report.hardware {
        let _ = writeln!(s, "{:<19}{v}", format!("{k}:"));
    }
    if let (Some(total), Some(free)) = (report.storage_total, report.storage_free) {
        let _ = writeln!(
            s,
            "Storage:           {} free of {}",
            indicatif::HumanBytes(free),
            indicatif::HumanBytes(total)
        );
    }
    let _ = writeln!(s);
    let _ = writeln!(s, "Battery");
    let _ = writeln!(s, "-------");
    match &report.battery {
        None => {
            let _ = writeln!(s, "Not available");
        }
        Some(b) => {
            if let Some(h) = b.health() {
                let _ = writeln!(s, "Maximum capacity:  {h:.0} %");
            }
            if let Some(c) = b.cycles {
                let _ = writeln!(s, "Charge cycles:     {c}");
            }
            if let (Some(d), Some(f)) = (b.design_capacity, b.full_capacity) {
                let _ = writeln!(s, "Capacity:          {f} mAh of {d} mAh");
            }
            if let Some(l) = b.level {
                let _ = writeln!(s, "Charge level:      {l} %");
            }
            if let Some(t) = b.temperature {
                let _ = writeln!(s, "Temperature:       {t:.1} °C");
            }
            if let Some(v) = b.voltage {
                let _ = writeln!(s, "Voltage:           {v:.2} V");
            }
            if let Some(sn) = &b.serial {
                let _ = writeln!(s, "Battery serial:    {sn}");
            }
        }
    }
    s
}

/// Writes the report to `Documents/Obsidia/Reports`.
pub fn save_report(dev: &Device, model: &str, report: &Report) -> Result<PathBuf> {
    let dir = crate::firmware::data_dir().join("Reports");
    std::fs::create_dir_all(&dir)?;
    let id = dev.serial.clone().unwrap_or_else(|| dev.udid.clone());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("{id}-{stamp}.txt"));
    std::fs::write(&path, report_text(dev, model, report))?;
    Ok(path)
}

fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // civil date from days since 1970-01-01 (UTC)
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60
    )
}

pub fn restart(udid: &str) -> Result<()> {
    let _lock = device::shared();
    let (ok, out) = device::run_full(
        "idevicediagnostics",
        &["-u", udid, "restart"],
        std::time::Duration::from_secs(30),
    )?;
    if !ok {
        bail!(
            "{}",
            out.lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("failed")
                .trim()
        );
    }
    Ok(())
}

pub fn shutdown(udid: &str) -> Result<()> {
    let _lock = device::shared();
    let (ok, out) = device::run_full(
        "idevicediagnostics",
        &["-u", udid, "shutdown"],
        std::time::Duration::from_secs(30),
    )?;
    if !ok {
        bail!(
            "{}",
            out.lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("failed")
                .trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const IOREG: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>IORegistry</key>
  <dict>
    <key>CycleCount</key><integer>412</integer>
    <key>DesignCapacity</key><integer>3200</integer>
    <key>NominalChargeCapacity</key><integer>2784</integer>
    <key>AppleRawMaxCapacity</key><integer>2800</integer>
    <key>Temperature</key><integer>3050</integer>
    <key>Voltage</key><integer>4012</integer>
    <key>IsCharging</key><true/>
    <key>ExternalConnected</key><true/>
    <key>CurrentCapacity</key><integer>84</integer>
    <key>Serial</key><string>F5D2341ABCD</string>
  </dict>
</dict>
</plist>"#;

    #[test]
    fn parses_smart_battery() {
        let b = parse_battery(IOREG).unwrap();
        assert_eq!(b.cycles, Some(412));
        assert_eq!(b.full_capacity, Some(2784));
        assert_eq!(b.level, Some(84));
        assert!((b.health().unwrap() - 87.0).abs() < 0.1);
        assert!((b.temperature.unwrap() - 30.5).abs() < 0.01);
        assert_eq!(b.charging, Some(true));
    }

    #[test]
    fn ignores_output_without_battery() {
        assert!(parse_battery("ERROR: Could not connect").is_none());
    }
}
