use macroquad::prelude as mq;
use nertsio_types as ni_ty;
use std::sync::Arc;

const BACKGROUND_COLOR: mq::Color = mq::Color::new(0.2, 0.7, 0.2, 1.0);

const CARD_WIDTH: f32 = 90.0;
const CARD_HEIGHT: f32 = 135.0;
const VERTICAL_STACK_SPACING: f32 = 20.0;

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

struct InsecureVerifier;
impl rustls::client::ServerCertVerifier for InsecureVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::client::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}

async fn handle_connection() -> Result<(), anyhow::Error> {
    let mut endpoint = quinn::Endpoint::client(([0, 0, 0, 0], 0).into())?;
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new({
        let mut cfg = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(rustls::RootCertStore { roots: vec![] })
            .with_no_client_auth();
        cfg.dangerous()
            .set_certificate_verifier(Arc::new(InsecureVerifier));
        cfg
    })));

    let conn = endpoint
        .connect(([127, 0, 0, 1], 6465).into(), "nio.invalid")?
        .await?;

    println!("connected");

    Ok(())
}

#[macroquad::main("nertsio")]
async fn main() {
    let async_rt = tokio::runtime::Runtime::new().unwrap();

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
            draw_card(card.card, x, y + (i as f32) * VERTICAL_STACK_SPACING);
        }
    };

    let mut hand_state = ni_ty::HandState::generate(4);

    let mut held_cards: Option<(ni_ty::PlayerStackLocation, usize)> = None;

    async_rt.spawn(async {
        if let Err(err) = handle_connection().await {
            eprintln!("Failed to handle connection: {:?}", err);
        }
    });

    loop {
        let mouse_pos = mq::mouse_position();

        {
            let player_state = &hand_state.players()[0];

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
                    player_state
                        .tableau_stacks()
                        .iter()
                        .enumerate()
                        .filter_map(|(i, stack)| {
                            if mq::Rect::new(
                                140.0 + CARD_WIDTH + (i as f32) * (CARD_WIDTH + 10.0),
                                10.0,
                                CARD_WIDTH,
                                CARD_HEIGHT + ((stack.len() as f32) - 1.0) * VERTICAL_STACK_SPACING,
                            )
                            .contains(mouse_vec)
                            {
                                let loc = ni_ty::PlayerStackLocation::Tableau(i as u8);
                                if stack.len() > 0 {
                                    let found_idx =
                                        (((mouse_pos.1 - 10.0) / VERTICAL_STACK_SPACING) as usize)
                                            .min(stack.len() - 1);

                                    Some((loc, stack.len() - found_idx))
                                } else {
                                    Some((loc, 0))
                                }
                            } else {
                                None
                            }
                        })
                        .next()
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
                                if matches!(target_loc, ni_ty::PlayerStackLocation::Tableau(_)) {
                                    if let Some(target_stack) = player_state.stack_at(target_loc) {
                                        if let Some(src_stack) = player_state.stack_at(src_loc) {
                                            let stack_cards = src_stack.cards();
                                            let back_card =
                                                &stack_cards[stack_cards.len() - src_count];

                                            if target_stack.can_add(*back_card) {
                                                let action = ni_ty::HandAction::Move {
                                                    from: ni_ty::StackLocation::Player(0, src_loc),
                                                    count: src_count as u8,
                                                    to: ni_ty::StackLocation::Player(0, target_loc),
                                                };

                                                let _ = player_state;
                                                if let Err(err) = hand_state.apply(0, action) {
                                                    eprintln!(
                                                        "failed to apply movement: {:?}",
                                                        err
                                                    );
                                                }
                                                held_cards = None;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let player_state = &hand_state.players()[0];

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
            let cards = stack.cards();
            let cards =
                if let Some((ni_ty::PlayerStackLocation::Tableau(stack_idx), count)) = held_cards {
                    if i == (stack_idx as usize) {
                        &cards[..(cards.len() - count)]
                    } else {
                        cards
                    }
                } else {
                    cards
                };

            draw_vertical_stack_cards(
                cards,
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
