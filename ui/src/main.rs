use macroquad::prelude as mq;
use nertsio_types as ni_ty;

const BACKGROUND_COLOR: mq::Color = mq::Color::new(0.2, 0.7, 0.2, 1.0);

fn get_card_rect(card: ni_ty::Card) -> mq::Rect {
    const SPACING: f32 = 10.0;
    const WIDTH: f32 = 120.0;
    const HEIGHT: f32 = 180.0;

    let x = SPACING + (f32::from(card.rank.value() - 1) * (WIDTH + SPACING));
    let y = SPACING
        + ((match card.suit {
            ni_ty::Suit::Spades => 0.0,
            ni_ty::Suit::Hearts => 1.0,
            ni_ty::Suit::Diamonds => 2.0,
            ni_ty::Suit::Clubs => 3.0,
        }) * (HEIGHT + SPACING));

    mq::Rect {
        x,
        y,
        w: WIDTH,
        h: HEIGHT,
    }
}

#[macroquad::main("nertsio")]
async fn main() {
    let cards_texture =
        mq::Texture2D::from_file_with_format(nertsio_textures::CARDS, Some(mq::ImageFormat::Png));
    let backs_texture =
        mq::Texture2D::from_file_with_format(nertsio_textures::BACKS, Some(mq::ImageFormat::Png));

    let draw_card = |card: ni_ty::Card, x: f32, y: f32| {
        mq::draw_texture_ex(
            cards_texture,
            x,
            y,
            mq::WHITE,
            mq::DrawTextureParams {
                source: Some(get_card_rect(card)),
                ..Default::default()
            },
        );
    };

    let draw_back = |x: f32, y: f32| {
        mq::draw_texture_ex(
            backs_texture,
            x,
            y,
            mq::WHITE,
            mq::DrawTextureParams {
                source: Some(mq::Rect {
                    x: 10.0,
                    y: 10.0,
                    w: 120.0,
                    h: 180.0,
                }),
                ..Default::default()
            },
        );
    };

    let player_state = ni_ty::HandPlayerState::generate(0, 4);

    println!("{:?}", player_state.nerts_stack().len());

    loop {
        mq::clear_background(BACKGROUND_COLOR);

        for i in 0..(player_state.nerts_stack().len() - 1) {
            draw_back(10.0 + (i as f32) * 10.0, 10.0);
        }
        if let Some(card) = player_state.nerts_stack().last() {
            draw_card(
                card.card,
                10.0 + ((player_state.nerts_stack().len() - 1) as f32) * 10.0,
                10.0,
            );
        }

        for (i, stack) in player_state.tableau_stacks().iter().enumerate() {
            let card = stack.last().unwrap();
            draw_card(card.card, 140.0 + 120.0 + (i as f32) * 130.0, 10.0);
        }

        mq::next_frame().await
    }
}
