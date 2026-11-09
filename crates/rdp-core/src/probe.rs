use std::{
    net::{TcpStream, ToSocketAddrs},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reach {
    Online { rtt_ms: u32 },
    Offline,
}

/// Blocking TCP connect; call from a worker thread. Only the first resolved address is tried.
pub fn probe(host: &str, port: u16, timeout: Duration) -> Reach {
    let Some(addr) = (host, port).to_socket_addrs().ok().and_then(|mut a| a.next()) else {
        return Reach::Offline;
    };
    let t = Instant::now();
    match TcpStream::connect_timeout(&addr, timeout) {
        Ok(_) => Reach::Online {
            rtt_ms: t.elapsed().as_millis() as u32,
        },
        Err(_) => Reach::Offline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_and_offline() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(matches!(
            probe("127.0.0.1", port, Duration::from_secs(1)),
            Reach::Online { .. }
        ));
        drop(l);
        assert_eq!(probe("127.0.0.1", port, Duration::from_secs(1)), Reach::Offline);
        assert_eq!(probe("host.invalid", 3389, Duration::from_millis(200)), Reach::Offline);
    }
}
