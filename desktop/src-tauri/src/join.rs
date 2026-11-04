//! The link and QR code a phone uses to join without Bonjour discovery. The format is in
//! shared/PROTOCOL.md: tstvmic://join?host=…&host=…&name=…[&code=…]

use qrcode::QrCode;
use qrcode::render::svg;

/// `hosts` are tried in order. `code` is only included when the desktop requires one.
pub fn join_link(hosts: &[String], name: &str, code: Option<&str>) -> String {
    let mut link = String::from("tstvmic://join?");
    for host in hosts {
        link.push_str("host=");
        link.push_str(&encode(host));
        link.push('&');
    }
    link.push_str(&format!("name={}", encode(name)));
    if let Some(code) = code {
        link.push_str(&format!("&code={}", encode(code)));
    }
    link
}

pub fn qr_svg(link: &str) -> Option<String> {
    let code = QrCode::new(link).ok()?;
    Some(
        code.render::<svg::Color>()
            .min_dimensions(320, 320)
            .dark_color(svg::Color("#000000"))
            .light_color(svg::Color("#ffffff"))
            .build(),
    )
}

/// Percent-encodes everything except the characters that are always safe in a URL.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_lists_every_address_and_encodes_names() {
        let hosts = vec!["10.0.0.5".to_string(), "2607:fb90::1".to_string()];
        let link = join_link(&hosts, "TSTV (RG’s Mac)", None);
        assert_eq!(link, "tstvmic://join?host=10.0.0.5&host=2607%3Afb90%3A%3A1&name=TSTV%20%28RG%E2%80%99s%20Mac%29");
    }

    #[test]
    fn link_includes_the_code_only_when_given() {
        let hosts = vec!["10.0.0.5".to_string()];
        assert!(join_link(&hosts, "Mac", Some("1234")).ends_with("&code=1234"));
        assert!(!join_link(&hosts, "Mac", None).contains("code"));
    }

    #[test]
    fn qr_code_renders() {
        let svg = qr_svg("tstvmic://join?host=10.0.0.5&name=Mac").unwrap();
        assert!(svg.contains("<svg"));
    }
}
