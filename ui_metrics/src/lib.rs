use nertsio_types as ni_ty;

pub const CARD_WIDTH: f32 = 90.0;
pub const CARD_HEIGHT: f32 = 135.0;
pub const LAKE_SPACING: f32 = 10.0;
pub const NERTS_STACK_SPACING: f32 = 10.0;
pub const HORIZONTAL_STACK_SPACING: f32 = 15.0;
pub const VERTICAL_STACK_SPACING: f32 = 25.0;
pub const PLAYER_SPACING: f32 = 20.0;
pub const PLAYER_Y: f32 = 200.0;

pub const NOTICE_FONT_SIZE: u16 = 25;
pub const NOTICE_HEIGHT: f32 = 30.0;

#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn center(self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn contains(self, point: (f32, f32)) -> bool {
        point.0 > self.x
            && point.0 < self.x + self.width
            && point.1 > self.y
            && point.1 < self.y + self.height
    }
}

#[derive(Clone, Copy)]
pub struct PlayerLocation {
    pub x: f32,
    pub inverted: bool,
}

impl PlayerLocation {
    pub fn pos(&self) -> (f32, f32) {
        (self.x, PLAYER_Y)
    }
}

pub struct HandMetrics {
    players: usize,
    tableau_stacks: usize,
    lake_stacks: usize,
    nerts_pile_size: usize,
}

impl HandMetrics {
    pub fn new(
        players: usize,
        tableau_stacks: usize,
        lake_stacks: usize,
        nerts_pile_size: usize,
    ) -> HandMetrics {
        Self {
            players,
            tableau_stacks,
            lake_stacks,
            nerts_pile_size,
        }
    }

    pub fn player_hand_width(&self) -> f32 {
        ((if self.nerts_pile_size > 0 {
            NERTS_STACK_SPACING * (self.nerts_pile_size - 1) as f32 + CARD_WIDTH
        } else {
            0.0
        }) + if self.tableau_stacks == 0 { 0.0 } else { 20.0 }
            + (self.tableau_stacks as f32) * (CARD_WIDTH + 10.0))
            .max(CARD_WIDTH + 10.0 + VERTICAL_STACK_SPACING * 2.0 + CARD_WIDTH)
    }

    pub fn lake_width(&self) -> f32 {
        if self.lake_stacks == 0 {
            0.0
        } else {
            ((self.lake_stacks as f32) * CARD_WIDTH)
                + ((self.lake_stacks - 1) as f32) * LAKE_SPACING
        }
    }

    pub fn min_side_player_count(&self) -> usize {
        self.players / 2
    }

    pub fn max_side_player_count(&self) -> usize {
        if self.players % 2 == 0 {
            self.min_side_player_count()
        } else {
            self.min_side_player_count() + 1
        }
    }

    pub fn max_side_width(&self) -> f32 {
        self.player_hand_width() * (self.max_side_player_count() as f32)
            + PLAYER_SPACING * ((self.max_side_player_count() - 1) as f32)
    }

    pub fn needed_screen_width(&self) -> f32 {
        self.lake_width().max(self.max_side_width())
    }

    pub fn needed_screen_height(&self) -> f32 {
        (PLAYER_Y + CARD_HEIGHT + 10.0 + CARD_HEIGHT + NOTICE_HEIGHT) * 2.0
    }

    pub fn lake_start_x(&self) -> f32 {
        -self.lake_width() / 2.0
    }

    pub fn player_loc(&self, player_idx: usize) -> PlayerLocation {
        let inverted = player_idx >= self.min_side_player_count();
        let side_player_count = if inverted {
            self.max_side_player_count()
        } else {
            self.min_side_player_count()
        };
        let player_side_idx = if inverted {
            player_idx - self.min_side_player_count()
        } else {
            player_idx
        };

        let side_width = (self.player_hand_width() * (side_player_count as f32))
            + PLAYER_SPACING * (side_player_count - 1) as f32;

        let x = -(side_width / 2.0)
            + (self.player_hand_width() + PLAYER_SPACING) * (player_side_idx as f32);

        PlayerLocation { x, inverted }
    }

    pub fn stack_pos(&self, loc: ni_ty::StackLocation) -> (f32, f32) {
        match loc {
            ni_ty::StackLocation::Lake(idx) => (
                self.lake_start_x() + (idx as f32) * (CARD_WIDTH + LAKE_SPACING),
                -CARD_HEIGHT / 2.0,
            ),
            ni_ty::StackLocation::Player(player, loc) => {
                self.player_stack_pos(loc, self.player_loc(player.into()))
            }
        }
    }

    pub fn player_stack_pos(
        &self,
        loc: ni_ty::PlayerStackLocation,
        player_loc: PlayerLocation,
    ) -> (f32, f32) {
        let position = player_loc.pos();

        match loc {
            ni_ty::PlayerStackLocation::Nerts => position,
            ni_ty::PlayerStackLocation::Tableau(idx) => (
                position.0
                    + NERTS_STACK_SPACING * (self.nerts_pile_size - 1) as f32
                    + CARD_WIDTH
                    + 20.0
                    + (idx as f32) * (CARD_WIDTH + 10.0),
                position.1,
            ),
            ni_ty::PlayerStackLocation::Stock => (position.0, position.1 + CARD_HEIGHT + 10.0),
            ni_ty::PlayerStackLocation::Waste => (
                position.0 + CARD_WIDTH + 10.0,
                position.1 + CARD_HEIGHT + 10.0,
            ),
        }
    }

    /// Calculate the clickable area of a stack segment. Used by bots & practice feeder, may give
    /// incorrect results for impossible plans.
    pub fn get_dest_for_stack(
        &self,
        hand: &ni_ty::HandState,
        loc: ni_ty::StackLocation,
        take_count: usize,
        inverted: bool,
    ) -> Rect {
        let get_dest_for_card_pos = |pos: (f32, f32)| Rect {
            x: pos.0,
            y: pos.1,
            width: CARD_WIDTH,
            height: CARD_HEIGHT,
        };

        let stack = hand.stack_at(loc).unwrap();
        let remaining_count = stack.len() - take_count;

        let stack_pos = self.stack_pos(loc);

        match loc {
            ni_ty::StackLocation::Lake(_) => {
                let stack_pos = if let ni_ty::StackLocation::Lake(_) = loc {
                    if inverted {
                        (-stack_pos.0 - CARD_WIDTH, stack_pos.1)
                    } else {
                        stack_pos
                    }
                } else {
                    stack_pos
                };

                get_dest_for_card_pos(stack_pos)
            }
            ni_ty::StackLocation::Player(_, loc) => match loc {
                ni_ty::PlayerStackLocation::Nerts => get_dest_for_card_pos((
                    stack_pos.0 + (remaining_count as f32) * NERTS_STACK_SPACING,
                    stack_pos.1,
                )),
                ni_ty::PlayerStackLocation::Tableau(_) => {
                    if take_count < 2 {
                        get_dest_for_card_pos((
                            stack_pos.0,
                            stack_pos.1 + (remaining_count as f32) * VERTICAL_STACK_SPACING,
                        ))
                    } else {
                        Rect {
                            x: stack_pos.0,
                            y: stack_pos.1 + (remaining_count as f32) * VERTICAL_STACK_SPACING,
                            width: CARD_WIDTH,
                            height: VERTICAL_STACK_SPACING,
                        }
                    }
                }
                ni_ty::PlayerStackLocation::Stock => get_dest_for_card_pos(stack_pos),
                ni_ty::PlayerStackLocation::Waste => {
                    let remaining_visible = stack.len().min(3) - take_count;
                    get_dest_for_card_pos((
                        stack_pos.0 + (remaining_visible as f32) * HORIZONTAL_STACK_SPACING,
                        stack_pos.1,
                    ))
                }
            },
        }
    }
}
