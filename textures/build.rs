use std::io::Write;

fn main() {
    println!("cargo:rerun-if-changed=res");
    let mut mod_output = std::fs::File::create(format!(
        "{}{}generated.rs",
        std::env::var("OUT_DIR").unwrap(),
        std::path::MAIN_SEPARATOR
    ))
    .unwrap();

    {
        let content = std::fs::read("res/cards.svg").unwrap();
        let tree = usvg::Tree::from_data(&content, &usvg::Options::default().to_ref()).unwrap();

        let mut pixmap = tiny_skia::Pixmap::new(1700, 770).unwrap();
        resvg::render(
            &tree,
            usvg::FitTo::Size(pixmap.width(), pixmap.height()),
            pixmap.as_mut(),
        )
        .unwrap();

        pixmap
            .save_png(format!(
                "{}{}cards.png",
                std::env::var("OUT_DIR").unwrap(),
                std::path::MAIN_SEPARATOR
            ))
            .unwrap();

        writeln!(
            mod_output,
            "pub const CARDS: &[u8] = include_bytes!(\"cards.png\");"
        )
        .unwrap();
    }
}
