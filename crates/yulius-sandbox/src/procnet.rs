//! Listening TCP ports from `/proc/net/tcp` and `/proc/net/tcp6` (as the reading process's
//! network namespace sees them).

use std::collections::BTreeSet;

/// `st` of a listening socket.
const LISTEN: &str = "0A";
/// Lines read per table at most (a sandbox with more sockets reports a partial list).
const MAX_LINES: usize = 65_536;

/// Ports in `first..=last` that a socket listens on, from the text of one table.
pub fn listening_in(text: &str, first: u16, last: u16, into: &mut BTreeSet<u16>) {
    for line in text.lines().skip(1).take(MAX_LINES) {
        let mut fields = line.split_whitespace();
        let (Some(_slot), Some(local), Some(_remote), Some(state)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if state != LISTEN {
            continue;
        }
        let Some((_, port)) = local.rsplit_once(':') else {
            continue;
        };
        if let Ok(port) = u16::from_str_radix(port, 16)
            && (first..=last).contains(&port)
        {
            into.insert(port);
        }
    }
}

/// Ports in `first..=last` listening in this process's network namespace (IPv4 and IPv6).
#[must_use]
pub fn listening_ports(first: u16, last: u16) -> Vec<u16> {
    let mut ports = BTreeSet::new();
    for table in ["/proc/self/net/tcp", "/proc/self/net/tcp6"] {
        if let Ok(text) = std::fs::read_to_string(table) {
            listening_in(&text, first, last, &mut ports);
        }
    }
    ports.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:4E20 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1001 1 0 100 0 0 10 0\n\
   1: 00000000:4E21 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1002 1 0 100 0 0 10 0\n\
   2: 0100007F:4E22 0100007F:9C40 01 00000000:00000000 00:00000000 00000000     0        0 1003 1 0 100 0 0 10 0\n\
   3: 0100007F:1F91 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1004 1 0 100 0 0 10 0\n\
   broken line\n";
    const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 00000000000000000000000001000000:4E23 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 2001 1 0 100 0 0 10 0\n";

    #[test]
    fn only_listening_sockets_in_the_range_count() {
        let mut ports = BTreeSet::new();
        listening_in(TCP, 20_000, 20_019, &mut ports);
        listening_in(TCP6, 20_000, 20_019, &mut ports);
        // 20000 (127.0.0.1), 20001 (0.0.0.0), 20003 (::1); 20002 is a connection, 8081 out of
        // range.
        assert_eq!(
            ports.into_iter().collect::<Vec<_>>(),
            vec![20_000, 20_001, 20_003]
        );
    }

    #[test]
    fn this_process_sees_its_own_listener() -> Result<(), Box<dyn std::error::Error>> {
        if !std::path::Path::new("/proc/self/net/tcp").exists() {
            return Ok(());
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        assert!(listening_ports(port, port).contains(&port));
        Ok(())
    }
}
