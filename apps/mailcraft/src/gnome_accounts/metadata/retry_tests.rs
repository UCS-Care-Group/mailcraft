// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn metadata_retries_one_transient_failure_across_a_snapshot() {
    let mut read = MetadataRead { started: Instant::now(), retried: false };
    let mut calls = 0;
    assert_eq!(
        read.call(|| {
            calls += 1;
            if calls == 1 { Err(std::io::Error::from(ErrorKind::TimedOut).into()) } else { Ok("metadata") }
        }),
        Ok("metadata")
    );
    assert_eq!(calls, 2);
    assert_eq!(read.call::<()>(|| Err(std::io::Error::from(ErrorKind::TimedOut).into())), Err(DiscoveryError::Unavailable));
}

#[test]
fn metadata_does_not_retry_denied_or_expired_requests() {
    let mut read = MetadataRead { started: Instant::now(), retried: false };
    let mut calls = 0;
    assert_eq!(
        read.call::<()>(|| {
            calls += 1;
            Err(zbus::fdo::Error::AccessDenied("private detail".into()).into())
        }),
        Err(DiscoveryError::Failed)
    );
    assert_eq!(calls, 1);
    assert!(!read.retried);
    read.started -= BUDGET;
    assert_eq!(read.call(|| panic!("expired discovery cannot issue another call")), Err::<(), _>(DiscoveryError::Unavailable));
}
