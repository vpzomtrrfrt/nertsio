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
        ("cards_hivis", (1700, 770)),
        ("backs", (270, 200)),
        ("placeholder", (140, 200)),
        ("cursors", (40, 80)),
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

    {
        let content = std::fs::read("res/icon.svg").unwrap();
        let tree = usvg::Tree::from_data(&content, &usvg::Options::default().to_ref()).unwrap();

        for size in [16, 32, 64] {
            let mut pixmap = tiny_skia::Pixmap::new(size, size).unwrap();
            resvg::render(
                &tree,
                usvg::FitTo::Size(pixmap.width(), pixmap.height()),
                pixmap.as_mut(),
            )
            .unwrap();

            let bytes = pixmap.data();
            std::fs::write(
                format!(
                    "{}{}icon-{}",
                    std::env::var("OUT_DIR").unwrap(),
                    std::path::MAIN_SEPARATOR,
                    size,
                ),
                bytes,
            )
            .unwrap();

            writeln!(
                mod_output,
                "pub const ICON_PIXELS_{0}: &[u8] = include_bytes!(\"icon-{0}\");",
                size,
            )
            .unwrap();
        }
    }

    {
        let android_res_dir = std::env::current_dir()
            .unwrap()
            .parent()
            .unwrap()
            .join("target")
            .join("android_res");
        let mipmap_dir = android_res_dir.join("mipmap");

        std::fs::create_dir_all(&mipmap_dir).unwrap();

        let content = std::fs::read("res/icon.svg").unwrap();
        let tree = usvg::Tree::from_data(&content, &usvg::Options::default().to_ref()).unwrap();

        let mut pixmap = tiny_skia::Pixmap::new(512, 512).unwrap();
        resvg::render(
            &tree,
            usvg::FitTo::Size(pixmap.width(), pixmap.height()),
            pixmap.as_mut(),
        )
        .unwrap();

        let bytes = pixmap.encode_png().unwrap();
        std::fs::write(mipmap_dir.join("icon.png"), bytes).unwrap();
    }

    {
        let mac_res_dir = std::env::current_dir()
            .unwrap()
            .parent()
            .unwrap()
            .join("target")
            .join("mac_res");
        let iconset_dir = mac_res_dir.join("nertsio.iconset");

        std::fs::create_dir_all(&iconset_dir).unwrap();

        let content = std::fs::read("res/icon.svg").unwrap();
        let tree = usvg::Tree::from_data(&content, &usvg::Options::default().to_ref()).unwrap();

        for base_size in [16, 32, 128, 256, 512] {
            for density in [1, 2] {
                let real_size = base_size * density;
                let filename = format!(
                    "icon_{0}x{0}{1}.png",
                    base_size,
                    match density {
                        1 => "",
                        2 => "@2x",
                        _ => unreachable!(),
                    },
                );

                let mut pixmap = tiny_skia::Pixmap::new(real_size, real_size).unwrap();
                resvg::render(
                    &tree,
                    usvg::FitTo::Size(pixmap.width(), pixmap.height()),
                    pixmap.as_mut(),
                )
                .unwrap();

                let bytes = pixmap.encode_png().unwrap();
                std::fs::write(iconset_dir.join(filename), bytes).unwrap();
            }
        }
    }

    {
        let ios_res_dir = std::env::current_dir()
            .unwrap()
            .parent()
            .unwrap()
            .join("target")
            .join("ios_res");
        let iconset_dir = ios_res_dir.join("nertsio.iconset");

        std::fs::create_dir_all(&iconset_dir).unwrap();

        let content = std::fs::read("res/icon.svg").unwrap();
        let tree = usvg::Tree::from_data(&content, &usvg::Options::default().to_ref()).unwrap();

        for (base_size, density) in [(1024, 1)] {
            let real_size = base_size * density;
            let filename = format!(
                "icon_{0}x{0}{1}.png",
                base_size,
                match density {
                    1 => "",
                    2 => "@2x",
                    3 => "@3x",
                    _ => unreachable!(),
                },
            );

            let mut pixmap = tiny_skia::Pixmap::new(real_size, real_size).unwrap();
            resvg::render(
                &tree,
                usvg::FitTo::Size(pixmap.width(), pixmap.height()),
                pixmap.as_mut(),
            )
            .unwrap();

            let bytes = pixmap.encode_png().unwrap();
            std::fs::write(iconset_dir.join(filename), bytes).unwrap();
        }
    }
}
