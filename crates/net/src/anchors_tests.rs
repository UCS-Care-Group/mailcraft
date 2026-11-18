// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The authorities trusted for mail, read from a bundle the system's trust store extracts
//! (#30): never the web sites' set.

use crate::anchors::{MAIL_BUNDLES, bundle, bundles};

/// A PEM bundle of two certificates (made here, of no authority anybody trusts) and junk.
#[test]
fn a_bundle_gives_its_certificates_and_leaves_out_the_rest() {
    let a = rcgen::generate_simple_self_signed(vec!["a.example".to_string()]).unwrap();
    let b = rcgen::generate_simple_self_signed(vec!["b.example".to_string()]).unwrap();
    let dir = std::env::temp_dir().join(format!("mailcraft-anchors-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("email-ca-bundle.pem");
    let pem = |der: &[u8]| format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", mailcraft_charset::base64::STANDARD.encode(der));
    let text = format!("# junk\n{}\nnot a certificate\n{}", pem(a.cert.der()), pem(b.cert.der()));
    std::fs::write(&path, text).unwrap();
    let found = bundle(&path).unwrap();
    assert_eq!(found, vec![a.cert.der().to_vec(), b.cert.der().to_vec()]);
    std::fs::write(&path, "nothing here").unwrap();
    assert_eq!(bundle(&path), None);
    assert_eq!(bundle(&dir.join("missing.pem")), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Only the bundles of authorities trusted for mail are read, never the web sites' set.
#[test]
fn only_the_mail_bundles_are_read() {
    assert!(MAIL_BUNDLES.iter().all(|p| p.ends_with("email-ca-bundle.pem")));
    // FreeBSD keeps no set for mail: what its ports put under /usr/local is the web sites' set.
    assert!(MAIL_BUNDLES.iter().all(|p| !p.starts_with("/usr/local/")));
}

/// NSS's list stands in only where none of the bundles for mail is there: one that is there but
/// empty or unreadable is the system's word that no authority is trusted for mail.
#[test]
fn nss_s_list_stands_in_only_where_no_bundle_for_mail_is_there() {
    let a = rcgen::generate_simple_self_signed(vec!["a.example".to_string()]).unwrap();
    let dir = std::env::temp_dir().join(format!("mailcraft-bundles-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (first, second, missing) = (dir.join("first.pem"), dir.join("second.pem"), dir.join("missing.pem"));
    let pem = format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", mailcraft_charset::base64::STANDARD.encode(a.cert.der()));
    assert_eq!(bundles(&[&missing, &dir.join("no-folder/x.pem")]), None, "nothing there: NSS's list may stand in");
    std::fs::write(&first, "").unwrap();
    assert_eq!(bundles(&[&missing, &first]), Some(Vec::new()), "an emptied bundle trusts none, and NSS's list doesn't stand in");
    std::fs::write(&second, &pem).unwrap();
    assert_eq!(bundles(&[&first, &second]), Some(vec![a.cert.der().to_vec()]), "the first with certificates is read");
    std::fs::remove_file(&first).unwrap();
    std::os::unix::fs::symlink(&missing, &first).unwrap();
    assert_eq!(bundles(&[&first]), Some(Vec::new()), "a link to nothing is there, not missing");
    let _ = std::fs::remove_dir_all(&dir);
}
