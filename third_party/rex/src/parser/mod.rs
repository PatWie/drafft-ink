//! Parses strings representing LateX formulas into [`ParseNode`]'s
//!
//! The main function function of interest is [`engine::parse`]

pub mod color;
mod control_sequence;
pub mod environments;
pub mod error;
pub mod macros;
pub mod nodes;
pub mod symbols;
mod textoken;

use unicode_math::TexSymbolType;

use crate::dimensions::AnyUnit;
use crate::error::ParseResult;
use crate::font::style_symbol;
use crate::font::Style;
use crate::parser::control_sequence::parse_color;
use crate::parser::control_sequence::PrimitiveControlSequence;
use crate::parser::nodes::Accent;
use crate::parser::nodes::Delimited;
use crate::parser::nodes::GenFraction;
use crate::parser::nodes::PlainText;
use crate::parser::textoken::TexToken;

use self::control_sequence::SpaceKind;
use self::environments::Environment;
use self::error::ParseError;
use self::macros::CommandCollection;
use self::macros::ExpandedTokenIter;
pub use self::nodes::is_symbol;
pub use self::nodes::ParseNode;
use self::nodes::Scripts;
use self::symbols::Symbol;
use self::textoken::NumberOfPrimes;
use self::textoken::TokenIterator;

/// Different types of implicit TeX groupings (e.g. `{..}` or `\begin{xyz} .. \end{xyz}`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    /// A group delimited by `{..}'
    BraceGroup,
    /// A `\begin{..} .. \end{..}` group
    Env(Environment),
    /// a group ended by &
    Align,
    /// a group end by \\
    NewLine,
    /// end of file
    EndOfInput,
    /// A group ended by `\middle`
    MiddleDelimiter,
    /// A group ended by `\right`
    RightDelimiter,
}

impl std::fmt::Display for GroupKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GroupKind::BraceGroup      => write!(f, "}}"),
            GroupKind::Env(_)          => write!(f, r"\end"),
            GroupKind::Align           => write!(f, "&"),
            GroupKind::NewLine         => write!(f, r"\\"),
            GroupKind::EndOfInput      => write!(f, "end of input"),
            GroupKind::MiddleDelimiter => write!(f, r"\middle"),
            GroupKind::RightDelimiter  => write!(f, r"\right"),
        }
    }
}


struct List {
    nodes : Vec<ParseNode>,
    group : GroupKind
}


/// Contains the internal state of the TeX parser, what's left to parse, and has methods to parse various TeX construct.  
/// Holds a reference to `CommandCollection`, which holds the definition of custom TeX macros defined by the user.
/// When not using custom macros, the parser can be made `'static`.
pub struct Parser<'a, I : Iterator<Item = TexToken<'a>>> {
    token_iter : ExpandedTokenIter<'a, I>,
    current_style : Style,
}

impl<'a> Parser<'a, TokenIterator<'a>> {
    /// Creates a new parser from a macro collection and some string input.  
    /// The parser borrows the command collection and the input string for the same lifetime `'a`.
    pub fn new<'command : 'a, 'input : 'a>(command_collection: & 'command CommandCollection, input: & 'input str) -> Self { 
        Self { 
            token_iter : ExpandedTokenIter::new(command_collection, TokenIterator::new(input)),
            current_style : Style::default()
        } 
    }
}

impl<'a, I : Iterator<Item = TexToken<'a>>> Parser<'a, I> {

    const EMPTY_COMMAND_COLLECTION : & 'static CommandCollection = &CommandCollection::new();

    /// Creates a parser from a macro collection and an iterator over tokens.
    pub fn from_iter<'command : 'a>(command_collection: & 'command CommandCollection, input: I) -> Self { 
        Self { 
            token_iter : ExpandedTokenIter::new(command_collection, input),
            current_style: Style::default(),
        } 
    }


    /// Parses the input into an array of [`ParseNode`].
    pub fn parse(&mut self) -> ParseResult<Vec<ParseNode>> {
        let List { nodes, group } = self.parse_until_end_of_group()?;
        if let GroupKind::EndOfInput = group 
        { Ok(nodes) }
        else 
        { Err(ParseError::UnexpectedEndGroup {
            expected: Box::from([GroupKind::EndOfInput]),
            got: group,
        }) }
    }


    fn parse_until_end_of_group(&mut self) -> ParseResult<List> {
        let mut results = Vec::new();

        while let Some(token) = self.token_iter.next_token()? {
            match token {
                TexToken::Superscript 
                | TexToken::Subscript  
                | TexToken::Prime(..)  
                => {
                    let last_node = results.pop();
                    let mut script = match last_node {
                        Some(ParseNode::Scripts(mut scripts)) => scripts,
                        Some(node) => 
                            Scripts { 
                                base: Some(Box::new(node)), 
                                superscript: None,
                                subscript: None, 
                            }
                        ,
                        None => 
                            Scripts { 
                                base: None, 
                                superscript: None,
                                subscript: None, 
                            }
                        ,
                    };
                    
                    let is_superscript = !matches!(token, TexToken::Subscript);
                    
                    let exponent = script.get_script(is_superscript);
                    let exponent = exponent.get_or_insert(Vec::new());
                    
                    
                    if let TexToken::Prime(number_of_primes) = token {
                        let codepoint = match number_of_primes {
                            NumberOfPrimes::Simple => '′',
                            NumberOfPrimes::Double => '″',
                            NumberOfPrimes::Triple => '‴',
                        };
                        let symbol = Symbol { codepoint, atom_type: TexSymbolType::Ordinary };
                        exponent.push(ParseNode::Symbol(symbol));
                    }
                    else {
                        let group = self.parse_required_argument_as_nodes().map_err(|e| match e {
                            ParseError::ExpectedToken => ParseError::MissingSubSuperScript,
                            e => e,
                        })?;
                        exponent.extend(group);
                    }
                    
                    
                    
                    results.push(ParseNode::Scripts(script));
                },
                TexToken::Tilde => { 
                    results.push(ParseNode::Kerning(SpaceKind::WordSpace.size()))
                },
                TexToken::WhiteSpace => { },
                TexToken::BeginGroup => {
                    // Font changes made within a group should not affect what happens outside of it
                    let old_style = self.current_style;
                    let List { nodes, group } = self.parse_until_end_of_group()?;
                    self.current_style = old_style;
                    if group != GroupKind::BraceGroup {
                        return Err(ParseError::UnexpectedEndGroup{expected: Box::from([GroupKind::BraceGroup]), got: group});
                    }

                    results.push(ParseNode::Group(nodes));
                },
                TexToken::EndGroup => {
                    return Ok(List { nodes: results, group: GroupKind::BraceGroup });
                },
                TexToken::Alignment => {
                    return Ok(List { nodes: results, group: GroupKind::Align });
                },
                TexToken::Char(codepoint) => {
                    let symbol = self.char_to_symbol(codepoint)?;
                    results.push(ParseNode::Symbol(symbol));
                },
                TexToken::ControlSequence("\\") => {
                    return Ok(List { nodes: results, group: GroupKind::NewLine });
                }
                // Here we deal with "primitive" control sequences, not macros
                TexToken::ControlSequence(control_sequence_name) => {
                    let command = 
                        PrimitiveControlSequence::from_name(control_sequence_name)
                        .ok_or_else(|| ParseError::UnrecognizedControlSequence(control_sequence_name.to_string().into_boxed_str()))?
                    ;
                    use PrimitiveControlSequence::*;
                    match command {
                        Radical(character) => {
                            // Check for optional argument: \sqrt[n]{...}
                            let index = self.parse_optional_bracket_arg()?;
                            let inner = self.parse_control_seq_argument_as_nodes(control_sequence_name)?;
                            results.push(ParseNode::Radical(nodes::Radical { inner, character, index }));
                        },
                        Rule => {
                            let width_tokens = self.token_iter.capture_group().map_err(|e| match e {
                                ParseError::ExpectedToken => ParseError::MissingArgForCommand(Box::from(control_sequence_name)),
                                _ => e,
                            })?;
                            let width_string = tokens_as_string(width_tokens.into_iter())?;
                            let width = parse_dimension(&width_string)?;

                            let height_tokens = self.token_iter.capture_group().map_err(|e| match e {
                                ParseError::ExpectedToken => ParseError::MissingArgForCommand(Box::from(control_sequence_name)),
                                _ => e,
                            })?;
                            let height_string = tokens_as_string(height_tokens.into_iter())?;
                            let height = parse_dimension(&height_string)?;

                            results.push(ParseNode::Rule(nodes::Rule {
                                width, height,
                            }))
                        },
                        Color => {
                            let color_name_group = self.token_iter.capture_group().map_err(|e| match e {
                                ParseError::ExpectedToken => ParseError::MissingArgForCommand(Box::from(control_sequence_name)),
                                _ => e,
                            })?;
                            let color = parse_color(color_name_group.into_iter())?;
                            let inner = self.parse_control_seq_argument_as_nodes(control_sequence_name)?;
                            results.push(ParseNode::Color(nodes::Color {
                                color,
                                inner,
                            }));
                        },
                        ColorLit(color) => {
                            let inner = self.parse_control_seq_argument_as_nodes(control_sequence_name)?;
                            results.push(ParseNode::Color(nodes::Color {
                                color,
                                inner,
                            }));
                        },
                        StyleChange { family, weight, takes_arg } => {
                            let old_style = self.current_style;
                            if let Some(family) = family {
                                self.current_style = self.current_style.with_family(family);
                            }
                            if let Some(weight) = weight {
                                self.current_style = self.current_style.with_weight(weight);
                            }

                            if takes_arg {
                                let nodes = self.parse_required_argument_as_nodes()?;
                                self.current_style = old_style;
                                results.push(ParseNode::Group(nodes));
                            }
                        }
                        Fraction(left_delimiter, right_delimiter, bar_thickness, style) => {
                            let numerator   = self.parse_control_seq_argument_as_nodes(control_sequence_name)?;
                            let denominator = self.parse_control_seq_argument_as_nodes(control_sequence_name)?;

                            results.push(ParseNode::GenFraction(GenFraction {
                                numerator, denominator,
                                left_delimiter, right_delimiter,
                                bar_thickness, style,
                            }));
                        },
                        ExtendedDelimiter(delimiter_size, atom_type) => {
                            let mut delimiter = self.parse_next_token_as_delimiter()?;
                            match delimiter.atom_type {
                                TexSymbolType::Open | TexSymbolType::Close | TexSymbolType::Fence 
                                => (),
                                _ => return Err(ParseError::ExpectedDelimiter),
                            }
                            delimiter.atom_type = atom_type;

                            let height_enclosed_content = AnyUnit::from(delimiter_size.to_size());

                            results.push(ParseNode::ExtendedDelimiter(nodes::ExtendedDelimiter::new(
                                delimiter,
                                height_enclosed_content
                            )));
                        },
                        Kerning(space) => {
                            results.push(ParseNode::Kerning(space))
                        },
                        StyleCommand(style) => {
                            results.push(ParseNode::Style(style));
                        },
                        AtomChange(at) => {
                            let inner = self.parse_control_seq_argument_as_nodes(control_sequence_name)?;
                            results.push(ParseNode::AtomChange(nodes::AtomChange {
                                at, inner,
                            }));
                        },
                        Underline => {
                            let inner = self.parse_control_seq_argument_as_nodes(control_sequence_name)?;
                            results.push(ParseNode::FontEffect(nodes::FontEffect {
                                inner,
                            }));
                        },
                        TextOperator(op_name, limits_placement) => {
                            results.push(make_operator(op_name, limits_placement));
                        },
                        OperatorName => {
                            // Capture operator name
                            let text_group = self.token_iter.capture_group().map_err(|e| match e {
                                ParseError::ExpectedToken => ParseError::MissingArgForCommand(Box::from(control_sequence_name)),
                                _ => e,
                            })?;
                            let op_name = tokens_as_string(text_group.into_iter())?;
                            results.push(make_operator(&op_name, false));
                        }
                        SubStack(atom_type) => {
                            let group = self.token_iter.capture_group().map_err(|e| match e {
                                ParseError::ExpectedToken => ParseError::MissingArgForCommand(Box::from(control_sequence_name)),
                                _ => e,
                            })?;

                            let mut forked_parser = Parser::from_iter(Self::EMPTY_COMMAND_COLLECTION, group.into_iter());
                            forked_parser.current_style = self.current_style;

                            let mut lines = Vec::new();

                            while {
                                let List { nodes, group } = forked_parser.parse_until_end_of_group()?;

                                if !nodes.is_empty() || group != GroupKind::EndOfInput {
                                    lines.push(nodes);
                                }

                                match group {
                                    GroupKind::NewLine => true,
                                    GroupKind::EndOfInput => false,
                                    _ => return Err(ParseError::UnexpectedEndGroup {expected: Box::from([GroupKind::NewLine, GroupKind::EndOfInput]), got: group})
                                }
                            } {}

                            results.push(ParseNode::Stack(nodes::Stack {
                                atom_type,
                                lines,
                            }))

                        },
                        Limits(add_limits) => {
                            let node =
                                results
                                    .last_mut()
                                    .ok_or(ParseError::LimitControlSequenceMustBeAfterOperator)?
                            ;
                            if let TexSymbolType::Operator(_) = node.atom_type() {
                                node.set_atom_type(TexSymbolType::Operator(add_limits))
                            }
                            else {
                                return Err(ParseError::LimitControlSequenceMustBeAfterOperator);
                            }

                        }
                        Text => {
                            let text_group = self.token_iter.capture_group().map_err(|e| match e {
                                ParseError::ExpectedToken => ParseError::MissingArgForCommand(Box::from(control_sequence_name)),
                                _ => e,
                            })?;
                            let text = tokens_as_string(text_group.into_iter())?;
                            results.push(ParseNode::PlainText(PlainText {
                                text,
                            }));
                        },
                        Mbox => {
                            let text_group = self.token_iter.capture_group().map_err(|e| match e {
                                ParseError::ExpectedToken => ParseError::MissingArgForCommand(Box::from(control_sequence_name)),
                                _ => e,
                            })?;
                            let text = tokens_as_string(text_group.into_iter())?;
                            // We create a group so as to scope the style change only to the \mbox not to later nodes
                            // Maybe setting the style in this way is too crude
                            let mut nodes = Vec::with_capacity(2);
                            nodes.push(ParseNode::Style(crate::layout::Style::Text));
                            nodes.push(ParseNode::PlainText(PlainText {
                                text,
                            })); 
                            results.push(ParseNode::Group(nodes));
                        }
                        BeginEnv => {
                            let env_name_group = self.token_iter.capture_group().map_err(|e| match e {
                                ParseError::ExpectedToken => ParseError::MissingArgForCommand(Box::from(control_sequence_name)),
                                _ => e,
                            })?;
                            let env_name = tokens_as_string(env_name_group.into_iter())?;
                            let env = Environment::from_name(&env_name).ok_or_else(|| ParseError::UnrecognizedEnvironment(env_name.into_boxed_str()))?;
                            let array = self.parse_environment(env)?;
                            results.push(ParseNode::Array(array));
                        },
                        EndEnv => {
                            let env_name_group = self.token_iter.capture_group().map_err(|e| match e {
                                ParseError::ExpectedToken => ParseError::MissingArgForCommand(Box::from(control_sequence_name)),
                                _ => e,
                            })?;
                            let env_name = tokens_as_string(env_name_group.into_iter())?;
                            let env = Environment::from_name(&env_name).ok_or_else(|| ParseError::UnrecognizedEnvironment(env_name.into_boxed_str()))?;

                            return Ok(List { nodes: results, group: GroupKind::Env(env) });
                        },
                        Left => {
                            let delimiter = self.parse_next_token_as_delimiter()?;
                            if !delimiter.is_open_delimiter() {
                                return Err(ParseError::ExpectedOpenDelimiter);
                            }

                            let mut delimiters = vec![delimiter];
                            let mut inners     = Vec::new();
                            while {
                                let List { nodes, group } = self.parse_until_end_of_group()?;
                                inners.push(nodes);

                                match group {
                                    GroupKind::MiddleDelimiter => {
                                        let delimiter = self.parse_next_token_as_delimiter()?;
                                        if !delimiter.is_middle_delimiter() {
                                            return Err(ParseError::ExpectedMiddleDelimiter);
                                        }
                                        delimiters.push(delimiter);
                                        true
                                    },
                                    GroupKind::RightDelimiter  => {
                                        let delimiter = self.parse_next_token_as_delimiter()?;
                                        if !delimiter.is_close_delimiter() {
                                            return Err(ParseError::ExpectedClosingDelimiter);
                                        }
                                        delimiters.push(delimiter);
                                        false
                                    },
                                    _ => return Err(ParseError::UnexpectedEndGroup { 
                                        expected: Box::from([GroupKind::RightDelimiter, GroupKind::MiddleDelimiter]), 
                                        got: group, 
                                    })
                                }
                            }{}

                            results.push(ParseNode::Delimited(Delimited::new(
                                delimiters, 
                                inners
                            )))
                        },
                        Middle => {
                            return Ok(List { nodes: results, group: GroupKind::MiddleDelimiter });
                        },
                        Right => {
                            return Ok(List { nodes: results, group: GroupKind::RightDelimiter });
                        },
                        Unsupported => {
                            let n_args = PrimitiveControlSequence::n_args(control_sequence_name).unwrap_or(0);
                            // Parse number of args and do nothing with it
                            for _ in 0 .. n_args {
                                self.parse_required_argument_as_nodes()?;
                            }
                        }
                        SymbolCommand(mut symbol) => {
                            match symbol.atom_type {
                                  TexSymbolType::Accent 
                                | TexSymbolType::AccentWide   
                                | TexSymbolType::Over   
                                | TexSymbolType::Under  => {
                                    let nucleus = self.parse_required_argument_as_nodes()?;
                                    results.push(ParseNode::Accent(Accent {
                                        symbol,
                                        nucleus,
                                        // Only "accent" are not extended
                                        extend: symbol.atom_type != TexSymbolType::Accent,
                                        under:  symbol.atom_type == TexSymbolType::Under,
                                    }));
                                },
                                _ => {
                                    self.style_symbol_with_current_style(&mut symbol);
                                    results.push(ParseNode::Symbol(symbol));
                                },
                            }
                        },
                    }
                },
                TexToken::Argument(_) => return Err(ParseError::UnexpectedMacroArgument),
            }
        }

        Ok(List { nodes: results, group: GroupKind::EndOfInput })
    }

    fn style_symbol_with_current_style(&self, symbol: &mut Symbol) {
        let Symbol { codepoint, .. } = symbol;
        *codepoint = style_symbol(*codepoint, self.current_style);
    }

    fn char_to_symbol(&self, codepoint: char) -> Result<Symbol, ParseError> {
        let atom_type = codepoint_atom_type(codepoint).ok_or_else(|| ParseError::UnrecognizedSymbol(codepoint))?;
        let mut symbol = Symbol { codepoint, atom_type };
        self.style_symbol_with_current_style(&mut symbol);
        Ok(symbol)
    }

    fn parse_control_seq_argument_as_nodes(&mut self, control_seq_name : &str) -> ParseResult<Vec<ParseNode>> {
        self.parse_required_argument_as_nodes()
            .map_err(|e| match e {
                ParseError::ExpectedToken => ParseError::MissingArgForCommand(Box::from(control_seq_name)),
                e => e,
            })
    }

    /// Parse an optional bracket argument `[...]`, returning `Some(nodes)` or `None`.
    ///
    /// Used for `\sqrt[n]{...}` where `[n]` is the root index.
    fn parse_optional_bracket_arg(&mut self) -> ParseResult<Option<Vec<ParseNode>>> {
        // Peek at the next token to see if it's a '['
        let peeked = self.token_iter.peek_token()?;
        match peeked {
            Some(TexToken::Char('[')) => {
                // Consume the '[' (it was put back by peek, consume it now)
                let _ = self.token_iter.next_token()?;

                // Collect tokens until we find the matching ']'
                let mut tokens = Vec::new();
                let mut depth = 1u32;
                loop {
                    let token = self.token_iter.next_token()?
                        .ok_or(ParseError::UnmatchedBrackets)?;
                    match &token {
                        TexToken::Char('[') => depth += 1,
                        TexToken::Char(']') => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    tokens.push(token);
                }

                // Parse the collected tokens as nodes
                let mut forked_parser = Parser::from_iter(Self::EMPTY_COMMAND_COLLECTION, tokens.into_iter());
                forked_parser.current_style = self.current_style;
                let list = forked_parser.parse_until_end_of_group()?;
                Ok(Some(list.nodes))
            }
            _ => Ok(None),
        }
    }

    fn parse_next_token_as_delimiter(&mut self) -> ParseResult<Symbol> {
        let token = self.token_iter.next_token()?.ok_or_else(|| ParseError::ExpectedSymbolAfterDelimiterCommand)?;
        match token {
            TexToken::Char(c) => {
                self.char_to_symbol(c)
            },
            TexToken::ControlSequence(control_sequence_name) => {
                let command = 
                    PrimitiveControlSequence::from_name(control_sequence_name)
                    .ok_or_else(|| ParseError::UnrecognizedControlSequence(control_sequence_name.to_string().into_boxed_str()))?
                ;
                match command {
                    PrimitiveControlSequence::SymbolCommand(mut symbol) => {
                        self.style_symbol_with_current_style(&mut symbol); 
                        Ok(symbol)
                    },
                    _ => Err(ParseError::ExpectedSymbolAfterDelimiterCommand),
                }
            },
              TexToken::Superscript | TexToken::Subscript  | TexToken::Alignment 
            | TexToken::WhiteSpace  | TexToken::BeginGroup | TexToken::EndGroup 
            | TexToken::Argument(_) | TexToken::Tilde      | TexToken::Prime { .. }
            => Err(ParseError::ExpectedSymbolAfterDelimiterCommand),
        }
    }

    fn parse_required_argument_as_nodes(&mut self) -> ParseResult<Vec<ParseNode>> {
        let group = self.token_iter.capture_group()?;

        // Normally all tokens are already expanded after `capture_group`
        // There is no need to have further expansions
        let mut forked_parser = Parser::from_iter(Self::EMPTY_COMMAND_COLLECTION, group.into_iter());
        forked_parser.current_style = self.current_style;

        let List { nodes, group } = forked_parser.parse_until_end_of_group()?;

        if group != GroupKind::EndOfInput {
            return Err(ParseError::UnexpectedEndGroup { expected : Box::from([GroupKind::EndOfInput]), got : group });
        }
        
        Ok(nodes)
    }
}

fn make_operator(op_name: &str, limits_placement: bool) -> ParseNode {
    ParseNode::AtomChange(nodes::AtomChange {
        at: TexSymbolType::Operator(limits_placement),
        inner: 
            op_name
            .chars()
            .map(|c| ParseNode::Symbol(Symbol {
                codepoint: c,
                atom_type: TexSymbolType::Ordinary,
            }))
            .collect()
        ,
    })
}

/// Parses the input as a dimension, e.g. `1cm` or `-2pt or `3.5em`
fn parse_dimension(input_string: &str) -> ParseResult<AnyUnit> {
    fn is_float_char(character : &char) -> bool {
        character.is_ascii_digit()
        || *character == '-'
        || *character == '+'
        || *character == ' '
        || *character == '.'
    }

    let float_input_to_parse : String = input_string.chars().take_while(is_float_char).collect();
    let number = float_input_to_parse.replace(' ', "").parse::<f64>().map_err(|_| ParseError::UnrecognizedDimension(Box::from(input_string)))?;

    let dim_string = &input_string[float_input_to_parse.len() ..];

    // expecting 2 ASCII characters representing the dimension
    let dim = dim_string.get(.. 2).ok_or_else(|| ParseError::UnrecognizedDimension(Box::from(input_string)))?;

    match dim {
        "em" => Ok(AnyUnit::Em(number)),
        "px" => Ok(AnyUnit::Px(number)),
        _ => Err(ParseError::UnrecognizedDimension(Box::from(input_string))),
    }
}

fn tokens_as_string<'a, I : Iterator<Item = TexToken<'a>>>(iterator : I) -> ParseResult<String> {
    let mut to_return = String::new();
    for token in iterator {
        match token {
            TexToken::Char(c)     => to_return.push(c),
            TexToken::WhiteSpace  => to_return.push(' '),
            TexToken::Prime(number_of_primes)  => {
                to_return.push_str(match number_of_primes {
                    NumberOfPrimes::Simple => "'",
                    NumberOfPrimes::Double => "''",
                    NumberOfPrimes::Triple => "'''",
                });
            },
            TexToken::ControlSequence("{") => to_return.push('{'), 
            TexToken::ControlSequence("}") => to_return.push('}'),
            TexToken::BeginGroup | TexToken::EndGroup => (), 
            
            TexToken::ControlSequence(_) 
            | TexToken::Superscript 
            | TexToken::Subscript 
            | TexToken::Tilde 
            | TexToken::Alignment
            | TexToken::Argument(_)
            => return Err(ParseError::ExpectedChars),
        }
    }
    Ok(to_return)
}


/// Parses an input into a sequence of [`ParseNode`].
/// This function is the API entry point for parsing tex.
pub fn parse(input: &str) -> ParseResult<Vec<ParseNode>> {
    parse_with_custom_commands(input, &CommandCollection::default())
}


/// Like [`parse`], but with a specified macro collection.
pub fn parse_with_custom_commands<'a>(input: & 'a str, custom_commands : &CommandCollection) -> ParseResult<Vec<ParseNode>> {
    Parser::new(custom_commands, input).parse()
}







/// Helper function for determining an atomtype based on a given codepoint.
/// This is primarily used for characters while processing, so may give false
/// negatives when used for other things.
fn codepoint_atom_type(codepoint: char) -> Option<TexSymbolType> {
    Some(match codepoint {
             'a' ..= 'z' | 'A' ..= 'Z' | '0' ..= '9' | 'Α' ..= 'Ω' | 'α' ..= 'ω' => TexSymbolType::Alpha,
             '*' | '+' | '-' => TexSymbolType::Binary,
             '[' | '(' => TexSymbolType::Open,
             ']' | ')' | '?' | '!' => TexSymbolType::Close,
             '=' | '<' | '>' | ':' => TexSymbolType::Relation,
             ',' | ';' => TexSymbolType::Punctuation,
             '|' => TexSymbolType::Fence,
             '/' | '@' | '.' | '"' => TexSymbolType::Alpha,
             _ => return None,
         })
}
