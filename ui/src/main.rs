use futures_util::{SinkExt, StreamExt, TryStreamExt};
use macroquad::prelude as mq;
use macroquad::ui as mqui;
use nertsio_types as ni_ty;
use std::sync::Arc;

const BACKGROUND_COLOR: mq::Color = mq::Color::new(0.2, 0.7, 0.2, 1.0);

const CARD_WIDTH: f32 = 90.0;
const CARD_HEIGHT: f32 = 135.0;
const VERTICAL_STACK_SPACING: f32 = 20.0;

enum State {
    Connecting,
    GameNeutral,
    GameHand {
        hand_state: ni_ty::HandState,
        held_cards: Option<(ni_ty::PlayerStackLocation, usize)>,
    },
}

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

async fn handle_connection(
    info_mutex: &std::sync::Mutex<Option<(ni_ty::GameState, u8)>>,
    mut game_msg_recv: tokio::sync::mpsc::UnboundedReceiver<ni_ty::protocol::GameMessageC2S>,
) -> Result<(), anyhow::Error> {
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

    let handshake_stream = conn.connection.open_bi().await?;

    println!("opened stream");

    let mut handshake_stream_send =
        async_bincode::AsyncBincodeWriter::<_, ni_ty::protocol::HandshakeMessageC2S, _>::from(
            handshake_stream.0,
        )
        .for_async();
    let handshake_stream_recv = async_bincode::AsyncBincodeReader::<
        _,
        ni_ty::protocol::HandshakeMessageS2C,
    >::from(handshake_stream.1);

    let hello_msg = ni_ty::protocol::HandshakeMessageC2S::Hello {
        name: "Nerter".to_owned(),
        game_id: 42,
    };
    handshake_stream_send.send(hello_msg).await?;

    println!("sent hello");

    let (first_message, handshake_stream_recv) = handshake_stream_recv.into_future().await;
    let first_message = first_message.ok_or(anyhow::anyhow!("Failed to complete handshake"))??;

    let _ = (handshake_stream_recv, handshake_stream_send);

    #[allow(irrefutable_let_patterns)]
    if let ni_ty::protocol::HandshakeMessageS2C::Hello = first_message {
    } else {
        anyhow::bail!("Unknown handshake response");
    }

    println!("aaa");

    let (game_stream_res, _bi_streams) = conn.bi_streams.into_future().await;
    let game_stream = game_stream_res.ok_or(anyhow::anyhow!("Missing game stream"))??;

    println!("bbb");

    let mut game_stream_send =
        async_bincode::AsyncBincodeWriter::<_, ni_ty::protocol::GameMessageC2S, _>::from(
            game_stream.0,
        )
        .for_async();
    let game_stream_recv =
        async_bincode::AsyncBincodeReader::<_, ni_ty::protocol::GameMessageS2C>::from(
            game_stream.1,
        );

    println!("wat");

    futures_util::future::try_join(
        async {
            while let Some(msg) = game_msg_recv.recv().await {
                println!("sending {:?}", msg);
                game_stream_send.send(msg).await?;
            }
            Result::<_, anyhow::Error>::Ok(())
        },
        async {
            game_stream_recv
                .map_err(Into::into)
                .try_for_each(|msg| async {
                    use ni_ty::protocol::GameMessageS2C;

                    println!("received {:?}", msg);

                    match msg {
                        GameMessageS2C::Joined {
                            info,
                            your_player_id,
                        } => {
                            *info_mutex.lock().unwrap() = Some((info, your_player_id));
                        }
                        GameMessageS2C::PlayerJoin { id, info } => {
                            (*info_mutex.lock().unwrap())
                                .as_mut()
                                .unwrap()
                                .0
                                .players
                                .insert(id, info);
                        }
                        GameMessageS2C::PlayerLeave { id } => {
                            (*info_mutex.lock().unwrap())
                                .as_mut()
                                .unwrap()
                                .0
                                .players
                                .remove(&id);
                        }
                        GameMessageS2C::PlayerUpdateReady { id, value } => {
                            (*info_mutex.lock().unwrap())
                                .as_mut()
                                .unwrap()
                                .0
                                .players
                                .get_mut(&id)
                                .unwrap()
                                .ready = value;
                        }
                    }

                    Ok(())
                })
                .await
        },
    )
    .await?;

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

    let game_info_mutex = Arc::new(std::sync::Mutex::new(None));
    let (game_msg_send, game_msg_recv) = tokio::sync::mpsc::unbounded_channel();

    async_rt.spawn({
        let game_info_mutex = game_info_mutex.clone();
        async move {
            if let Err(err) = handle_connection(&game_info_mutex, game_msg_recv).await {
                eprintln!("Failed to handle connection: {:?}", err);
            }
        }
    });

    let mut state = State::Connecting;

    loop {
        let mouse_pos = mq::mouse_position();

        state = match state {
            State::Connecting => {
                mq::clear_background(BACKGROUND_COLOR);

                if (*game_info_mutex.lock().unwrap()).is_some() {
                    State::GameNeutral
                } else {
                    State::Connecting
                }
            }
            State::GameNeutral => {
                mq::clear_background(BACKGROUND_COLOR);

                let mut lock = game_info_mutex.lock().unwrap();
                let (game, my_player_id) = (*lock).as_mut().unwrap();

                for (i, (key, player)) in game.players.iter_mut().enumerate() {
                    let y = 10.0 + (i as f32) * 25.0;

                    if key == my_player_id {
                        if mqui::root_ui().button(
                            mq::Vec2::new(10.0, y),
                            if player.ready { "Unready" } else { "Ready" },
                        ) {
                            let new_value = !player.ready;
                            player.ready = new_value;

                            game_msg_send
                                .send(ni_ty::protocol::GameMessageC2S::UpdateSelfReady {
                                    value: new_value,
                                })
                                .unwrap();
                        }
                    } else {
                        mqui::root_ui().label(
                            mq::Vec2::new(10.0, y),
                            if player.ready { "Ready" } else { "Not Ready" },
                        );
                    }
                }

                State::GameNeutral
            }
            State::GameHand {
                mut hand_state,
                mut held_cards,
            } => {
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
                                        CARD_HEIGHT
                                            + ((stack.len() as f32) - 1.0) * VERTICAL_STACK_SPACING,
                                    )
                                    .contains(mouse_vec)
                                    {
                                        let loc = ni_ty::PlayerStackLocation::Tableau(i as u8);
                                        if stack.len() > 0 {
                                            let found_idx = (((mouse_pos.1 - 10.0)
                                                / VERTICAL_STACK_SPACING)
                                                as usize)
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
                                        if matches!(
                                            target_loc,
                                            ni_ty::PlayerStackLocation::Tableau(_)
                                        ) {
                                            if let Some(target_stack) =
                                                player_state.stack_at(target_loc)
                                            {
                                                if let Some(src_stack) =
                                                    player_state.stack_at(src_loc)
                                                {
                                                    let stack_cards = src_stack.cards();
                                                    let back_card =
                                                        &stack_cards[stack_cards.len() - src_count];

                                                    if target_stack.can_add(*back_card) {
                                                        let action = ni_ty::HandAction::Move {
                                                            from: ni_ty::StackLocation::Player(
                                                                0, src_loc,
                                                            ),
                                                            count: src_count as u8,
                                                            to: ni_ty::StackLocation::Player(
                                                                0, target_loc,
                                                            ),
                                                        };

                                                        let _ = player_state;
                                                        if let Err(err) =
                                                            hand_state.apply(0, action)
                                                        {
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
                        if let Some((ni_ty::PlayerStackLocation::Tableau(stack_idx), count)) =
                            held_cards
                        {
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

                State::GameHand {
                    held_cards,
                    hand_state,
                }
            }
        };

        mq::next_frame().await
    }
}
