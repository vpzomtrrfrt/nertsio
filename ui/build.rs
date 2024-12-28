use std::io::Write;

fn main() {
    let mut licenses = bureau::gather_licenses(bureau::Options {
        accepted_licenses: &[
            "MIT",
            "ISC",
            "BSD-2-Clause",
            "BSD-3-Clause",
            "Apache-2.0",
            "BSL-1.0",
            "Unicode-DFS-2016",
            "Unicode-3.0",
            "Zlib",
            "MPL-2.0",
            "AGPL-3.0-or-later",
            "OFL-1.1",
            "LicenseRef-UFL-1.0",
        ],
        path_overrides: &[
            &bureau::PathOverride {
                crate_name: "epaint",
                license_id: Some("LicenseRef-UFL-1.0"),
                path: std::path::Path::new("fonts/UFL.txt"),
            },
            &bureau::PathOverride {
                crate_name: "epaint",
                license_id: Some("OFL-1.1"),
                path: std::path::Path::new("fonts/OFL.txt"),
            },
        ],
        ..Default::default()
    });

    licenses.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));

    let mut mod_output = std::fs::File::create(format!(
        "{}{}generated.rs",
        std::env::var("OUT_DIR").unwrap(),
        std::path::MAIN_SEPARATOR
    ))
    .unwrap();

    write!(mod_output, "pub const LICENSES: &[(&str, &str)] = &[").unwrap();

    for license in licenses {
        write!(
            mod_output,
            "({:?}, {:?}),",
            format!("{} {}", license.crate_name, license.version),
            license.license_text
        )
        .unwrap();
    }

    write!(mod_output, "];").unwrap();
}
