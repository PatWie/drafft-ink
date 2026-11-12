/// An enum representing the desired category of a symbol
/// A symbol's category determines its spacing relative to each other, e.g `1+23` ought to be typeset with some space between + and 2, but very little between 2 and 3.
/// The category also determines whether something can be `\left` or `\right` delimiter
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum TexSymbolType {
    Punctuation,
    Ordinary,
    Open,
    Close,
    Binary,
    Relation,
    Accent,
    AccentWide,
    AccentOverlay,
    BotAccent,
    BotAccentWide,
    Alpha,
    Fence,
    /// A mathematical operator like `\sum`, 
    /// the boolean parameter sets whether limits are positioned like exponents or not.
    /// For instance, in $\sum_0^1$, the 0 and 1 could be placed above and below ∑ (boolean true)
    /// or like regular exponents, e.g. ∑³, (boolean false).
    Operator(bool),
    Over,
    Under,
    Inner,
    Radical,
    Transparent,
}


pub const OPERATOR_LIMITS: &[&str] = &[
    "coprod",
    "bigvee",
    "bigwedge",
    "biguplus",
    "bigcap",
    "bigcup",
    "prod",
    "sum",
    "bigotimes",
    "bigoplus",
    "bigodot",
    "bigsqcup",
];

impl TexSymbolType {
    pub fn from_str(name: &str) -> Option<TexSymbolType> {
        match name {
            "mathalpha"                                 => Some(TexSymbolType::Alpha),
            "mathpunct"                                 => Some(TexSymbolType::Punctuation),
            "mathopen"                                  => Some(TexSymbolType::Open),
            "mathclose"                                 => Some(TexSymbolType::Close),
            "mathord"                                   => Some(TexSymbolType::Ordinary),
            "mathbin"                                   => Some(TexSymbolType::Binary),
            "mathrel"                                   => Some(TexSymbolType::Relation),
            // "mathop" if OPERATOR_LIMITS.contains(&name) => Some(TexSymbolType::Operator(true)),
            "mathop"                                    => Some(TexSymbolType::Operator(false)),
            "mathfence"                                 => Some(TexSymbolType::Fence),
            "mathover"                                  => Some(TexSymbolType::Over),
            "mathunder"                                 => Some(TexSymbolType::Under),
            "mathaccent"                                => Some(TexSymbolType::Accent),
            "mathaccentwide"                            => Some(TexSymbolType::AccentWide),
            "mathaccentoverlay"                         => Some(TexSymbolType::AccentOverlay),
            "mathbotaccent"                             => Some(TexSymbolType::BotAccent),
            "mathbotaccentwide"                         => Some(TexSymbolType::BotAccentWide),
            "mathinner"                                 => Some(TexSymbolType::Inner),
            "mathradical"                               => Some(TexSymbolType::Radical),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Symbol<'a> {
    pub codepoint: char,
    pub name: & 'a str,
    pub description: & 'a str,
    pub atom_type: TexSymbolType,
}
