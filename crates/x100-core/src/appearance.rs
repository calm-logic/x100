//! How an agent looks: the outline of its face and the colour it is drawn in. Both are closed
//! sets, so a look the app cannot draw cannot be recorded.
//!
//! The CEO starts as a circle with no colour. An agent another creates gets its creator's colour
//! and an outline chosen at random from the others, preferring one its siblings do not have. The
//! choice is made once, by the daemon, and recorded in the event that adds the agent, so folding
//! the log never rolls anything. The builder may change any agent's look later.

use serde::{Deserialize, Serialize};

/// The outline of an agent's face. Each still reads as itself at row size with two eyes inside.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Outline {
    #[default]
    Circle,
    Square,
    Hexagon,
    Diamond,
    Octagon,
    Triangle,
    Shield,
    Capsule,
}

impl Outline {
    pub const ALL: [Outline; 8] = [
        Outline::Circle,
        Outline::Square,
        Outline::Hexagon,
        Outline::Diamond,
        Outline::Octagon,
        Outline::Triangle,
        Outline::Shield,
        Outline::Capsule,
    ];
}

/// A colour an agent's face may be drawn in. Without one it is drawn in the text colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hue {
    Red,
    Orange,
    Yellow,
    Green,
    Teal,
    Blue,
    Violet,
    Pink,
}

impl Hue {
    pub const ALL: [Hue; 8] = [
        Hue::Red,
        Hue::Orange,
        Hue::Yellow,
        Hue::Green,
        Hue::Teal,
        Hue::Blue,
        Hue::Violet,
        Hue::Pink,
    ];
}

/// An agent's look. The default is the CEO's: a circle with no colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Appearance {
    pub outline: Outline,
    pub hue: Option<Hue>,
}

impl Appearance {
    /// The look of an agent `creator` makes, whose other agents have `siblings`' outlines: the
    /// creator's colour, and an outline other than the creator's, one no sibling has while any is
    /// left. `roll` is any random number; the same roll gives the same look.
    pub fn child(creator: Appearance, siblings: &[Outline], roll: u64) -> Appearance {
        let others: Vec<Outline> = Outline::ALL
            .into_iter()
            .filter(|o| *o != creator.outline)
            .collect();
        let unused: Vec<Outline> = others
            .iter()
            .copied()
            .filter(|o| !siblings.contains(o))
            .collect();
        let pool = if unused.is_empty() { others } else { unused };
        let at = (roll % pool.len() as u64) as usize;
        Appearance {
            outline: pool[at],
            hue: creator.hue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_keeps_its_creators_colour_and_never_its_outline() {
        let creator = Appearance {
            outline: Outline::Hexagon,
            hue: Some(Hue::Teal),
        };
        for roll in 0..64 {
            let child = Appearance::child(creator, &[], roll);
            assert_eq!(child.hue, Some(Hue::Teal));
            assert_ne!(child.outline, Outline::Hexagon);
        }
    }

    #[test]
    fn siblings_get_outlines_none_of_them_has_while_any_is_left() {
        let ceo = Appearance::default();
        let mut taken = Vec::new();
        // Seven outlines other than the CEO's circle, so seven children differ from each other.
        for roll in [3, 3, 3, 3, 3, 3, 3] {
            let child = Appearance::child(ceo, &taken, roll);
            assert!(!taken.contains(&child.outline), "{taken:?} {child:?}");
            assert_eq!(child.hue, None);
            taken.push(child.outline);
        }
        // An eighth must repeat one, and still not take the creator's.
        let eighth = Appearance::child(ceo, &taken, 3);
        assert_ne!(eighth.outline, Outline::Circle);
    }

    #[test]
    fn the_same_roll_gives_the_same_look() {
        let ceo = Appearance::default();
        let siblings = [Outline::Square, Outline::Diamond];
        for roll in 0..32 {
            assert_eq!(
                Appearance::child(ceo, &siblings, roll),
                Appearance::child(ceo, &siblings, roll)
            );
        }
    }

    #[test]
    fn an_unknown_outline_or_colour_is_refused_at_the_boundary() {
        assert!(serde_json::from_str::<Outline>("\"star\"").is_err());
        assert!(serde_json::from_str::<Hue>("\"mauve\"").is_err());
        let look: Appearance =
            serde_json::from_str(r#"{"outline":"shield","hue":"violet"}"#).unwrap();
        assert_eq!(
            look,
            Appearance {
                outline: Outline::Shield,
                hue: Some(Hue::Violet)
            }
        );
    }
}
