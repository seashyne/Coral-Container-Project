//! CGI-style response header parsing for Wasm stdout.

/// Parses optional CGI-style headers (`Status: 201`, `Content-Type: application/json`, blank line)
/// from Wasm stdout; falls back to `200 OK` raw body if no header block is present.
pub fn parse_cgi_or_raw_response(raw: &[u8]) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let sep = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| (p, 4))
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n").map(|p| (p, 2)));

    let Some((pos, sep_len)) = sep else {
        return (200, Vec::new(), raw.to_vec());
    };
    let head = String::from_utf8_lossy(&raw[..pos]);
    let mut status = 200u16;
    let mut headers = Vec::new();
    let mut valid_cgi = false;

    for line in head.lines() {
        let Some((k, v)) = line.split_once(':') else {
            return (200, Vec::new(), raw.to_vec());
        };
        let key = k.trim();
        let val = v.trim();
        if key.is_empty() || key.contains(' ') {
            return (200, Vec::new(), raw.to_vec());
        }
        valid_cgi = true;
        if key.eq_ignore_ascii_case("status") {
            if let Some(code) = val.split_whitespace().next().and_then(|s| s.parse::<u16>().ok()) {
                status = code;
            }
        } else {
            headers.push((key.to_string(), val.to_string()));
        }
    }

    if valid_cgi {
        (status, headers, raw[pos + sep_len..].to_vec())
    } else {
        (200, Vec::new(), raw.to_vec())
    }
}
