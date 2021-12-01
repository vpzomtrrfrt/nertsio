use macroquad::prelude as mq;
use nertsio_types as ni_ty;

const BACKGROUND_COLOR: mq::Color = mq::Color::new(0.2, 0.7, 0.2, 1.0);

const CARD_WIDTH: f32 = 90.0;
const CARD_HEIGHT: f32 = 135.0;

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
    let card_size = mq::Vec2::new(CARD_WIDTH, CARD_HEIGHT);

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
                dest_size: Some(card_size),
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
                dest_size: Some(card_size),
                ..Default::default()
            },
        );
    };

    let draw_vertical_stack_cards = |cards: &[ni_ty::CardInstance], x: f32, y: f32| {
        for (i, card) in cards.iter().enumerate() {
            draw_card(card.card, x, y + (i as f32) * 10.0);
        }
    };

    let player_state = ni_ty::HandPlayerState::generate(0, 4);
    let mut held_cards: Option<(ni_ty::PlayerStackLocation, usize)> = None;

    println!("{:?}", player_state.nerts_stack().len());

    loop {
        let mouse_pos = mq::mouse_position();

        if mq::is_mouse_button_pressed(mq::MouseButton::Left) {
            let mouse_vec = mouse_pos.into();
            let found = if mq::Rect::new(
                10.0 + ((player_state.nerts_stack().len() - 1) as f32) * 10.0,
                10.0,
                CARD_WIDTH,
                CARD_HEIGHT,
            )
            .contains(mouse_vec)
            {
                Some((ni_ty::PlayerStackLocation::Nerts, 1))
            } else {
                None
            };

            if let Some(found) = found {
                match held_cards {
                    None => {
                        held_cards = Some(found);
                    }
                    Some((src_loc, src_count)) => {
                        let (target_loc, ..) = found;
                        if target_loc == src_loc {
                            held_cards = None;
                        } else {
                            if let Some(target_stack) = player_state.stack_at(target_loc) {
                                if let Some(src_stack) = player_state.stack_at(src_loc) {
                                    let stack_cards = src_stack.cards();
                                    let back_card = &stack_cards[stack_cards.len() - src_count];

                                    if target_stack.can_add(*back_card) {
                                        let action = ni_ty::HandAction::Move {
                                            from: ni_ty::StackLocation::Player(0, src_loc),
                                            count: src_count as u8,
                                            to: ni_ty::StackLocation::Player(0, target_loc),
                                        };
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        mq::clear_background(BACKGROUND_COLOR);

        for i in 0..(player_state.nerts_stack().len() - 1) {
            draw_back(10.0 + (i as f32) * 10.0, 10.0);
        }
        if let Some(card) = player_state.nerts_stack().last() {
            if !matches!(held_cards, Some((ni_ty::PlayerStackLocation::Nerts, ..))) {
                draw_card(
                    card.card,
                    10.0 + ((player_state.nerts_stack().len() - 1) as f32) * 10.0,
                    10.0,
                );
            }
        }

        for (i, stack) in player_state.tableau_stacks().iter().enumerate() {
            let card = stack.last().unwrap();
            draw_card(
                card.card,
                140.0 + CARD_WIDTH + (i as f32) * (CARD_WIDTH + 10.0),
                10.0,
            );
        }

        if let Some((stack_loc, count)) = held_cards {
            let stack = player_state.stack_at(stack_loc);
            if let Some(stack) = stack {
                let stack_cards = stack.cards();
                let cards = &stack_cards[(stack_cards.len() - count)..];

                draw_vertical_stack_cards(
                    cards,
                    mouse_pos.0 - CARD_WIDTH / 2.0,
                    mouse_pos.1 - CARD_HEIGHT / 2.0,
                );
            } else {
                held_cards = None;
            }
        }

        mq::next_frame().await
    }
}
