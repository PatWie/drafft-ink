//! Small wording helpers for the messages that name an element kind.

use std::fmt;

/// A noun with its indefinite article: `an arrow`, `a rectangle`.
///
/// The article follows the first letter (`a`, `e`, `i`, `o`, `u` take `an`). That is right for every
/// element kind name (`arrow`, `ellipse`, `image`, and the others, which start with a consonant). Use it
/// wherever a message builds "a/an" from a kind, so no message reads "a arrow".
///
/// ```
/// use drafftink_scene::words::Article;
/// assert_eq!(Article::new("arrow").to_string(), "an arrow");
/// assert_eq!(Article::new("line").to_string(), "a line");
/// assert_eq!(Article::new("ellipse").capitalized().to_string(), "An ellipse");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Article<'a> {
    noun: &'a str,
    capital: bool,
}

impl<'a> Article<'a> {
    /// The article and `noun`, in lower case.
    pub fn new(noun: &'a str) -> Self {
        Self { noun, capital: false }
    }

    /// The same with a capital first letter, for the start of a sentence.
    pub fn capitalized(self) -> Self {
        Self { capital: true, ..self }
    }
}

impl fmt::Display for Article<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let vowel =
            self.noun.chars().next().is_some_and(|c| matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u'));
        let article = match (vowel, self.capital) {
            (true, true) => "An",
            (true, false) => "an",
            (false, true) => "A",
            (false, false) => "a",
        };
        write!(f, "{article} {}", self.noun)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;

    #[test]
    fn every_kind_gets_the_right_article() {
        for (kind, text) in [
            (Kind::Arrow, "an arrow"),
            (Kind::Ellipse, "an ellipse"),
            (Kind::Image, "an image"),
            (Kind::Rectangle, "a rectangle"),
            (Kind::Diamond, "a diamond"),
            (Kind::Line, "a line"),
            (Kind::Freehand, "a freehand"),
            (Kind::Text, "a text"),
            (Kind::Math, "a math"),
            (Kind::Frame, "a frame"),
        ] {
            assert_eq!(Article::new(kind.name()).to_string(), text);
        }
        assert_eq!(Article::new("arrow").capitalized().to_string(), "An arrow");
        assert_eq!(Article::new("").to_string(), "a ");
    }
}
