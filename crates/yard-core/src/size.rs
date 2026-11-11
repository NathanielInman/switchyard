//! Memory sizes as people write them ("16G", "512M", "1.5GiB").

pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    let unit = unit.trim().to_ascii_lowercase();
    let unit = unit.trim_end_matches('b').trim_end_matches('i');
    let mult: f64 = match unit {
        "" => 1.0,
        "k" => 1024.0,
        "m" => 1024.0 * 1024.0,
        "g" => 1024.0 * 1024.0 * 1024.0,
        "t" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    if n < 0.0 {
        return None;
    }
    Some((n * mult).round() as u64)
}

/// "5.8 GB", "740 MB", "12 KB". Binary units with the short names people use.
pub fn human(bytes: u64) -> String {
    let b = bytes as f64;
    const G: f64 = 1024.0 * 1024.0 * 1024.0;
    const M: f64 = 1024.0 * 1024.0;
    if b >= G {
        let v = b / G;
        if v >= 10.0 {
            format!("{:.0} GB", v)
        } else {
            format!("{:.1} GB", v)
        }
    } else if b >= M {
        format!("{:.0} MB", b / M)
    } else {
        format!("{:.0} KB", (b / 1024.0).max(1.0))
    }
}

/// The systemd property form: "16G", "512M".
pub fn systemd(bytes: u64) -> String {
    const G: u64 = 1024 * 1024 * 1024;
    const M: u64 = 1024 * 1024;
    if bytes % G == 0 {
        format!("{}G", bytes / G)
    } else if bytes % M == 0 {
        format!("{}M", bytes / M)
    } else {
        bytes.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        assert_eq!(parse_size("16G"), Some(16 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("512M"), Some(512 * 1024 * 1024));
        assert_eq!(parse_size("1.5GiB"), Some(1610612736));
        assert_eq!(parse_size("100"), Some(100));
        assert_eq!(parse_size("12 gb"), Some(12 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("lots"), None);
        assert_eq!(parse_size(""), None);
    }

    #[test]
    fn formats() {
        assert_eq!(human(6_227_702_579), "5.8 GB");
        assert_eq!(human(16 * 1024 * 1024 * 1024), "16 GB");
        assert_eq!(human(300 * 1024 * 1024), "300 MB");
        assert_eq!(systemd(16 * 1024 * 1024 * 1024), "16G");
        assert_eq!(systemd(512 * 1024 * 1024), "512M");
    }
}
