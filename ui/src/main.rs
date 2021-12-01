use macroquad::prelude as mq;

const BACKGROUND_COLOR: mq::Color = mq::Color::new(0.2, 0.7, 0.2, 1.0);

#[macroquad::main("nertsio")]
async fn main() {
    let cards_texture = mq::Texture2D::from_file_with_format(nertsio_textures::CARDS, Some(mq::ImageFormat::Png));

    loop {
        mq::clear_background(BACKGROUND_COLOR);
        mq::draw_texture_ex(cards_texture, 10.0, 10.0, mq::WHITE, mq::DrawTextureParams {
            source: Some(mq::Rect {
                x: 10.0,
                y: 10.0,
                w: 120.0,
                h: 180.0,
            }),
            ..Default::default()
        });

        mq::next_frame().await
    }
}
