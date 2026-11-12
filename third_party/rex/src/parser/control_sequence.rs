use unicode_math::TexSymbolType;

use crate::{dimensions::{units::Em, AnyUnit, Unit}, font::{Family, Weight}, layout::{constants, Style as LayoutStyle}, parser::{nodes::{BarThickness, MathStyle}, symbols::Symbol}, RGBA};

use super::{error::{ParseError, ParseResult}, textoken::TexToken};

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum PrimitiveControlSequence {
    /// Represents LaTeX `\sqrt{..}` or `\cuberoot{..}` any item with type `\mathradical`.  
    /// The enclosed argument gives the Unicode corresponding to this character.
    Radical(char),
    Rule,
    /// Represents ReX's command `\color{..}{..}`
    Color,
    /// Represents ReX's command `\blue{..}`, `\red{..}`
    ColorLit(RGBA),
    /// Represents LaTeX `\frac{..}`
    Fraction(Option<Symbol>, Option<Symbol>, BarThickness, MathStyle),
    /// Represents `\limits` and `\nolimits` control sequences (cf [here](https://texfaq.org/FAQ-limits))
    Limits(bool),
    ExtendedDelimiter(DelimiterSize, TexSymbolType),
    Kerning(AnyUnit),
    StyleCommand(LayoutStyle),
    AtomChange(TexSymbolType),
    TextOperator(&'static str, bool),
    SubStack(TexSymbolType),
    SymbolCommand(Symbol),
    StyleChange { family: Option<Family>, weight: Option<Weight>, takes_arg : bool },
    /// Represents `\operatorname{..}` ; this puts the text in braces as mathrm and appropriately handles space around it.
    OperatorName,
    /// Underline `\underline{..}` ; creates a horizontal bar below box
    Underline,
    BeginEnv,
    EndEnv,
    Left,
    Middle,
    Right,
    Text,
    /// The `\mbox` control sequence
    /// This command is like `\text`, except it ignores font size changes in sub-/super-scripts, etc.
    Mbox,
    /// An unsupported TeX control sequence.  
    ///
    /// Certain commands like `\label{..}` or `\nonumber` make sense in the context of a larger document but not in ReX, which renders simple formula. 
    /// Our policy is to accept these commands but compile them to nothing
    Unsupported,
}


/// Delimiter size (as offered by `\big`, `\bigg`, etc.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelimiterSize {
    /// The smallest "big" size, a size of 0.85em
    Big,
    /// 1.5 times the size of [`DelimiterSize::Big`]
    BBig,
    /// 2 times the size of [`DelimiterSize::Big`]
    Bigg,
    /// 2.5 times the size of [`DelimiterSize::Big`]
    BBigg,
}

impl DelimiterSize {
    pub fn to_size(self) -> Unit<Em> {
        constants::BIG_HEIGHT.scale(match self {
            DelimiterSize::Big   => 1.0,
            DelimiterSize::BBig  => 1.5,
            DelimiterSize::Bigg  => 2.0,
            DelimiterSize::BBigg => 2.5,
        })
    }
}


impl PrimitiveControlSequence {
    pub fn from_name(name: &str) -> Option<Self> {
        Option::or_else(
            
            Self::parse_command_name(name),
            || Symbol::from_name(name).map(|s| 
                // special case: if the object is a radical, we give it tis special type
                // this way the parser can recognize that this symbol needs something to enclose
                if let TexSymbolType::Radical = s.atom_type {
                    PrimitiveControlSequence::Radical(s.codepoint)
                }
                else {
                    PrimitiveControlSequence::SymbolCommand(s)
                }
            ),
        )
    }

    fn parse_command_name(name: &str) -> Option<Self> {
        // TODO: use a lookup table
        const OPEN_PAREN  : Option<Symbol> = Some(Symbol { codepoint : '(', atom_type : TexSymbolType::Open  });
        const CLOSE_PAREN : Option<Symbol> = Some(Symbol { codepoint : ')', atom_type : TexSymbolType::Close });
        Some(match name {
            "frac"   => Self::Fraction(None, None,              BarThickness::Default, MathStyle::NoChange),
            "tfrac"  => Self::Fraction(None, None,              BarThickness::Default, MathStyle::Text),
            "dfrac"  => Self::Fraction(None, None,              BarThickness::Default, MathStyle::Display),
            "binom"  => Self::Fraction(OPEN_PAREN, CLOSE_PAREN, BarThickness::None,    MathStyle::NoChange),
            "tbinom" => Self::Fraction(OPEN_PAREN, CLOSE_PAREN, BarThickness::None,    MathStyle::Text),
            "dbinom" => Self::Fraction(OPEN_PAREN, CLOSE_PAREN, BarThickness::None,    MathStyle::Display),

            // Stacking commands
            "substack" => Self::SubStack(TexSymbolType::Inner),


            // Style-change command
            "mathbf"   => Self::StyleChange {family: None,                     weight: Some(Weight::Bold),   takes_arg: true, },
            "mathit"   => Self::StyleChange {family: None,                     weight: Some(Weight::Italic), takes_arg: true, },
            "mathrm"   => Self::StyleChange {family: Some(Family::Roman),      weight: None,                 takes_arg: true, },
            "mathscr"  => Self::StyleChange {family: Some(Family::Script),     weight: None,                 takes_arg: true, },
            "mathfrak" => Self::StyleChange {family: Some(Family::Fraktur),    weight: None,                 takes_arg: true, },
            "mathbb"   => Self::StyleChange {family: Some(Family::Blackboard), weight: None,                 takes_arg: true, },
            "mathsf"   => Self::StyleChange {family: Some(Family::SansSerif),  weight: None,                 takes_arg: true, },
            "mathtt"   => Self::StyleChange {family: Some(Family::Monospace),  weight: None,                 takes_arg: true, },
            "mathcal"  => Self::StyleChange {family: Some(Family::Script),     weight: None,                 takes_arg: true, },

            "bf"   => Self::StyleChange {family: None,                     weight: Some(Weight::Bold),   takes_arg: false, },
            "it"   => Self::StyleChange {family: None,                     weight: Some(Weight::Italic), takes_arg: false, },
            "rm"   => Self::StyleChange {family: Some(Family::Roman),      weight: None,                 takes_arg: false, },
            "sf"   => Self::StyleChange {family: Some(Family::SansSerif),  weight: None,                 takes_arg: false, },
            "tt"   => Self::StyleChange {family: Some(Family::Monospace),  weight: None,                 takes_arg: false, },
            "cal"  => Self::StyleChange {family: Some(Family::Script),     weight: None,                 takes_arg: false, },


            // Delimiter size commands
            "bigl"  => Self::ExtendedDelimiter(DelimiterSize::Big,   TexSymbolType::Open),
            "Bigl"  => Self::ExtendedDelimiter(DelimiterSize::BBig,  TexSymbolType::Open),
            "biggl" => Self::ExtendedDelimiter(DelimiterSize::Bigg,  TexSymbolType::Open),
            "Biggl" => Self::ExtendedDelimiter(DelimiterSize::BBigg, TexSymbolType::Open),
            "bigr"  => Self::ExtendedDelimiter(DelimiterSize::Big,   TexSymbolType::Close),
            "Bigr"  => Self::ExtendedDelimiter(DelimiterSize::BBig,  TexSymbolType::Close),
            "biggr" => Self::ExtendedDelimiter(DelimiterSize::Bigg,  TexSymbolType::Close),
            "Biggr" => Self::ExtendedDelimiter(DelimiterSize::BBigg, TexSymbolType::Close),
            "bigm"  => Self::ExtendedDelimiter(DelimiterSize::Big,   TexSymbolType::Relation),
            "Bigm"  => Self::ExtendedDelimiter(DelimiterSize::BBig,  TexSymbolType::Relation),
            "biggm" => Self::ExtendedDelimiter(DelimiterSize::Bigg,  TexSymbolType::Relation),
            "Biggm" => Self::ExtendedDelimiter(DelimiterSize::BBigg, TexSymbolType::Relation),
            "big"   => Self::ExtendedDelimiter(DelimiterSize::Big,   TexSymbolType::Ordinary),
            "Big"   => Self::ExtendedDelimiter(DelimiterSize::BBig,  TexSymbolType::Ordinary),
            "bigg"  => Self::ExtendedDelimiter(DelimiterSize::Bigg,  TexSymbolType::Ordinary),
            "Bigg"  => Self::ExtendedDelimiter(DelimiterSize::BBigg, TexSymbolType::Ordinary),

            // Spacing related commands
            "!"     => Self::Kerning(SpaceKind::NegativeThinMuSkip.size()),
            ","     => Self::Kerning(SpaceKind::ThinSpace.size()),
            ":"     => Self::Kerning(SpaceKind::MedSpace.size()),
            ";"     => Self::Kerning(SpaceKind::ThickSpace.size()),
            " "     => Self::Kerning(SpaceKind::WordSpace.size()),
            "quad"  => Self::Kerning(SpaceKind::QuadSpace.size()),
            "qquad" => Self::Kerning(SpaceKind::DoubleQuadSpace.size()),

            "rule"       => Self::Rule,
            "underline"  => Self::Underline,


            // Display style changes
            "textstyle"         => Self::StyleCommand(LayoutStyle::Text),
            "displaystyle"      => Self::StyleCommand(LayoutStyle::Display),
            "scriptstyle"       => Self::StyleCommand(LayoutStyle::Script),
            "scriptscriptstyle" => Self::StyleCommand(LayoutStyle::ScriptScript),

            "text"              => Self::Text,
            "mbox"              => Self::Mbox,

            // Atom-type changes
            "mathop"  => Self::AtomChange(TexSymbolType::Operator(false)),
            "mathrel" => Self::AtomChange(TexSymbolType::Relation),
            "mathord" => Self::AtomChange(TexSymbolType::Alpha),

            // Color related
            "color"   => Self::Color,
            "blue"    => Self::ColorLit(RGBA(0,0,0xff,0xff)),
            "red"     => Self::ColorLit(RGBA(0xff,0,0,0xff)),
            "gray"    => Self::ColorLit(RGBA(0x80,0x80,0x80,0xff)),
            "phantom" => Self::ColorLit(RGBA(0,0,0,0)),

            // Operators with limits
            "det"     => Self::TextOperator("det", true),
            "gcd"     => Self::TextOperator("gcd", true),
            "lim"     => Self::TextOperator("lim", true),
            "limsup"  => Self::TextOperator("lim,sup", true),
            "liminf"  => Self::TextOperator("lim,inf", true),
            "sup"     => Self::TextOperator("sup", true),
            "supp"    => Self::TextOperator("supp", true),
            "inf"     => Self::TextOperator("inf", true),
            "max"     => Self::TextOperator("max", true),
            "min"     => Self::TextOperator("min", true),
            "Pr"      => Self::TextOperator("Pr", true),

            // Operators without limits
            "sin"     => Self::TextOperator("sin", false),
            "cos"     => Self::TextOperator("cos", false),
            "tan"     => Self::TextOperator("tan", false),
            "cot"     => Self::TextOperator("cot", false),
            "csc"     => Self::TextOperator("csc", false),
            "sec"     => Self::TextOperator("sec", false),
            "arcsin"  => Self::TextOperator("arcsin", false),
            "arccos"  => Self::TextOperator("arccos", false),
            "arctan"  => Self::TextOperator("arctan", false),
            "sinh"    => Self::TextOperator("sinh", false),
            "cosh"    => Self::TextOperator("cosh", false),
            "tanh"    => Self::TextOperator("tanh", false),
            "arg"     => Self::TextOperator("arg", false),
            "deg"     => Self::TextOperator("deg", false),
            "dim"     => Self::TextOperator("dim", false),
            "exp"     => Self::TextOperator("exp", false),
            "hom"     => Self::TextOperator("hom", false),
            "Hom"     => Self::TextOperator("Hom", false),
            "ker"     => Self::TextOperator("ker", false),
            "Ker"     => Self::TextOperator("Ker", false),
            "ln"      => Self::TextOperator("ln", false),
            "log"     => Self::TextOperator("log", false),

            // Custom operators
            "operatorname" => Self::OperatorName,

            // Environment
            "begin" => Self::BeginEnv,
            "end"   => Self::EndEnv,

            // Environment
            "left"    => Self::Left,
            "middle"  => Self::Middle,
            "right"   => Self::Right,

            // Limits
            "limits"   => Self::Limits(true),
            "nolimits" => Self::Limits(false),

            // unsupported
              "label"  
            | "nonumber" => Self::Unsupported,

            _ => return None
        })
    }

    pub fn n_args(command_name : &str) -> Option<usize> {
        Some(match command_name {
            "frac"   => 2,
            "tfrac"  => 2,
            "dfrac"  => 2,
            "binom"  => 2,
            "tbinom" => 2,
            "dbinom" => 2,

            // Stacking commands
            "substack" => 1,

            // Radical commands
            "sqrt" => 1,

            // Style-change command
            "mathbf"   => 1,
            "mathit"   => 1,
            "mathrm"   => 1,
            "mathscr"  => 1,
            "mathfrak" => 1,
            "mathbb"   => 1,
            "mathsf"   => 1,
            "mathtt"   => 1,
            "mathcal"  => 1,



            // Spacing related commands
            "!"     => 0,
            ","     => 0,
            ":"     => 0,
            ";"     => 0,
            " "     => 0,
            "quad"  => 0,
            "qquad" => 0,
            "rule"  => 2,



            // Atom-type changes
            "mathop"  => 1,
            "mathrel" => 1,
            "mathord" => 1,

            // Color related
            "color"   => 2,
            "blue"    => 1,
            "red"     => 1,
            "gray"    => 1,
            "phantom" => 1,

            // Operators with limits
            "det"     => 0,
            "gcd"     => 0,
            "lim"     => 0,
            "limsup"  => 0,
            "liminf"  => 0,
            "sup"     => 0,
            "supp"    => 0,
            "inf"     => 0,
            "max"     => 0,
            "min"     => 0,
            "Pr"      => 0,

            // Operators without limits
            "sin"     => 0,
            "cos"     => 0,
            "tan"     => 0,
            "cot"     => 0,
            "csc"     => 0,
            "sec"     => 0,
            "arcsin"  => 0,
            "arccos"  => 0,
            "arctan"  => 0,
            "sinh"    => 0,
            "cosh"    => 0,
            "tanh"    => 0,
            "arg"     => 0,
            "deg"     => 0,
            "dim"     => 0,
            "exp"     => 0,
            "hom"     => 0,
            "Hom"     => 0,
            "ker"     => 0,
            "Ker"     => 0,
            "ln"      => 0,
            "log"     => 0,

            // Custom operators
            "operatorname" => 1,

            // Text
            "underline" => 1,
            "text" => 1,
            "mbox" => 1,

            // unsupported
            "label"    => 1,
            "nonumber" => 0,

            _ => return None
        })
    }
}


#[derive(Debug, Clone, Copy)]
pub enum SpaceKind {
   /// Space produced by "\!"     
   NegativeThinMuSkip,

   /// Space produced by "\,"     
   ThinSpace,

   /// Space produced by "\:"     
   MedSpace,

   /// Space produced by "\;"     
   ThickSpace,

   /// Space produced by "\ "     
   WordSpace,

   /// Space produced by "\quad"  
   QuadSpace,

   /// Space produced by "\qquad" 
   DoubleQuadSpace,
}

impl SpaceKind {
    pub fn size(self) -> AnyUnit {
        match self {
            SpaceKind::NegativeThinMuSkip => AnyUnit::Em(-3f64/18f64),
            SpaceKind::ThinSpace          => AnyUnit::Em(3f64/18f64),
            SpaceKind::MedSpace           => AnyUnit::Em(4f64/18f64),
            SpaceKind::ThickSpace         => AnyUnit::Em(5f64/18f64),
            SpaceKind::WordSpace          => AnyUnit::Em(1f64/4f64),
            SpaceKind::QuadSpace          => AnyUnit::Em(1.0f64),
            SpaceKind::DoubleQuadSpace    => AnyUnit::Em(2.0f64),
        }
    }
}



pub fn parse_color<'a, I : Iterator<Item = TexToken<'a>>>(token_iter : I) -> ParseResult<RGBA> {
    let mut color_name = String::with_capacity("#11223344".len()); // #rrggbbaa, preparing for the worst case
    for token in token_iter {
        match token {
            TexToken::Char(c) => color_name.push(c),
            _ => return Err(ParseError::ExpectedChars),
            // NOTE: there used to be the code below here, which implies that there might be a way for color argument to contain primitive commands; 
            // not sure whether this is ever possible so I remove it
            //
            // TexToken::ControlSequence(_) => todo!(),
            // _ => todo!()
        }
    }
    let color : RGBA = color_name.parse().map_err(|_| ParseError::UnrecognizedColor(color_name.into_boxed_str()))?;
    Ok(color)
}

