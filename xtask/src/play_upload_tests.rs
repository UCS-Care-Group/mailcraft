// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use std::cell::RefCell;
use std::io::{BufRead as _, BufReader};
use std::net::TcpListener;

use ring::signature::{KeyPair as _, RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};

use super::*;

/// A throwaway RSA key (PKCS#8 DER in base64), made up for these tests and used nowhere else.
const TEST_KEY: &[&str] = &[
    "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCZsx8Kv08B/DUxtg1AB5i5lAu0mUusufKFzO/g2DVfa9kw",
    "So6uFTK/hKq+wzseEjpOjEPtNJ/BBNydD4F7rWfEG+z0UAWR33oGPwpJhTjzcy1stHzBVZbEG/tNjtAJLEAqHFFvzgWyP5Ac",
    "Kv8WQztXo3odnLgYbDAj7D/a8zVqUP7emqyAbrj5tDRLq63T5LDEK44A2fv7kohan/iSOluRw8Q9VL4i7PaoyWVnbwcdEtNu",
    "RHACuivq4BCeKv+7vU8cmjKb/UKv0xeXgwjVkWtB8ge3xMBKFhP6hT8rnxzGjhw7vrUf3FVC/dpHhNXO4U2nWWzEVF4C/htv",
    "4c9pxRjNAgMBAAECggEACqJsnus+tbX0HkTo5MnBxPFb3/qNVF2nTJ+x5z4+byRAqkk2VxAraznsMHfEot+C/kVRO9vcw9eo",
    "xgs5G8wTNmvyOLNCt2WdhUoeVZjsqFeLSI6yZRQdcuM2v5aGiWPw6pj7/37jQQKJ35EQBkPPmtA6VksuJ65wpEOn5WfXhcwK",
    "V1ICUUQOJ1JNUnBoUgM6QYKEAf/s2vIG9Dstay35rVWiO+KTfMvLKqMDg8i66Nwt3cDe96oqCMoqPwfnJ+0fW1k8h1kRpr9Z",
    "bODJIdCLI0SPX0Iv6DqN+54ELfPJIcCh+d5HaOUAjV2B/DKVahTTBHYTWBxrQVk4breQE8haUQKBgQDV6Oah7cFRZ60WXcqh",
    "d78AckPA9pf17liqSAHS4MHlzamoicziPYvj7ynjq0/S9zHxG7WlEhODEBqAVMbudPLaFuG8GDyrqlCtT8nXHintCb3dLrJl",
    "bSnB558xAqYEjDsgAj8Pyqj0O0MBrmIg4KsPZ8l+lh0drTNu5ZpJJDiXJQKBgQC38U7kelLhv4/m3vY0ZE1rLRSuuOiTw4BG",
    "G034nwPxW+xKTgFU1QTwYCFKa1kA9yfzEMXo3Bf0pkGfsCklIWa2JWMbuoJY368P6M8SArwmzth5PtQhiV7NMd1mmalKDFNl",
    "UUfLizS1cIJJGoAxEW1PesBLxi5BDc/Ec+xNmTh+iQKBgFhv1MJHwK/+NBJWLos5La9qjlZ93O+hXejwokhkAeYXdDgo2JsE",
    "pQxUebYVwofPCUfMk/muEoESLFK4OhKmKS0/Xo5cWuECJh6xKRqV7vGAII4MW1cRnbz8HNWvaaHhtCQUqavGtrlasuZECW/c",
    "UvyohSafu2PkkqobpQDuR7M9AoGAW7TBIoQiAk7hTbdnhGNlfcOyBhJGUjUsJMfELeoJqzGrMriFTWgO2FD1CAdRDnwqvLJm",
    "GVxxzRV+rkDDorFuZe1p5g8soWaT/xiQ9GfMqoSBxKEgXcABNchgm6CMmFBg26iUZNR4aOU473143/3SQ8ftS1n5AZIDPeyV",
    "qXJ0ihECgYEAsSRuHH2xur/PTKm5gGzGMGhNeVbl6+pinyPpOTnz8LfIwGZF3FtACkTbVJn5EH56mGkVsyZgQ3uQcg8UFtDr",
    "3lX46MC9OU6sCofhAwRcUiPMmpH+97t2JpI70nNq+WcpGU47fSybJARXagcXyFez9M+gF3dKNI38jMR0zMGBzNc=",
];

const EMAIL: &str = "uploader@made-up.iam.gserviceaccount.example";

fn account_json() -> String {
    let b64: String = TEST_KEY.concat();
    let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap()).collect();
    let pem = format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", lines.join("\n"));
    serde_json::json!({ "type": "service_account", "client_email": EMAIL, "private_key": pem }).to_string()
}

fn decode_json(part: &str) -> serde_json::Value {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).unwrap()).unwrap()
}

#[test]
fn the_jwt_has_googles_header_and_claims_and_a_good_signature() {
    let account = Account::parse(&account_json()).unwrap();
    let jwt = account.jwt(1_800_000_000).unwrap();
    let parts: Vec<&str> = jwt.split('.').collect();
    assert_eq!(parts.len(), 3);
    assert_eq!(decode_json(parts[0]), serde_json::json!({ "alg": "RS256", "typ": "JWT" }));
    let claims = decode_json(parts[1]);
    assert_eq!(claims["iss"], EMAIL);
    assert_eq!(claims["scope"], "https://www.googleapis.com/auth/androidpublisher");
    assert_eq!(claims["aud"], "https://oauth2.googleapis.com/token");
    assert_eq!(claims["iat"], 1_800_000_000u64);
    assert_eq!(claims["exp"], 1_800_003_600u64);
    let pair = RsaKeyPair::from_pkcs8(&STANDARD.decode(TEST_KEY.concat()).unwrap()).unwrap();
    let public = UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, pair.public_key().as_ref().to_vec());
    let signed = format!("{}.{}", parts[0], parts[1]);
    public.verify(signed.as_bytes(), &URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
}

#[test]
fn a_bad_account_is_refused_without_quoting_it() {
    let secret = "do-not-echo-me";
    let bad = [
        String::new(),
        format!("{{\"client_email\":\"{EMAIL}\",\"private_key\":\"{secret}\"}}"),
        format!("{{\"client_email\":\"{EMAIL}\",\"private_key\":\"-----BEGIN PRIVATE KEY-----{secret}-----END PRIVATE KEY-----\"}}"),
        format!("{{\"private_key\":\"{secret}\"}}"),
        format!("{{\"client_email\":\"a\\\"b\",\"private_key\":\"{secret}\"}}"),
        "x".repeat(20_000),
    ];
    for json in &bad {
        let Err(e) = Account::parse(json) else { panic!("accepted a bad account") };
        assert!(!e.contains(secret) && e.starts_with("PLAY_SERVICE_ACCOUNT_JSON"), "{e}");
    }
}

/// Records each request (as curl would get it) and answers from a list.
struct Fake {
    seen: RefCell<Vec<(String, String)>>,
    answers: RefCell<Vec<(u16, &'static str)>>,
}

impl Http for Fake {
    fn send(&self, request: &Request) -> Result<(u16, Vec<u8>), String> {
        self.seen.borrow_mut().push((request.step.to_string(), curl_config(request)));
        let (status, body) = self.answers.borrow_mut().remove(0);
        Ok((status, body.as_bytes().to_vec()))
    }
}

fn fake(answers: Vec<(u16, &'static str)>) -> Fake {
    Fake { seen: RefCell::new(Vec::new()), answers: RefCell::new(answers) }
}

fn aab(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("mailcraft-play-upload-{name}-{}.aab", std::process::id()));
    std::fs::write(&path, b"PK made up").unwrap();
    path
}

const TOKEN: &str = "ya29.made-up-token";
const OK: [(u16, &str); 5] = [
    (200, r#"{"access_token":"ya29.made-up-token","token_type":"Bearer","expires_in":3599}"#),
    (200, r#"{"id":"edit-17","expiryTimeSeconds":"1800003600"}"#),
    (200, r#"{"versionCode":1203,"sha1":"ab","sha256":"cd"}"#),
    (200, r#"{"track":"internal","releases":[]}"#),
    (200, r#"{"id":"edit-17"}"#),
];

#[test]
fn the_five_steps_go_to_googles_addresses_in_order() {
    let http = fake(OK.to_vec());
    let at = Endpoints { token: TOKEN_URL.into(), api: "https://androidpublisher.googleapis.com".into() };
    let account = Account::parse(&account_json()).unwrap();
    let file = aab("steps");
    assert_eq!(upload(&http, &at, &account, &file, 1_800_000_000).unwrap(), 1203);
    let seen = http.seen.borrow();
    let steps: Vec<&str> = seen.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(steps, ["token", "edits.insert", "edits.bundles.upload", "edits.tracks.update", "edits.commit"]);
    let edits = "https://androidpublisher.googleapis.com/androidpublisher/v3/applications/work.mailcraft.android/edits";
    let token = &seen[0].1;
    assert!(token.starts_with("url = \"https://oauth2.googleapis.com/token\"\nrequest = \"POST\"\n"), "{token}");
    assert!(token.contains("data-binary = \"grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion=ey"));
    assert!(!token.contains("Authorization"));
    assert!(seen[1].1.starts_with(&format!("url = \"{edits}\"\nrequest = \"POST\"\n")));
    let upload_url = "https://androidpublisher.googleapis.com/upload/androidpublisher/v3/applications/work.mailcraft.android/edits/edit-17/bundles?uploadType=media";
    assert!(seen[2].1.starts_with(&format!("url = \"{upload_url}\"\nrequest = \"POST\"\n")));
    assert!(seen[2].1.contains("header = \"Content-Type: application/octet-stream\"\n"));
    assert!(seen[2].1.contains(&format!("upload-file = \"{}\"\n", file.display().to_string().replace('\\', "\\\\"))));
    assert!(seen[3].1.starts_with(&format!("url = \"{edits}/edit-17/tracks/internal\"\nrequest = \"PUT\"\n")));
    assert!(seen[3].1.contains(r#"data-binary = "{\"releases\":[{\"status\":\"draft\",\"versionCodes\":[\"1203\"]}],\"track\":\"internal\"}""#));
    assert!(seen[4].1.starts_with(&format!("url = \"{edits}/edit-17:commit\"\nrequest = \"POST\"\n")));
    assert!(seen[4].1.ends_with("data-binary = \"\"\n"));
    for (_, config) in &seen[1..] {
        assert!(config.contains(&format!("header = \"Authorization: Bearer {TOKEN}\"\n")));
    }
    let _ = std::fs::remove_file(file);
}

#[test]
fn a_failed_step_says_its_name_and_status_and_nothing_else() {
    let mut answers = OK.to_vec();
    answers[2] = (403, r#"{"error":{"message":"ya29.made-up-token is not allowed"}}"#);
    let http = fake(answers);
    let at = Endpoints { token: TOKEN_URL.into(), api: "https://androidpublisher.googleapis.com".into() };
    let file = aab("failed");
    let e = upload(&http, &at, &Account::parse(&account_json()).unwrap(), &file, 0).unwrap_err();
    assert_eq!(e, "edits.bundles.upload: HTTP 403");
    assert_eq!(http.seen.borrow().len(), 3, "nothing after a failed step");
    let _ = std::fs::remove_file(file);
}

#[test]
fn a_token_or_an_edit_id_that_could_break_out_is_refused() {
    let at = Endpoints { token: TOKEN_URL.into(), api: "https://androidpublisher.googleapis.com".into() };
    let file = aab("breakout");
    let account = Account::parse(&account_json()).unwrap();
    let mut answers = OK.to_vec();
    answers[0] = (200, "{\"access_token\":\"a\\\"\\nurl = \\\"http://elsewhere\"}");
    assert_eq!(upload(&fake(answers), &at, &account, &file, 0).unwrap_err(), "token: no usable access_token in the answer");
    let mut answers = OK.to_vec();
    answers[1] = (200, r#"{"id":"../../x"}"#);
    assert_eq!(upload(&fake(answers), &at, &account, &file, 0).unwrap_err(), "edits.insert: no usable edit id");
    let mut answers = OK.to_vec();
    answers[2] = (200, r#"{"versionCode":-4}"#);
    assert_eq!(upload(&fake(answers), &at, &account, &file, 0).unwrap_err(), "edits.bundles.upload: no versionCode");
    let _ = std::fs::remove_file(file);
}

#[test]
fn curls_command_line_holds_nothing_of_the_request() {
    let args = Curl { proto: "=https" }.args();
    assert_eq!(args[..3], ["--config", "-", "--silent"]);
    assert!(args.windows(2).any(|w| w == ["--proto", "=https"]) && args.windows(2).any(|w| w == ["--proto-redir", "=https"]));
    assert!(!args.iter().any(|a| a.contains("googleapis") || a.contains("Bearer") || a.starts_with("http")));
}

#[test]
fn config_values_are_quoted_and_cannot_add_a_line() {
    let request = Request {
        step: "t",
        method: "POST",
        url: "https://h.example/a\"b\\c\nurl = \"http://x\"".into(),
        headers: vec!["X: 1\r\nupload-file = \"/etc/passwd\"".into()],
        body: Body::Text(String::new()),
    };
    let config = curl_config(&request);
    assert_eq!(config.lines().count(), 4, "{config}");
    assert!(config.starts_with(r#"url = "https://h.example/a\"b\\curl = \"http://x\"""#), "{config}");
}

#[test]
fn curls_output_splits_into_status_and_body() {
    assert_eq!(split_output(b"{\"a\":1}\n200"), Some((200, b"{\"a\":1}".to_vec())));
    assert_eq!(split_output(b"\n204"), Some((204, Vec::new())));
    assert_eq!(split_output(b"no status"), None);
}

/// Real curl against a server on loopback: the request arrives whole, from the config on stdin.
#[test]
fn curl_sends_what_the_config_says() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut head = Vec::new();
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().unwrap();
            }
            if line == "\r\n" {
                break;
            }
            head.push(line);
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let answer = r#"{"id":"edit-1"}"#;
        let mut stream = stream;
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len())
            .unwrap();
        (head, String::from_utf8(body).unwrap())
    });
    let request = Request {
        step: "edits.insert",
        method: "POST",
        url: format!("http://127.0.0.1:{port}/androidpublisher/v3/applications/{PACKAGE}/edits"),
        headers: vec![format!("Authorization: Bearer {TOKEN}"), "Content-Type: application/json".into()],
        body: Body::Text("{}".into()),
    };
    let (status, body) = Curl { proto: "=http" }.send(&request).unwrap();
    assert_eq!((status, body.as_slice()), (200, br#"{"id":"edit-1"}"#.as_slice()));
    let (head, sent) = server.join().unwrap();
    assert_eq!(head[0], format!("POST /androidpublisher/v3/applications/{PACKAGE}/edits HTTP/1.1\r\n"));
    assert!(head.contains(&format!("Authorization: Bearer {TOKEN}\r\n")), "{head:?}");
    assert_eq!(sent, "{}");
}
