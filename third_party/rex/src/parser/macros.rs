//! Structure for custom macros (as created by e.g. `\newcommand{..}`)

use crate::parser::error::ParseError;
use crate::parser::textoken::TokenIterator;
use crate::parser::tokens_as_string;

use super::{control_sequence::PrimitiveControlSequence, error::ParseResult, textoken::TexToken};




/// A collection of custom commands. You can find a macro with the given name using [`CommandCollection::query`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CommandCollection(Vec<CustomCommand>);



impl CommandCollection {

    /// Creates a new empty [`CommandCollection`]    
    pub const fn new() -> Self {
        Self(Vec::new())
    }


    /// Retrieves a method by name
    pub fn get<'s>(& 's self, name : &str) -> Option<& 's CustomCommand> {
        self.0
            .iter()
            .find(|command| command.name() == name)
    }

    /// Parses the content of a TeX file composed of a series of `\newcommand{..}{..}` statements into a command collection
    pub fn parse(input : &str) -> ParseResult<Self> {
        let command_collection = CommandCollection::default();
        let token_iter = TokenIterator::new(input);
        let mut expanded_token_iter = ExpandedTokenIter::new(&command_collection, token_iter);

        Self::parse_from_iter(&mut expanded_token_iter)
    }

    fn parse_from_iter<'a, I>(token_iter : &mut ExpandedTokenIter<'a, I>) -> ParseResult<Self> 
    where I: Iterator<Item = TexToken<'a>> 
    {
        let mut definitions = Vec::new();
        while token_iter.peek_token()? != None {
            let definition = CustomCommand::parse_macro_definition_from_iter(token_iter)?;
            definitions.push(definition);

            // Consume whitespace
            let mut token = token_iter.peek_token()?;
            while let Some(TexToken::WhiteSpace) = token {
                token_iter.next_token()?;
                token = token_iter.peek_token()?;
            }
        }
        Ok(Self(definitions))
    }

}


#[derive(Debug, Clone, PartialEq, Eq)]
enum CommandToken {
    NormalToken(TexToken<'static>),
    OwnedCommand(String),
    ArgSlot(usize),
}


#[derive(Debug, Clone)]
enum TokenConversionError {
    IllegalParameterNumber
}

impl<'a> TryFrom<TexToken<'a>> for CommandToken {
    type Error = TokenConversionError;

    fn try_from(value: TexToken<'a>) -> Result<Self, Self::Error> {
        Ok(match value {
            TexToken::Char(c)               => Self::NormalToken(TexToken::Char(c)),
            TexToken::ControlSequence(name) => Self::OwnedCommand(name.to_string()),
            TexToken::Superscript           => Self::NormalToken(TexToken::Superscript),
            TexToken::Subscript             => Self::NormalToken(TexToken::Subscript),
            TexToken::Alignment             => Self::NormalToken(TexToken::Alignment),
            TexToken::Tilde                 => Self::NormalToken(TexToken::Tilde),
            TexToken::WhiteSpace            => Self::NormalToken(TexToken::WhiteSpace),
            TexToken::Argument(i)           => 
                match i.checked_sub(1) {
                    Some(i) => Self::ArgSlot(i),
                    None    => return Err(TokenConversionError::IllegalParameterNumber),
                }
            ,
            TexToken::BeginGroup            => Self::NormalToken(TexToken::BeginGroup),
            TexToken::EndGroup              => Self::NormalToken(TexToken::EndGroup),
            TexToken::Prime(p)              => Self::NormalToken(TexToken::Prime(p)),
        })
    }
}

impl From<TokenConversionError> for ParseError {
    fn from(value: TokenConversionError) -> Self {
        match value {
            TokenConversionError::IllegalParameterNumber => Self::IllegalParameterNumber,
        }
    }
}

/// A custom LateX command, as defined by e.g. \newcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomCommand {
    n_args : usize,
    name : String,

    // !! This should all be private
    expansion : Vec<CommandToken>,
}

struct ExpansionIterator<'args, 'token> {
    token_remaining : & 'token [CommandToken],
    args : & 'args [Vec<TexToken<'token>>],
    arg_currently_output : Option<& 'args [TexToken<'token>]>,
}

impl<'arg, 'a> Iterator for ExpansionIterator<'arg, 'a> {
    type Item = TexToken<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let Self { token_remaining, args, arg_currently_output } = self;
        let (first_token, rest) = token_remaining.split_first()?;
        match first_token {
            CommandToken::NormalToken(token) => {*token_remaining = rest; Some(token.clone())},
            CommandToken::OwnedCommand(name) => {*token_remaining = rest; Some(TexToken::ControlSequence(name))},
            CommandToken::ArgSlot(i)         => 
                if let Some(tokens) = arg_currently_output {
                    if let Some((first_token, rest)) = tokens.split_first() {
                        arg_currently_output.replace(rest);
                        Some(first_token.clone())
                    }
                    else {
                        *arg_currently_output = None;
                        *token_remaining = rest;
                        self.next()
                    }
                }
                else {
                    *arg_currently_output = Some(args[*i].as_slice());
                    self.next()
                }
            ,
        }
    }
}



impl CustomCommand {
    /// A macro that does expands to nothing, as could be defined with `\newcommand{\emptycommand}[n]{}`.
    pub fn empty_command(name : &str, n_args : usize) -> Self {
        Self { n_args, name: name.to_string(), expansion: Vec::new() }
    }

    /// Number of arguments required for macro expansion.
    pub fn n_args(&self) -> usize {
        self.n_args
    }

    fn expand_iter<'args, 'token>(& 'token self, args : & 'args [Vec<TexToken<'token>>]) -> ExpansionIterator<'args, 'token> {
        let Self { expansion, .. } = self;
        ExpansionIterator { 
            token_remaining: expansion.as_slice(), 
            args, 
            arg_currently_output: None,
        }
    }
    // fn expand<'a, 'arg, 't>(& 'a self, args : & 'arg [Vec<TexToken<'t>>]) -> ExpansionIterator<'a, 'arg, 't> {
    //     todo!()
    // }

    /// Macro name (e.g. `mymacro` for `\newcommand{\mymacro}[2]{zfee}`)
    pub fn name(&self) -> &str {
        &self.name
    }

    fn parse_macro_definition_from_iter<'a, I : Iterator<Item = TexToken<'a>>>(token_iter : &mut ExpandedTokenIter<'a, I>) -> ParseResult<Self> {
        let mut token = token_iter.next_token()?;
        while let Some(TexToken::WhiteSpace) = token {
            token = token_iter.next_token()?;
        }

        match token {
            Some(TexToken::ControlSequence("newcommand")) => (),
            _ => {
                return Err(ParseError::ExpectedNewCommand);
            },
        }


        let group = token_iter.capture_group()?;
        let name = match group[..] {
            [TexToken::ControlSequence(name)] => name,
            _ => return Err(ParseError::ExpectedMacroName),
        };


        let group = token_iter.capture_optional_group()?;


        let n_args : usize = 
            if let Some(n_arg_group) = group {
                let n_args_string = tokens_as_string(n_arg_group.into_iter())?;
                str::parse::<usize>(&n_args_string).map_err(|_| ParseError::ExpectedNumber)?
            }
            else 
            { 0 }
        ;

        let group = token_iter.capture_group()?;

        let mut expansion = Vec::with_capacity(group.len());
        // check if any error occurred
        for token in group {
            let command_token = CommandToken::try_from(token)?;
            if let CommandToken::ArgSlot(i) = &command_token {
                if *i >= n_args {
                    return Err(ParseError::MoreArgsThanSpecified)
                }
            }
            expansion.push(command_token);
        }


        let custom_command = CustomCommand {
            n_args,
            name: name.to_string(),
            expansion,
        };
        
        Ok(custom_command)
    }

}

/// Wraps a token iterator, expanding every command token that correspond to a macro.
pub struct ExpandedTokenIter<'a, I : Iterator<Item = TexToken<'a>>> {
    command_collection : & 'a CommandCollection,
    token_iter : I,
    /// token obtained from macro expansion
    expanded_token : Vec<TexToken<'a>>, 
}

impl<'a, I : Iterator<Item = TexToken<'a>>> Iterator for ExpandedTokenIter<'a, I> {
    type Item = TexToken<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_token().ok().flatten()
    }
}

impl<'a, I : Iterator<Item = TexToken<'a>>> ExpandedTokenIter<'a, I> {

    /// Get next token from the iterator
    pub fn next_token(&mut self) -> ParseResult<Option<TexToken<'a>>> {
        if let Some(token) = self.produce_next_token() {
            if let TexToken::ControlSequence(command) = token {
                if let Some(command) = self.command_collection.get(command) {
                    let tokens: Vec<Vec<TexToken<'a>>> = self.gather_args_of_command(command)?;
                    let token_slice : & [Vec<TexToken<'a>>] = tokens.as_slice();
                    // TODO: something not to have to do reversals
                    let mut expanded_tokens : Vec<TexToken<'a>> = command.expand_iter(token_slice).collect();
                    self.expanded_token.reserve(expanded_tokens.len());
                    while let Some(token) = expanded_tokens.pop() {
                        self.expanded_token.push(token)
                    }
                    self.next_token()
                }
                else {
                    Ok(Some(token))
                }
            }
            else {
                Ok(Some(token))
            }
        }
        else {
            Ok(None)            
        }
    }

    /// Obtain next token from the iterator and places it back on the expansion stack, so that a next call to either [`ExpandedTokenIter::next_token`] or [`ExpandedTokenIter::peek_token`] will return the same token.
    pub fn peek_token(&mut self) -> ParseResult<Option<TexToken<'a>>> {
        let token = self.next_token()?;
        if let Some(token) = token.clone() {
            self.expanded_token.push(token);
        }
        Ok(token)
    }

    /// From a regular token iterator, creates one that expands macros.
    pub fn new<'command : 'a>(command_collection: & 'command CommandCollection, token_iter: I) -> Self {
        Self { command_collection, token_iter, expanded_token: Vec::new() }
    }


    fn produce_next_token(&mut self) -> Option<TexToken<'a>> {
        Option::or_else(
            self.expanded_token.pop(),
            || self.token_iter.next(),
        )
    }

    fn gather_args_of_command(&mut self, command : &CustomCommand) -> ParseResult<Vec<Vec<TexToken<'a>>>> {
        let n_args = command.n_args();
        let mut args : Vec<Vec<TexToken>> = Vec::with_capacity(n_args);
        for i in 0 .. n_args {
            let arg = self
                .capture_group()
                .map_err(|e| match e {
                    ParseError::ExpectedToken => ParseError::MissingArgForMacro { expected: n_args, got: i },
                    _ => e,
                })?;
            args.push(arg);
        }
        Ok(args)
    }

    /// Returns a sequence of token corresponding to the next group in the input.
    pub fn capture_group(&mut self) -> ParseResult<Vec<TexToken<'a>>> {
        let mut arg = Vec::with_capacity(1);
        let mut token = self.next_token()?
            .ok_or_else(|| ParseError::ExpectedToken)?;
        while let TexToken::WhiteSpace = token {
            token = self.next_token()?
                .ok_or_else(|| ParseError::ExpectedToken)?;
        }
        match token {
            TexToken::BeginGroup => {
                let mut n_open_paren : u32 = 1;
                while n_open_paren != 0 {
                    let token = self.next_token()?
                        .ok_or(ParseError::UnmatchedBrackets)?;
                    if let TexToken::BeginGroup = token {
                        n_open_paren += 1;
                    }
                    else if let TexToken::EndGroup = token {
                        n_open_paren -= 1;
                    }

                    arg.push(token);
                }
                arg.pop(); // the last bracket shouldn't be added
            },
            TexToken::ControlSequence(command_name) => {
                arg.push(token);
                let n_args = PrimitiveControlSequence::n_args(command_name).unwrap_or(0);
                for _ in 0 .. n_args {
                    arg.push(TexToken::BeginGroup);
                    for token in self.capture_group()? {
                        arg.push(token);
                    }
                    arg.push(TexToken::EndGroup);
                }
            },
            TexToken::Superscript | TexToken::Subscript => {
                arg.push(token);
                arg.push(TexToken::BeginGroup);
                for token in self.capture_group()? {
                    arg.push(token);
                }
                arg.push(TexToken::EndGroup);
            }
            token => {
               arg.push(token);
            }
        }
        Ok(arg)
    }

    /// Captures a group enclosed in square brackets, if there is one following.  
    /// Does not move parser forward otherwise.
    pub fn capture_optional_group(&mut self) -> ParseResult<Option<Vec<TexToken<'a>>>> {
        let mut token = self.peek_token()?;
        while let Some(TexToken::WhiteSpace) = token {
            self.next_token()?;
            token = self.peek_token()?;
        }

        // TODO: maybe a dedicated token type for the square bracket
        // must square bracket be matched when capturing a group ?
        if let Some(TexToken::Char('[')) = token 
        { 
            self.next_token()?;
        }
        else {
            return Ok(None);
        }

        let mut to_return = Vec::new();

        let mut brackets : u32 = 1;
        while brackets != 0 {
            let token = self.next_token()?
                .ok_or(ParseError::UnmatchedBrackets)?;
            if let TexToken::Char('[') = token {
                brackets += 1;
            }
            else if let TexToken::Char(']') = token {
                brackets -= 1;
            }

            to_return.push(token);
        }
        to_return.pop(); // the last bracket shouldn't be added

        Ok(Some(to_return))
    }

}
