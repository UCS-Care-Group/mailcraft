// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use mailcraft_ui_egui::printing::Job;

const LIST: &str = "Office_Laser\nHome_Inkjet\n-o evil\nOffice_Laser\n";
const DEFAULT: &str = "system default destination: Home_Inkjet\n";
const LONG: &str = "printer Office_Laser is idle.  enabled since Thu Aug  6 13:43:26 2026\n\tForm mounted:\n\tDescription: Laser on the second floor\n\tLocation: Room 2\nprinter Home_Inkjet disabled since Mon\n\tDescription: \n";

#[test]
fn printers_are_listed_default_first_with_their_descriptions() {
    let printers = parse_printers(LIST, parse_default(DEFAULT).as_deref(), &parse_descriptions(LONG));
    let names: Vec<&str> = printers.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Home_Inkjet", "Office_Laser"], "the default first, no duplicates, nothing option-like");
    assert!(printers[0].default);
    assert_eq!(printers[0].label, "Home Inkjet");
    assert_eq!(printers[1].label, "Laser on the second floor");
    assert_eq!(parse_default("no system default destination\n"), None);
}

#[test]
fn a_job_becomes_lp_arguments_with_the_file_last() {
    let job = Job {
        printer: "Office_Laser".into(),
        title: "Quarterly\nfigures".into(),
        pdf: Vec::new(),
        copies: 500,
        pages: Some("1-3,5".into()),
        two_sided: true,
        color: false,
        paper: "A4",
    };
    let args = lp_args(&job, "/tmp/x/Quarterly figures.pdf");
    assert_eq!(
        args,
        [
            "-d",
            "Office_Laser",
            "-n",
            "99",
            "-t",
            "Quarterlyfigures",
            "-P",
            "1-3,5",
            "-o",
            "media=A4",
            "-o",
            "sides=two-sided-long-edge",
            "-o",
            "print-color-mode=monochrome",
            "-o",
            "ColorModel=Gray",
            "--",
            "/tmp/x/Quarterly figures.pdf"
        ]
    );
    assert!(!usable_name("-dfoo") && !usable_name("a b") && usable_name("Office_Laser"));
}
