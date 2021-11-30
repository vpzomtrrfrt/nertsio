use strum::IntoEnumIterator;

lazy_static::lazy_static! {
    static ref FULL_DECK: Vec<Card> = {
        itertools::iproduct!(Suit::iter(), Rank::iter()).map(|(suit, rank)| Card { suit, rank }).collect()
    };
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
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

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, strum_macros::EnumIter)]
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

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Rank(u8);

impl Rank {
    pub const ACE: Rank = Rank(1);

    pub fn iter() -> impl Iterator<Item = Rank> + Clone {
        (1..13).map(Rank)
    }

    pub fn try_new(src: u8) -> Option<Self> {
        if src >= 1 && src <= 13 {
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

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Card {
    pub suit: Suit,
    pub rank: Rank,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CardInstance {
    pub card: Card,
    pub owner_id: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Ordering {
    Any,
    AlternatingDown,
    SingleSuitUp,
}

impl Ordering {
    pub fn allows(self, below: Card, above: Card) -> bool {
        match self {
            Ordering::Any => true,
            Ordering::AlternatingDown => below.rank.decrement() == Some(above.rank) && below.suit.color() == above.suit.color().opposite(),
            Ordering::SingleSuitUp => below.rank.increment() == Some(above.rank) && below.suit == above.suit,
        }
    }
}

#[derive(Debug)]
pub struct RejectedByOrdering;

#[derive(Clone, Debug)]
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

    pub fn try_add(&mut self, card: CardInstance) -> Result<(), RejectedByOrdering> {
        let ok = match self.cards.last() {
            None => if self.start_with_ace {
                card.card.rank == Rank::ACE
            } else {
                true
            },
            Some(below) => self.ordering.allows(below.card, card.card),
        };

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

    pub fn take_all(&mut self) -> Vec<CardInstance> {
        std::mem::replace(&mut self.cards, Vec::new())
    }

    pub fn cards(&self) -> &[CardInstance] {
        &self.cards
    }
}

#[derive(Clone, Debug)]
pub struct HandPlayerState {
    nerts_stack: Stack,
    stock_stack: Stack,
    waste_stack: Stack,
    tableau_stacks: Vec<Stack>,
}

impl HandPlayerState {
    pub fn generate(owner_id: u8, tableau_stacks_count: usize) -> Self {
        let mut cards: Vec<_> = FULL_DECK.iter().map(|card| CardInstance { owner_id, card: *card }).collect();
        rand::seq::SliceRandom::shuffle(&mut cards[..], &mut rand::thread_rng());

        let nerts_stack = Stack::from_list_unordered(cards.split_off(13));
        let tableau_stacks = (0..tableau_stacks_count).map(|_| {
            Stack::from_one(Ordering::AlternatingDown, false, cards.pop().unwrap())
        }).collect();

        let stock_stack = Stack::from_list_unordered(cards);
        let waste_stack = Stack::new(Ordering::Any, false);

        Self {
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
                break
            }
        }
    }

    pub fn return_stock(&mut self) {
        for card in self.waste_stack.take_all().into_iter().rev() {
            self.stock_stack.try_add(card).unwrap() // stock stack has no constraints
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
}

pub enum PlayerStackLocation {
    Nerts,
    Tableau(u8),
    Stock,
    Waste,
}

pub enum StackLocation {
    Lake(u16),
    Player(u8, PlayerStackLocation),
}

pub enum HandAction {
    FlipStock,
    ReturnStock,
    Move {
        from: StackLocation,
        base: Card,
        to: StackLocation,
    },
}

pub struct CannotApplyAction;

#[derive(Clone, Debug)]
pub struct HandState {
    players: Vec<HandPlayerState>,
    lake_stacks: Vec<Stack>,
}

impl HandState {
    pub fn generate(player_count: u8) -> Self {
        let players = (0..player_count).map(|_| {
            HandPlayerState::generate(player_count, 4)
        }).collect();
        let lake_stacks = (0..(player_count * 4)).map(|_| {
            Stack::new(Ordering::SingleSuitUp, true)
        }).collect();

        Self {
            players,
            lake_stacks,
        }
    }

    pub fn apply(&mut self, player: u8, action: HandAction) -> Result<(), CannotApplyAction> {
        match action {
            HandAction::FlipStock => {
                self.players[player as usize].flip_stock();
                Ok(())
            },
            HandAction::ReturnStock => {
                self.players[player as usize].return_stock();
                Ok(())
            },
            HandAction::Move { from, base, to } => unimplemented!(),
        }
    }

    pub fn players(&self) -> &[HandPlayerState] {
        &self.players
    }
    pub fn lake_stacks(&self) -> &[Stack] {
        &self.lake_stacks
    }
}
