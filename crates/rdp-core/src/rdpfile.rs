//! .rdp import/export.
//! ponytail: only the keys the profile model has; unknown keys are dropped on import.
//! Use FreeRDP's own parser (freerdp_client_settings_parse_connection_file) if full fidelity is needed.

use crate::model::{DisplayMode, Gateway, Profile, SharedFolder};

/// `full address:s:host[:port]` lines → Profile. Lines look like `key:type:value`.
pub fn parse(name: &str, text: &str) -> Result<Profile, String> {
    let mut p = Profile::new(name, "");
    let mut gw_host = String::new();
    p.redirect.clipboard = false;
    p.redirect.audio = false;
    let (mut w, mut h) = (None, None);
    let mut fullscreen = false;
    let mut dynamic = false;
    for line in text.lines() {
        let mut it = line.trim().splitn(3, ':');
        let (Some(k), Some(_ty), Some(v)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        match k.trim().to_lowercase().as_str() {
            "full address" => match v.rsplit_once(':') {
                Some((host, port)) if port.parse::<u16>().is_ok() => {
                    p.host = host.into();
                    p.port = port.parse().unwrap();
                }
                _ => p.host = v.into(),
            },
            "server port" => p.port = v.parse().unwrap_or(p.port),
            "username" => match v.split_once('\\') {
                Some((d, u)) => {
                    p.domain = d.into();
                    p.username = u.into();
                }
                None => p.username = v.into(),
            },
            "domain" => p.domain = v.into(),
            "desktopwidth" => w = v.parse().ok(),
            "desktopheight" => h = v.parse().ok(),
            "screen mode id" => fullscreen = v == "2",
            "dynamic resolution" => dynamic = v == "1",
            "redirectclipboard" => p.redirect.clipboard = v == "1",
            "redirectprinters" => p.redirect.printers = v == "1",
            "audiomode" => p.redirect.audio = v == "0",
            "gatewayhostname" if !v.is_empty() => gw_host = v.into(),
            "drivestoredirect" => {
                for path in v
                    .split(';')
                    .filter(|s| !s.is_empty() && !s.starts_with('*') && !s.starts_with("DynamicDrives"))
                {
                    let name = path.rsplit('/').next().unwrap_or(path);
                    p.redirect.folders.push(SharedFolder {
                        name: name.into(),
                        path: path.into(),
                    });
                }
            }
            _ => {}
        }
    }
    if p.host.is_empty() {
        return Err("no `full address` in .rdp file".into());
    }
    if !gw_host.is_empty() {
        p.gateway = Some(Gateway {
            host: gw_host,
            ..Gateway::default()
        });
    }
    if let (Some(w), Some(h)) = (w, h) {
        p.display.width = w;
        p.display.height = h;
        p.display.mode = DisplayMode::Fixed;
    }
    if dynamic {
        p.display.mode = DisplayMode::Dynamic;
    }
    if fullscreen {
        p.display.mode = DisplayMode::Fullscreen;
    }
    Ok(p)
}

/// Profile → .rdp text. The password is never written.
pub fn write(p: &Profile) -> String {
    let mut s = String::new();
    let mut line = |k: &str, t: char, v: String| s.push_str(&format!("{k}:{t}:{v}\r\n"));
    line(
        "full address",
        's',
        if p.port == 3389 {
            p.host.clone()
        } else {
            format!("{}:{}", p.host, p.port)
        },
    );
    if !p.username.is_empty() {
        line("username", 's', p.username.clone());
    }
    if !p.domain.is_empty() {
        line("domain", 's', p.domain.clone());
    }
    match p.display.mode {
        DisplayMode::Fullscreen => line("screen mode id", 'i', "2".into()),
        DisplayMode::Fixed => {
            line("screen mode id", 'i', "1".into());
            line("desktopwidth", 'i', p.display.width.to_string());
            line("desktopheight", 'i', p.display.height.to_string());
        }
        DisplayMode::Dynamic => line("dynamic resolution", 'i', "1".into()),
    }
    line("redirectclipboard", 'i', (p.redirect.clipboard as u8).to_string());
    line("redirectprinters", 'i', (p.redirect.printers as u8).to_string());
    line("audiomode", 'i', if p.redirect.audio { "0" } else { "2" }.into());
    if !p.redirect.folders.is_empty() {
        let v: Vec<_> = p.redirect.folders.iter().map(|f| f.path.as_str()).collect();
        line("drivestoredirect", 's', v.join(";"));
    }
    if let Some(g) = &p.gateway {
        line("gatewayhostname", 's', g.host.clone());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut p = Profile::new("web", "10.0.0.5");
        p.port = 3390;
        p.username = "svc".into();
        p.domain = "CORP".into();
        p.display.mode = DisplayMode::Fixed;
        p.display.width = 1280;
        p.display.height = 720;
        p.redirect.printers = true;
        p.redirect.clipboard = true;
        p.redirect.folders.push(SharedFolder {
            name: "Transfer".into(),
            path: "/home/b/Transfer".into(),
        });
        p.gateway = Some(Gateway {
            host: "gw.example.com".into(),
            ..Gateway::default()
        });
        let q = parse("web", &write(&p)).unwrap();
        assert_eq!((q.host.as_str(), q.port), ("10.0.0.5", 3390));
        assert_eq!((q.username.as_str(), q.domain.as_str()), ("svc", "CORP"));
        assert_eq!(q.display, p.display);
        assert!(q.redirect.printers && q.redirect.clipboard && q.redirect.audio);
        assert_eq!(q.redirect.folders, p.redirect.folders);
        assert_eq!(q.gateway.unwrap().host, "gw.example.com");
    }

    #[test]
    fn domain_backslash_and_missing_address() {
        let q = parse("x", "full address:s:h\r\nusername:s:CORP\\bob\r\n").unwrap();
        assert_eq!((q.domain.as_str(), q.username.as_str()), ("CORP", "bob"));
        assert!(parse("x", "username:s:bob").is_err());
    }
}
