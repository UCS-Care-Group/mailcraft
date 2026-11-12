// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn only_absolute_executables_are_considered() {
    assert!(absolute_file(PathBuf::from("pdfcraft-cli")).is_none());
    let dir = tempfile::tempdir().unwrap();
    assert!(absolute_file(dir.path().join("missing.exe")).is_none());
}

#[test]
fn pdf_page_counts_are_bounded() {
    assert_eq!(pdf_pages(br#"{"pages":2}"#), Ok(2));
    assert_eq!(pdf_pages(br#"{"pages":[{},{}]}"#), Ok(2));
    assert_eq!(pdf_pages(br#"{"pages":0}"#), Err(Error::Invalid));
    assert_eq!(pdf_pages(br#"{"pages":10001}"#), Err(Error::Invalid));
}

#[test]
fn only_bounded_png_output_is_decoded() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("preview.png");
    fs::write(&file, b"<html>not an image</html>").unwrap();
    assert_eq!(read_bounded(&file, 3), Err(Error::TooLarge));
    assert_eq!(decode_png(&read_bounded(&file, MAX_PNG_BYTES).unwrap()), Err(Error::Invalid));
    let png = image::RgbaImage::new(MAX_SIDE + 1, 1);
    png.save(&file).unwrap();
    assert_eq!(decode_png(&fs::read(&file).unwrap()), Err(Error::Invalid));
}

#[test]
fn no_user_configuration_paths_are_passed_to_the_helper() {
    let dir = tempfile::tempdir().unwrap();
    let command = command(Path::new("helper"), dir.path());
    assert_eq!(command.get_current_dir(), Some(dir.path()));
    assert_eq!(command.get_args().count(), 0);
    for (name, value) in command.get_envs() {
        if name != "SystemRoot" {
            assert_eq!(value, Some(dir.path().as_os_str()));
        }
    }
}

#[cfg(any(windows, unix))]
#[test]
fn a_slow_helper_is_terminated() {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(windows)]
    let (program, args) = (
        PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/WindowsPowerShell/v1.0/powershell.exe"),
        vec!["-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), "Start-Sleep -Seconds 10".into()],
    );
    #[cfg(unix)]
    let (program, args) = (PathBuf::from("/bin/sleep"), vec!["10".into()]);
    let now = Instant::now();
    let result = run_helper(
        &program,
        dir.path(),
        &args,
        &dir.path().join("preview.png"),
        MAX_PNG_BYTES,
        Stdio::null(),
        now + Duration::from_millis(100),
        &AtomicBool::new(false),
    );
    assert_eq!(result, Err(Error::TimedOut));
    assert!(now.elapsed() < Duration::from_secs(5));
}
