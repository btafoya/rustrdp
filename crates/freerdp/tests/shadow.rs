//! End-to-end test against FreeRDP's own shadow server. Run via `scripts/it-shadow.sh`,
//! which starts Xvfb + freerdp-shadow-cli and sets RUSTRDP_SHADOW_PORT.

use freerdp::{ptr, CertVerdict, Command, Credentials, Event, Prompt, Session};
use rdp_core::{DisplayMode, Profile};
use std::{
    sync::{mpsc, Mutex},
    time::{Duration, Instant},
};

#[test]
#[ignore = "needs a shadow server; run scripts/it-shadow.sh"]
fn connect_frames_input_disconnect() {
    let port: u16 = std::env::var("RUSTRDP_SHADOW_PORT")
        .expect("RUSTRDP_SHADOW_PORT")
        .parse()
        .unwrap();
    let mut p = Profile::new("shadow", "127.0.0.1");
    p.port = port;
    p.display.mode = DisplayMode::Fixed;
    // Match the Xvfb screen in scripts/it-shadow.sh so the server does not force a resize.
    p.display.width = 1280;
    p.display.height = 800;
    // e.g. RUSTRDP_TEST_OVERRIDES=FreeRDP_GfxH264=FALSE,FreeRDP_RemoteFxCodec=FALSE
    if let Ok(o) = std::env::var("RUSTRDP_TEST_OVERRIDES") {
        for kv in o.split(',').filter_map(|s| s.split_once('=')) {
            p.overrides.insert(kv.0.into(), kv.1.into());
        }
    }
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let session = Session::start(
        p,
        None,
        Box::new(move |e| {
            let _ = tx.lock().unwrap().send(e);
        }),
    )
    .expect("start");

    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut connected, mut frames) = (None, 0u32);
    while Instant::now() < deadline && !(connected.is_some() && frames > 0) {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Prompt(Prompt::Certificate { reply, info })) => {
                eprintln!("cert prompt: {info:?}");
                reply.send(CertVerdict::AcceptOnce).unwrap()
            }
            // The shadow server (no auth) accepts anything, but the client insists on credentials.
            Ok(Event::Prompt(Prompt::Credentials { reply, .. })) => reply
                .send(Some(Credentials {
                    username: "test".into(),
                    password: "test".into(),
                    domain: String::new(),
                }))
                .unwrap(),
            Ok(Event::Connected { width, height }) => connected = Some((width, height)),
            Ok(Event::Frame { w, h, .. }) => {
                assert!(w > 0 && h > 0);
                frames += 1;
            }
            Ok(Event::Disconnected { reason, .. }) => panic!("disconnected early: {reason}"),
            _ => {}
        }
    }
    let (w, h) = connected.expect("never connected");
    assert!(frames > 0, "connected but no frame arrived");
    {
        let f = session.frame();
        assert_eq!((f.width, f.height), (w, h));
        assert_eq!(f.data.len(), f.stride as usize * f.height as usize);
    }

    session.send(Command::Mouse {
        flags: ptr::MOVE,
        x: 100,
        y: 100,
    });
    session.send(Command::Key {
        scancode: 0x1E,
        down: true,
    }); // 'a'
    session.send(Command::Key {
        scancode: 0x1E,
        down: false,
    });
    session.send(Command::SetClipboardText("héllo".into()));
    session.send(Command::Disconnect);

    let end = Instant::now() + Duration::from_secs(15);
    loop {
        match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            Ok(Event::Disconnected { reason, error }) => {
                assert!(!error, "unclean disconnect: {reason}");
                break;
            }
            Ok(_) => {}
            Err(_) => panic!("no Disconnected event after Command::Disconnect"),
        }
    }
}
