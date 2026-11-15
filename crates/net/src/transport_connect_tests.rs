// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Deterministic regressions for connection attempts sharing one finite deadline.
//! No public servers, real accounts, or credentials are used.

use super::*;
use std::cell::Cell;

fn addresses(count: usize) -> Vec<SocketAddr> {
    (0..count).map(|i| SocketAddr::from(([127, 0, 0, 1], 20000 + i as u16))).collect()
}

#[test]
fn a_silent_first_address_must_not_starve_a_reachable_second_address() {
    let endpoints = addresses(2);
    let spent = Cell::new(Duration::ZERO);
    let mut attempts = Vec::new();
    let result = connect_resolved(
        &endpoints,
        Duration::from_secs(20),
        |address, timeout| {
            attempts.push((*address, timeout));
            if attempts.len() == 1 {
                spent.set(spent.get() + timeout);
                Err(ErrorKind::TimedOut.into())
            } else {
                Ok(*address)
            }
        },
        || spent.get(),
    );
    assert_eq!(result, Ok(endpoints[1]), "a stalled first DNS result must not suppress fallback");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].1, Duration::from_secs(10));
    assert_eq!(attempts[1].1, Duration::from_secs(10));
}

#[test]
fn every_bounded_address_receives_time_when_prior_attempts_time_out() {
    let endpoints = addresses(MAX_ADDRESSES);
    let spent = Cell::new(Duration::ZERO);
    let mut attempts = 0;
    let result = connect_resolved(
        &endpoints,
        Duration::from_secs(20),
        |address, timeout| {
            attempts += 1;
            assert!(!timeout.is_zero());
            if attempts == MAX_ADDRESSES {
                Ok(*address)
            } else {
                spent.set(spent.get() + timeout);
                Err(ErrorKind::TimedOut.into())
            }
        },
        || spent.get(),
    );
    assert_eq!(result, Ok(endpoints[MAX_ADDRESSES - 1]));
    assert_eq!(attempts, MAX_ADDRESSES);
    assert!(spent.get() < Duration::from_secs(20));
}

#[test]
fn unused_time_from_a_refused_address_is_available_to_later_attempts() {
    let endpoints = addresses(3);
    let spent = Cell::new(Duration::ZERO);
    let mut waits = Vec::new();
    let result = connect_resolved(
        &endpoints,
        Duration::from_secs(18),
        |address, timeout| {
            waits.push(timeout);
            match waits.len() {
                1 => Err(ErrorKind::ConnectionRefused.into()),
                2 => {
                    spent.set(spent.get() + timeout);
                    Err(ErrorKind::TimedOut.into())
                }
                _ => Ok(*address),
            }
        },
        || spent.get(),
    );
    assert_eq!(result, Ok(endpoints[2]));
    assert_eq!(waits, [Duration::from_secs(6), Duration::from_secs(9), Duration::from_secs(9)]);
}

#[test]
fn one_address_retains_the_full_connect_budget() {
    let endpoints = addresses(1);
    let mut observed = Duration::ZERO;
    let result = connect_resolved(
        &endpoints,
        Duration::from_secs(20),
        |_, timeout| {
            observed = timeout;
            Ok(())
        },
        || Duration::ZERO,
    );
    assert_eq!(result, Ok(()));
    assert_eq!(observed, Duration::from_secs(20));
}

#[test]
fn an_exhausted_deadline_does_not_open_another_socket() {
    let mut attempts = 0;
    let result = connect_resolved::<()>(
        &addresses(2),
        Duration::ZERO,
        |_, _| {
            attempts += 1;
            Ok(())
        },
        || Duration::ZERO,
    );
    assert_eq!(result, Err(said!("the server didn't answer")));
    assert_eq!(attempts, 0);
}

#[test]
fn no_addresses_produces_a_lookup_result_not_a_socket_attempt() {
    let result =
        connect_resolved::<()>(&[], Duration::from_secs(20), |_, _| panic!("must not attempt a socket without an address"), || Duration::ZERO);
    assert_eq!(result, Err(said!("no address was found for the name")));
}

#[test]
fn all_timed_out_addresses_exhaust_only_the_total_budget() {
    let spent = Cell::new(Duration::ZERO);
    let mut attempts = 0;
    let budget = Duration::from_secs(20);
    let result = connect_resolved::<()>(
        &addresses(3),
        budget,
        |_, timeout| {
            attempts += 1;
            spent.set(spent.get() + timeout);
            Err(ErrorKind::TimedOut.into())
        },
        || spent.get(),
    );
    assert_eq!(result, Err(said!("the server didn't answer")));
    assert_eq!(attempts, 3);
    assert_eq!(spent.get(), budget);
}

#[test]
fn the_address_count_is_bounded_even_with_instant_failures() {
    let mut attempts = 0;
    let result = connect_resolved::<()>(
        &addresses(MAX_ADDRESSES + 3),
        Duration::from_secs(20),
        |_, _| {
            attempts += 1;
            Err(ErrorKind::ConnectionRefused.into())
        },
        || Duration::ZERO,
    );
    assert_eq!(result, Err(said!("the server refused the connection")));
    assert_eq!(attempts, MAX_ADDRESSES);
}

#[test]
fn extremely_small_budgets_never_pass_zero_to_connect_timeout() {
    let spent = Cell::new(Duration::ZERO);
    let mut attempts = 0;
    let result = connect_resolved::<()>(
        &addresses(MAX_ADDRESSES),
        Duration::from_nanos(3),
        |_, timeout| {
            assert!(!timeout.is_zero());
            assert!(timeout <= Duration::from_nanos(3).saturating_sub(spent.get()));
            attempts += 1;
            spent.set(spent.get() + timeout);
            Err(ErrorKind::TimedOut.into())
        },
        || spent.get(),
    );
    assert_eq!(result, Err(said!("the server didn't answer")));
    assert_eq!(attempts, 3);
    assert_eq!(spent.get(), Duration::from_nanos(3));
}

#[test]
fn scheduling_overrun_still_stops_at_the_global_deadline() {
    let spent = Cell::new(Duration::ZERO);
    let mut attempts = 0;
    let result = connect_resolved::<()>(
        &addresses(2),
        Duration::from_secs(20),
        |_, _| {
            attempts += 1;
            spent.set(Duration::from_secs(21));
            Err(ErrorKind::TimedOut.into())
        },
        || spent.get(),
    );
    assert_eq!(result, Err(said!("the server didn't answer")));
    assert_eq!(attempts, 1);
}

#[test]
fn fallback_reaches_a_real_loopback_socket_after_a_simulated_blackhole() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let reachable = listener.local_addr().unwrap();
    let endpoints = [SocketAddr::from(([127, 0, 0, 1], 1)), reachable];
    let spent = Cell::new(Duration::ZERO);
    let mut calls = 0;
    let result = connect_resolved(
        &endpoints,
        Duration::from_secs(2),
        |address, timeout| {
            calls += 1;
            if calls == 1 {
                spent.set(timeout);
                Err(ErrorKind::TimedOut.into())
            } else {
                TcpStream::connect_timeout(address, timeout)
            }
        },
        || spent.get(),
    );
    let socket = result.expect("a reachable fallback must receive a connection attempt");
    assert_eq!(socket.peer_addr().unwrap(), reachable);
    let (_peer, _) = listener.accept().unwrap();
    assert_eq!(calls, 2);
}
