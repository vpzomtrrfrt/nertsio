use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use strum::IntoEnumIterator;

pub mod protocol;

lazy_static::lazy_static! {
    static ref FULL_DECK: Vec<Card> = {
        itertools::iproduct!(Suit::iter(), Rank::iter()).map(|(suit, rank)| Card { suit, rank }).collect()
    };
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Color {
    Red,
    Black,
}

impl Color {
    pub fn opposite(self) -> Color {
        match self {
            Color::Red => Color::Black,
            Color::Black => Color::Red,
        }
    }
}

#[derive(
    Clone, Copy, PartialEq, Eq, Hash, Debug, strum_macros::EnumIter, Serialize, Deserialize,
)]
pub enum Suit {
    Spades,
    Diamonds,
    Clubs,
    Hearts,
}

impl Suit {
    pub fn color(self) -> Color {
        match self {
            Suit::Spades | Suit::Clubs => Color::Black,
            Suit::Diamonds | Suit::Hearts => Color::Red,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct Rank(u8);

impl Rank {
    pub const ACE: Rank = Rank(1);

    pub fn iter() -> impl Iterator<Item = Rank> + Clone {
        (1..13).map(Rank)
    }

    pub fn try_new(src: u8) -> Option<Self> {
        if (1..=13).contains(&src) {
            Some(Rank(src))
        } else {
            None
        }
    }

    pub fn new(src: u8) -> Self {
        Rank::try_new(src).expect("Invalid rank")
    }

    pub fn value(self) -> u8 {
        self.0
    }

    pub fn increment(self) -> Option<Rank> {
        Rank::try_new(self.value() + 1)
    }

    pub fn decrement(self) -> Option<Rank> {
        Rank::try_new(self.value() - 1)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct Card {
    pub suit: Suit,
    pub rank: Rank,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct CardInstance {
    pub card: Card,
    pub owner_id: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Ordering {
    Any,
    AlternatingDown,
    SingleSuitUp,
}

impl Ordering {
    pub fn allows(self, below: Card, above: Card) -> bool {
        match self {
            Ordering::Any => true,
            Ordering::AlternatingDown => {
                below.rank.decrement() == Some(above.rank)
                    && below.suit.color() == above.suit.color().opposite()
            }
            Ordering::SingleSuitUp => {
                below.rank.increment() == Some(above.rank) && below.suit == above.suit
            }
        }
    }
}

#[derive(Debug)]
pub struct RejectedByOrdering;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stack {
    ordering: Ordering,
    start_with_ace: bool,
    cards: Vec<CardInstance>,
}

impl Stack {
    pub fn new(ordering: Ordering, start_with_ace: bool) -> Self {
        Self {
            ordering,
            start_with_ace,
            cards: Vec::new(),
        }
    }

    pub fn from_one(ordering: Ordering, start_with_ace: bool, card: CardInstance) -> Self {
        if start_with_ace && card.card.rank != Rank::ACE {
            panic!("Cannot create start_with_ace stack with non-ace start");
        }

        Self {
            ordering,
            start_with_ace,
            cards: vec![card],
        }
    }

    pub fn from_list_unordered(list: Vec<CardInstance>) -> Self {
        Self {
            ordering: Ordering::Any,
            start_with_ace: false,
            cards: list,
        }
    }

    pub fn last(&self) -> Option<&CardInstance> {
        self.cards.last()
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    pub fn can_add(&self, card: CardInstance) -> bool {
        match self.cards.last() {
            None => {
                if self.start_with_ace {
                    card.card.rank == Rank::ACE
                } else {
                    true
                }
            }
            Some(below) => self.ordering.allows(below.card, card.card),
        }
    }

    pub fn try_add(&mut self, card: CardInstance) -> Result<(), RejectedByOrdering> {
        let ok = self.can_add(card);

        if ok {
            self.cards.push(card);
            Ok(())
        } else {
            Err(RejectedByOrdering)
        }
    }

    pub fn pop(&mut self) -> Option<CardInstance> {
        self.cards.pop()
    }

    pub fn pop_many(&mut self, count: usize) -> Option<Vec<CardInstance>> {
        match self.len().cmp(&count) {
            std::cmp::Ordering::Less => None,
            std::cmp::Ordering::Equal => Some(self.take_all()),
            std::cmp::Ordering::Greater => Some(self.cards.split_off(self.len() - count)),
        }
    }

    pub fn take_all(&mut self) -> Vec<CardInstance> {
        std::mem::take(&mut self.cards)
    }

    pub fn cards(&self) -> &[CardInstance] {
        &self.cards
    }

    pub fn shuffle(&mut self, rng: &mut impl rand::Rng) {
        use rand::seq::SliceRandom;
        self.cards.shuffle(rng);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlayerStackLocation {
    Nerts,
    Tableau(u8),
    Stock,
    Waste,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandPlayerState {
    player_id: u8,

    nerts_stack: Stack,
    stock_stack: Stack,
    waste_stack: Stack,
    tableau_stacks: Vec<Stack>,
}

impl HandPlayerState {
    pub fn generate(player_id: u8, hand_player_id: u8, tableau_stacks_count: usize) -> Self {
        let mut cards: Vec<_> = FULL_DECK
            .iter()
            .map(|card| CardInstance {
                owner_id: hand_player_id,
                card: *card,
            })
            .collect();
        rand::seq::SliceRandom::shuffle(&mut cards[..], &mut rand::thread_rng());

        let nerts_stack = Stack::from_list_unordered(cards.split_off(cards.len() - 13));
        let tableau_stacks = (0..tableau_stacks_count)
            .map(|_| Stack::from_one(Ordering::AlternatingDown, false, cards.pop().unwrap()))
            .collect();

        let stock_stack = Stack::from_list_unordered(cards);
        let waste_stack = Stack::new(Ordering::Any, false);

        Self {
            player_id,

            nerts_stack,
            tableau_stacks,
            stock_stack,
            waste_stack,
        }
    }

    pub fn flip_stock(&mut self) {
        for _ in 0..3 {
            if let Some(card) = self.stock_stack.pop() {
                self.waste_stack.try_add(card).unwrap(); // waste stack has no constraints
            } else {
                break;
            }
        }
    }

    pub fn return_stock(&mut self) {
        for card in self.waste_stack.take_all().into_iter().rev() {
            self.stock_stack.try_add(card).unwrap() // stock stack has no constraints
        }
    }

    pub fn stack_at(&self, loc: PlayerStackLocation) -> Option<&Stack> {
        match loc {
            PlayerStackLocation::Nerts => Some(self.nerts_stack()),
            PlayerStackLocation::Stock => Some(self.stock_stack()),
            PlayerStackLocation::Waste => Some(self.waste_stack()),
            PlayerStackLocation::Tableau(idx) => self.tableau_stacks().get(idx as usize),
        }
    }

    pub fn mut_stack_at(&mut self, loc: PlayerStackLocation) -> Option<&mut Stack> {
        match loc {
            PlayerStackLocation::Nerts => Some(&mut self.nerts_stack),
            PlayerStackLocation::Stock => Some(&mut self.stock_stack),
            PlayerStackLocation::Waste => Some(&mut self.waste_stack),
            PlayerStackLocation::Tableau(idx) => self.tableau_stacks.get_mut(idx as usize),
        }
    }

    pub fn nerts_stack(&self) -> &Stack {
        &self.nerts_stack
    }
    pub fn stock_stack(&self) -> &Stack {
        &self.stock_stack
    }
    pub fn waste_stack(&self) -> &Stack {
        &self.waste_stack
    }
    pub fn tableau_stacks(&self) -> &[Stack] {
        &self.tableau_stacks
    }
    pub fn player_id(&self) -> u8 {
        self.player_id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StackLocation {
    Lake(u16),
    Player(u8, PlayerStackLocation),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandAction {
    ShuffleStock {
        seed: u64,
    },
    FlipStock,
    ReturnStock,
    Move {
        from: StackLocation,
        count: u8,
        to: StackLocation,
    },
}

impl HandAction {
    pub fn should_reset_stall(&self) -> bool {
        match self {
            HandAction::FlipStock | HandAction::ReturnStock => false,
            HandAction::ShuffleStock { .. } => true,
            HandAction::Move { from, count: _, to } => {
                matches!(from, StackLocation::Player(_, PlayerStackLocation::Nerts))
                    || matches!(to, StackLocation::Lake(_))
            }
        }
    }
}

#[derive(Debug)]
pub struct CannotApplyAction;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandState {
    players: Vec<HandPlayerState>,
    lake_stacks: Vec<Stack>,
    pub started: bool,
    pub nerts_called: bool,
}

impl HandState {
    pub fn generate(players: impl Iterator<Item = u8>) -> Self {
        let players: Vec<_> = players.collect();

        let tableau_stacks_count = match players.len() {
            1 | 2 => 6,
            3 => 5,
            _ => 4,
        };

        let players: Vec<_> = players
            .into_iter()
            .enumerate()
            .map(|(idx, player_id)| {
                HandPlayerState::generate(player_id, idx as u8, tableau_stacks_count)
            })
            .collect();
        let lake_stacks = (0..(players.len() * 4))
            .map(|_| Stack::new(Ordering::SingleSuitUp, true))
            .collect();

        Self {
            players,
            lake_stacks,
            started: false,
            nerts_called: false,
        }
    }

    pub fn apply(
        &mut self,
        player: Option<u8>,
        action: HandAction,
    ) -> Result<(), CannotApplyAction> {
        match action {
            HandAction::ShuffleStock { seed } => {
                if player.is_some() {
                    Err(CannotApplyAction) // players cannot trigger shuffle
                } else {
                    use rand::SeedableRng;

                    let mut shuffle_rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(seed);
                    for current in &mut self.players {
                        current.return_stock();
                        current.stock_stack.shuffle(&mut shuffle_rng);
                    }

                    Ok(())
                }
            }
            HandAction::FlipStock => {
                if let Some(player) = player {
                    self.players[player as usize].flip_stock();
                    Ok(())
                } else {
                    Err(CannotApplyAction)
                }
            }
            HandAction::ReturnStock => {
                if let Some(player) = player {
                    self.players[player as usize].return_stock();
                    Ok(())
                } else {
                    Err(CannotApplyAction)
                }
            }
            HandAction::Move { from, count, to } => {
                if let Some(player) = player {
                    if let StackLocation::Player(src_player, src_loc) = from {
                        if src_player != player {
                            return Err(CannotApplyAction);
                        }

                        (match src_loc {
                            PlayerStackLocation::Nerts | PlayerStackLocation::Waste => {
                                if count == 1 {
                                    Ok(())
                                } else {
                                    Err(CannotApplyAction)
                                }
                            }
                            PlayerStackLocation::Stock => Err(CannotApplyAction),
                            PlayerStackLocation::Tableau(_) => Ok(()),
                        })?;

                        (match to {
                            StackLocation::Player(dest_player, dest_loc) => {
                                if dest_player == player {
                                    match dest_loc {
                                        PlayerStackLocation::Nerts
                                        | PlayerStackLocation::Waste
                                        | PlayerStackLocation::Stock => Err(CannotApplyAction),
                                        PlayerStackLocation::Tableau(_) => Ok(()),
                                    }
                                } else {
                                    Err(CannotApplyAction)
                                }
                            }
                            StackLocation::Lake(_) => Ok(()),
                        })?;

                        {
                            let src_stack = self.players[player as usize]
                                .stack_at(src_loc)
                                .ok_or(CannotApplyAction)?;

                            println!("src_stack = {:?}", src_stack);

                            let first_card = &src_stack.cards()[src_stack.len() - count as usize];

                            let dest_stack = self.stack_at(to).ok_or(CannotApplyAction)?;

                            if !dest_stack.can_add(*first_card) {
                                return Err(CannotApplyAction);
                            }
                        }

                        if let Some(cards) = self.players[player as usize]
                            .mut_stack_at(src_loc)
                            .unwrap()
                            .pop_many(count as usize)
                        {
                            let dest_stack = self.mut_stack_at(to).unwrap();
                            for card in cards {
                                dest_stack.try_add(card).unwrap();
                            }

                            Ok(())
                        } else {
                            Err(CannotApplyAction)
                        }
                    } else {
                        Err(CannotApplyAction)
                    }
                } else {
                    Err(CannotApplyAction)
                }
            }
        }
    }

    pub fn stack_at(&self, loc: StackLocation) -> Option<&Stack> {
        match loc {
            StackLocation::Lake(idx) => self.lake_stacks().get(idx as usize),
            StackLocation::Player(player, loc) => self
                .players()
                .get(player as usize)
                .and_then(|player| player.stack_at(loc)),
        }
    }

    pub fn players(&self) -> &[HandPlayerState] {
        &self.players
    }
    pub fn lake_stacks(&self) -> &[Stack] {
        &self.lake_stacks
    }

    fn mut_stack_at(&mut self, loc: StackLocation) -> Option<&mut Stack> {
        match loc {
            StackLocation::Lake(idx) => self.lake_stacks.get_mut(idx as usize),
            StackLocation::Player(player, loc) => self
                .players
                .get_mut(player as usize)
                .and_then(|player| player.mut_stack_at(loc)),
        }
    }

    pub fn calculate_hand_scores(&self, settings: &GameSettings) -> Vec<i32> {
        let mut scores: Vec<_> = self
            .players()
            .iter()
            .map(|player| {
                -1 * (player.nerts_stack().len() as i32) * (settings.nerts_card_penalty as i32)
            })
            .collect();

        for stack in self.lake_stacks() {
            for card in stack.cards() {
                scores[card.owner_id as usize] += 1;
            }
        }

        scores
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GamePlayerState {
    pub name: String,
    pub ready: bool,
    pub score: i32,
    pub spectating: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameSettings {
    pub nerts_card_penalty: u8,
    pub max_players: u8,
    pub bot_difficulty: f32,
    pub public: bool,
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            nerts_card_penalty: 2,
            max_players: 6,
            bot_difficulty: 0.1,
            public: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameState {
    pub id: u32,
    pub players: BTreeMap<u8, GamePlayerState>,
    pub hand: Option<HandState>,
    pub master_player: u8,
    pub settings: GameSettings,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct HeldInfo {
    pub src: PlayerStackLocation,
    pub count: u8,
    pub offset: (f32, f32),
    pub top_card: Card,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MouseState {
    pub position: (f32, f32),
    pub held: Option<HeldInfo>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MenuMouseState {
    pub position: (f32, f32),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RegionInfo<'a> {
    pub id: Cow<'a, str>,
    pub name: Cow<'a, str>,
}
