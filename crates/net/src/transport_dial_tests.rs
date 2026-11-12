// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #433: an address that never answers doesn't use up the time the server's other addresses need.

use super::*;

#[test]
fn an_address_that_never_answers_leaves_time_for_the_next() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let here = listener.local_addr().unwrap();
    // Simulate a timeout: TEST-NET-1 may actually be intercepted by local networking software.
    let nowhere = SocketAddr::from(([192, 0, 2, 1], here.port()));
    let budget = Duration::from_secs(2);
    let started = Instant::now();
    let tcp = connect_resolved(
        &[nowhere, here],
        budget,
        |address, timeout| {
            if *address == nowhere {
                Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "simulated timeout"))
            } else {
                TcpStream::connect_timeout(address, timeout)
            }
        },
        || started.elapsed(),
    )
    .expect("the second address was never tried");
    assert_eq!(tcp.peer_addr().unwrap(), here);
    assert!(started.elapsed() < budget, "took {:?}", started.elapsed());
}

#[test]
fn no_address_says_so() {
    assert_eq!(connect_first(&[], Duration::from_secs(1)).err().map(|e| e.to_string()).as_deref(), Some("no address was found for the name"));
}
