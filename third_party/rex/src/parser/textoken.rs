//! This module defines TeX tokens, an intermediate object that characters are processed into, which is what the real parser processes.


#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumberOfPrimes {
    Simple, Double, Triple
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TexToken<'a> {
    Char(char),
    ControlSequence(& 'a str),
    Superscript,
    Subscript,
    /// A '&' character
    Alignment,
    /// A '~' character for non-breakable space
    Tilde,
    WhiteSpace,
    /// In macro definitions, an arguments slot of the form (#12)
    Argument(usize),
    BeginGroup,
    EndGroup,
    Prime(NumberOfPrimes),
}




pub struct TokenIterator<'a> {
    input_processor : InputProcessor<'a>,
}

impl<'a> TokenIterator<'a> {
    pub fn new(string : & 'a str) -> Self {
        Self { input_processor: InputProcessor::new(string) }
    }

    pub fn input_processor_mut(&mut self) -> &mut InputProcessor<'a> {
        &mut self.input_processor
    }
}


impl<'a> Iterator for TokenIterator<'a> {
    type Item = TexToken<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let Self { input_processor } = self;

        let (first_char, rest) = split_first_char((*input_processor).stream)?;

        match first_char {
            _ if first_char.is_ascii_whitespace() => {
                // if character is a space, eat all subsequent spaces
                // we don't distinguish between various spaces.
                // TODO: think whether in the restricted domain we look at, we should parse spaces at all.
                input_processor.skip_whitespace();
                Some(TexToken::WhiteSpace)
            },
            '\\' => {
                let beginning_control_seq = rest;
                if let Some((control_seq_first_char, rest)) = split_first_char(beginning_control_seq) {
                    // there is a character after '\'

                    let control_sequence_name : &str;
                    if control_seq_first_char.is_ascii_alphabetic() {
                        // the control sequence starts with an alphanumeric character ; 
                        // we take this character and all subsequent alphanumeric chars to be the control sequences name
                        let index = 
                            beginning_control_seq.find(|c : char| !c.is_ascii_alphabetic()) // either there is a non-alphanumeric character following
                            .unwrap_or_else(|| beginning_control_seq.len()); // or the rest of the string is alphanumeric
                        input_processor.stream = &beginning_control_seq[index ..];
                        control_sequence_name = &beginning_control_seq[.. index];
                    }
                    else {
                        // the control sequence does not start with an alphanumeric character
                        // that character and that character only is the name of the control sequence.
                        input_processor.stream = rest;
                        control_sequence_name = &beginning_control_seq[.. control_seq_first_char.len_utf8()];
                    }
                    // Either way, skip whitespaces following the control sequence name
                    // and return name
                    input_processor.skip_whitespace();
                    Some(TexToken::ControlSequence(control_sequence_name))
                }
                else {
                    // '\' char is just before end of string
                    input_processor.stream = rest;
                    Some(TexToken::ControlSequence(&input_processor.stream[0 .. 0]))
                }
            },
            '&' => {
                input_processor.stream = rest;
                Some(TexToken::Alignment)
            }
            '~' => {
                input_processor.stream = rest;
                Some(TexToken::Tilde)
            }
            '\'' => {
                let number_of_primes = if let Some(rest) = rest.strip_prefix("''") {
                    input_processor.stream = rest;
                    NumberOfPrimes::Triple
                }
                else if let Some(rest) = rest.strip_prefix("'") {
                    input_processor.stream = rest;
                    NumberOfPrimes::Double
                }
                else {
                    input_processor.stream = rest;
                    NumberOfPrimes::Simple
                };
                Some(TexToken::Prime(number_of_primes))
            }
            '^' => {
                input_processor.stream = rest;
                Some(TexToken::Superscript)                
            }
            '_' => {
                input_processor.stream = rest;
                Some(TexToken::Subscript)                
            }
            '{' => {
                input_processor.stream = rest;
                Some(TexToken::BeginGroup)                
            }
            '}' => {
                input_processor.stream = rest;
                Some(TexToken::EndGroup)                
            }
            '#' => {
                if let Some((rest, arg_index)) = split_string_integer(rest) {
                    input_processor.stream = rest;
                    Some(TexToken::Argument(arg_index))
                }
                else {
                    input_processor.stream = rest;
                    Some(TexToken::Char('#'))
                }
            }
            c => {
                // a plain old character
                input_processor.stream = rest;
                Some(TexToken::Char(c))
            }
        }
    }
}

fn split_string_integer(stream: &str) -> Option<(&str, usize)> {
    let (number, _) = stream.split_once(|c : char| !c.is_ascii_digit()).unwrap_or((stream, stream));
    let arg_index = str::parse::<usize>(number).ok()?;
    Some((&stream[number.len() ..], arg_index))
}

pub struct InputProcessor<'a> {
    stream : & 'a str,
}

impl<'a> InputProcessor<'a> {
    pub fn new(stream: & 'a str) -> Self { Self { stream } }

    pub fn skip_whitespace(&mut self) {
        if let Some(i) = self.stream.find(|c : char| !c.is_ascii_whitespace()) {
            self.stream = &self.stream[i ..];
        }
        else {
            self.stream = &self.stream[0 .. 0];
        }
    }

    pub fn token_iter(self) -> TokenIterator<'a> {
        TokenIterator { input_processor: self }
    }
}


fn split_first_char<'a>(string : & 'a str) -> Option<(char, & 'a str)> {
    let mut chars_iter = string.chars();
    let character = chars_iter.next()?;
    Some((character, chars_iter.as_str()))
}
