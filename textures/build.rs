use std::io::Write;

fn main() {
    println!("cargo:rerun-if-changed=res");
    let mut mod_output = std::fs::File::create(format!(
        "{}{}generated.rs",
        std::env::var("OUT_DIR").unwrap(),
        std::path::MAIN_SEPARATOR
    ))
    .unwrap();

    for (name, size) in [
        ("cards", (1700, 770)),
        ("backs", (270, 200)),
        ("placeholder", (140, 200)),
    ] {
        let content = std::fs::read(format!("res/{}.svg", name)).unwrap();
        let tree = usvg::Tree::from_data(&content, &usvg::Options::default().to_ref()).unwrap();

        let mut pixmap = tiny_skia::Pixmap::new(size.0, size.1).unwrap();
        resvg::render(
            &tree,
            usvg::FitTo::Size(pixmap.width(), pixmap.height()),
            pixmap.as_mut(),
        )
        .unwrap();

        pixmap
            .save_png(format!(
                "{}{}{}.png",
                std::env::var("OUT_DIR").unwrap(),
                std::path::MAIN_SEPARATOR,
                name,
            ))
            .unwrap();

        writeln!(
            mod_output,
            "pub const {}: &[u8] = include_bytes!(\"{}.png\");",
            name.to_uppercase(),
            name,
        )
        .unwrap();
    }
}
