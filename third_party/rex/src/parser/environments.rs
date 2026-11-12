/*!
 * Module for parsing LaTeX array environments
 * 
 */
use unicode_math::TexSymbolType;

use crate::layout;
use crate::parser::error::ParseError;
use crate::parser::List;

use super::nodes::{Array, ArrayColumnAlign, ArrayColumnsFormatting, ColSeparator, DummyNode};
use super::symbols::Symbol;
use super::{error::ParseResult, nodes::CellContent, textoken::TexToken, Parser};
use super::{GroupKind, ParseNode};

/// An enumeration of recognized environments.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Environment {
    /// The `\begin{array}{<col spec>} .. \end{array}` environment
    Array,
    /// The `\begin{matrix} .. \end{matrix}` environment, an array with centered columns.  
    /// Except for spacing, equivalent to `\begin{array}{c .. c} .. \end{array}` 
    Matrix,
    /// The `\begin{pmatrix} .. \end{pmatrix}` environment, an array with centered columns enclosed with parentheses.  
    ///  Except for spacing, equivalent to `\left(\begin{array}{c .. c} .. \end{array}\right)` 
    PMatrix,
    /// The `\begin{bmatrix} .. \end{bmatrix}` environment, an array with centered columns enclosed with square brackets.  
    ///  Roughly equivalent to `\left[\begin{array}{c .. c} .. \end{array}\right]` 
    BMatrix,
    /// The `\begin{Bmatrix} .. \end{Bmatrix}` environment, an array with centered columns enclosed with curly braces.  
    ///  Except for spacing, equivalent to `\left\{\begin{array}{c .. c} .. \end{array}\right\}` 
    BbMatrix,
    /// The `\begin{vmatrix} .. \end{vmatrix}` environment, an array with centered columns enclosed with | delimiters.  
    ///  Except for spacing, equivalent to `\left|\begin{array}{c .. c} .. \end{array}\right|` 
    VMatrix,
    /// The `\begin{Vmatrix} .. \end{Vmatrix}` environment, an array with centered columns enclosed with ‖ delimiters (i.e. `\Vert`).  
    ///  Except for spacing, equivalent to `\left\lVert\begin{array}{c .. c} .. \end{array}\right\rVert` 
    VvMatrix,
    /// The `\begin{aligned} .. \end{aligned}` environment, an array with alternating right-aligned and left-aligned column, not separated with spaces.  
    ///  Except for spacing, equivalent to `\left\lVert\begin{array}{rlrl .. rl} .. \end{array}\right\rVert` 
    Aligned,
}

impl Environment {
    /// Parse the LaTeX name of the environment. Returns `None` if the name is not an environment we support.
    pub fn from_name(name : &str) ->  Option<Self> {
        match name {
            "array"    => Some(Self::Array),
            "matrix"   => Some(Self::Matrix),
            "pmatrix"  => Some(Self::PMatrix),
            "bmatrix"  => Some(Self::BMatrix),
            "Bmatrix"  => Some(Self::BbMatrix),
            "vmatrix"  => Some(Self::VMatrix),
            "Vmatrix"  => Some(Self::VvMatrix),
            "aligned"  => Some(Self::Aligned),
            _ => None
        }
    }
}


impl<'a, I : Iterator<Item = TexToken<'a>>> Parser<'a, I> {
    /// Parses the body and column sepcification of the array environment.
    pub fn parse_environment(&mut self, env : Environment) -> ParseResult<Array> {
        let mut col_format = None;

        if let Environment::Array = env {
            let group = self.token_iter
                .capture_group()
                .map_err(|e| match e {
                    ParseError::ExpectedToken => ParseError::MissingColFormatForArrayEnvironment,
                    _ => e,
                })?;

            let mut forked_parser = Parser::from_iter(Self::EMPTY_COMMAND_COLLECTION, group.into_iter());
            col_format = Some(forked_parser.tokens_as_column_format()?);
        }
        let mut rows = self.parse_array_body(env)?;

        let left_delimiter;
        let right_delimiter;

        match env {
            Environment::Array   |
            Environment::Matrix  | 
            Environment::Aligned
            => {
                left_delimiter  = None;
                right_delimiter = None;
            },
            Environment::PMatrix  => {
                left_delimiter  = Some(Symbol {codepoint : '(', atom_type : TexSymbolType::Inner});
                right_delimiter = Some(Symbol {codepoint : ')', atom_type : TexSymbolType::Inner});
            },
            Environment::BMatrix  => {
                left_delimiter  = Some(Symbol {codepoint : '[', atom_type : TexSymbolType::Inner});
                right_delimiter = Some(Symbol {codepoint : ']', atom_type : TexSymbolType::Inner});
            },
            Environment::BbMatrix => {
                left_delimiter  = Some(Symbol {codepoint : '{', atom_type : TexSymbolType::Inner});
                right_delimiter = Some(Symbol {codepoint : '}', atom_type : TexSymbolType::Inner});
            },
            Environment::VMatrix  => {
                left_delimiter  = Some(Symbol {codepoint : '|', atom_type : TexSymbolType::Inner});
                right_delimiter = Some(Symbol {codepoint : '|', atom_type : TexSymbolType::Inner});
            },
            Environment::VvMatrix => {
                left_delimiter  = Some(Symbol {codepoint : '\u{2016}', atom_type : TexSymbolType::Inner});
                right_delimiter = Some(Symbol {codepoint : '\u{2016}', atom_type : TexSymbolType::Inner});
            },
        }

        // For the `aligned` ennvironment, we add dummies in even columns (second, fourth, etc.)
        // which copy the atom_type of the last node of the previous column
        if let Environment::Aligned = env {
            for row in rows.iter_mut() {
                for cell in row.chunks_exact_mut(2) {
                    let atom_type = cell[0].last().map_or_else(
                        || TexSymbolType::Ordinary, 
                        |node| node.atom_type(),
                    );
                    cell[1].insert(0, ParseNode::DummyNode(DummyNode { at: atom_type }));
                }
            }
        }

        let col_format = col_format.unwrap_or_else(|| {
            let n_cols = rows.last().map_or(0, |row| row.len());
            if let Environment::Aligned = env {
                ArrayColumnsFormatting {
                    alignment:  [ArrayColumnAlign::Right, ArrayColumnAlign::Left].iter().cycle().cloned().take(n_cols).collect(),
                    separators: [Vec::new(), vec![ColSeparator::AtExpression(Vec::new())]].iter().cycle().cloned().take(n_cols + 1).collect(),
                }
            }
            else {
                ArrayColumnsFormatting { 
                    alignment:  vec![ArrayColumnAlign::Centered; n_cols], 
                    separators: vec![vec![]; n_cols + 1], 
                }
            }
        });

        let extra_row_sep = match env {
            Environment::Aligned => true,
            Environment::Array | Environment::Matrix | Environment::PMatrix 
            | Environment::BMatrix | Environment::BbMatrix | Environment::VMatrix 
            | Environment::VvMatrix 
            => false,
        };

        let cell_layout_style = match env {
            Environment::Aligned => layout::Style::Display,
            Environment::Array | Environment::Matrix | Environment::PMatrix 
            | Environment::BMatrix | Environment::BbMatrix | Environment::VMatrix 
            | Environment::VvMatrix 
            => layout::Style::Text,
        };

        Ok(Array {
            col_format,
            rows,
            left_delimiter,
            right_delimiter,
            extra_row_sep,
            cell_layout_style,
        })
    }

    /// Parses the body of an array environment up till the `\end{..}`, including it.
    pub fn parse_array_body(&mut self, env : Environment) -> ParseResult<Vec<Vec<CellContent>>> {
        let mut to_return    = Vec::new();
        let mut current_line = Vec::new();

        while {
            let List {nodes, group} = self.parse_until_end_of_group()?;

            match group {
                GroupKind::Env(env_ended) if env == env_ended => {
                    if !current_line.is_empty() || !nodes.is_empty() {
                        current_line.push(nodes);
                        to_return.push(std::mem::take(&mut current_line));
                    }
                    false
                },
                GroupKind::Align => {
                    current_line.push(nodes);
                    true
                },
                GroupKind::NewLine => {
                    current_line.push(nodes);
                    to_return.push(std::mem::take(&mut current_line));
                    true
                },

                _ => return Err(ParseError::UnexpectedEndGroup { expected : vec![GroupKind::Align, GroupKind::NewLine, GroupKind::Env(env)].into_boxed_slice(), got : group }),
            }
        }
        {}


        Ok(to_return)
    }
}

impl<'a, I : Iterator<Item = TexToken<'a>>> Parser<'a, I> {
    fn tokens_as_column_format(&mut self) -> ParseResult<ArrayColumnsFormatting> {
        let mut alignment  = Vec::new();
        let mut separators = vec![Vec::new()];
        while let Some(token) = self.token_iter.next_token()? {
            match token {
                  TexToken::Char(c@'c') 
                | TexToken::Char(c@'l') 
                | TexToken::Char(c@'r') 
                => {
                    alignment.push(match c {
                        'c' => ArrayColumnAlign::Centered,
                        'l' => ArrayColumnAlign::Left,
                        'r' => ArrayColumnAlign::Right,
                        _   => unreachable!(), // This has already been ruled out in the previous match
                    });
                    separators.push(Vec::new());
                },
                TexToken::Char('|') 
                => {
                    // Safe to unwrap b/c `separators` always has at least one element
                    let current_separators = separators.last_mut().unwrap();

                    match current_separators.last_mut() {
                        Some(ColSeparator::VerticalBars(bars)) => {
                            *bars += 1;
                        },
                        _ => {
                            current_separators.push(ColSeparator::VerticalBars(1));
                        },
                    }
                },
                TexToken::Char('@') => {
                    let nodes = self.parse_control_seq_argument_as_nodes("@")?;
                    // Safe to unwrap b/c `separators` always has at least one element
                    let current_separators = separators.last_mut().unwrap();
                    current_separators.push(ColSeparator::AtExpression(nodes));
                }
                TexToken::Char(_) => {
                    return Err(ParseError::UnrecognizedArrayColumnFormat);
                }

                TexToken::WhiteSpace => (),

                TexToken::BeginGroup 
                | TexToken::EndGroup 
                | TexToken::ControlSequence(_) 
                | TexToken::Superscript 
                | TexToken::Tilde 
                | TexToken::Prime { .. } 
                | TexToken::Alignment 
                | TexToken::Argument(_) 
                | TexToken::Subscript => return Err(ParseError::UnrecognizedArrayColumnFormat),
            }
        }
        Ok(ArrayColumnsFormatting {
            alignment,
            separators,
        })
    }
}
