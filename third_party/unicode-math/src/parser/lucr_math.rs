use nom::{branch::alt, bytes::complete::{tag, take_while_m_n}, character::complete::{alpha1, space0, space1}, combinator::{eof, map_res, opt}, error::{Error, ParseError}, multi::{many0, many_till, separated_list0}, IResult};

use crate::common::TexSymbolType;



#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum UnicodeMathCharClass {
    /// N: includes all digits and symbols requiring only one form
    Normal,       
    /// A: Alphabetic
    Alphabetic,   
    /// B: Binary
    Binary,       
    /// C: Closing – usually paired with opening delimiter
    Closing,      
    /// D: Diacritic
    Diacritic,    
    /// F: Fence - unpaired delimiter (often used as opening or closing)
    Fence,        
    /// G: Glyph_Part - piece of large operator
    GlyphPart,    
    /// L: Large -n-ary or Large operator, often takes limits
    Large,        
    /// O: Opening – usually paired with closing delimiter
    Opening,      
    /// P: Punctuation
    Punctuation,  
    /// R: Relation - includes arrows
    Relation,     
    /// S: Space
    Space,        
    /// U: Unary – operators that are only unary
    Unary,        
    /// V: Vary – operators that can be unary or binary depending on context
    Vary,         
    /// X: Special – characters not covered by other classes
    Special,      
}

impl UnicodeMathCharClass {
    pub fn from_str(c: &str) -> Option<Self> {
        match c {
            "N" | "N?"=> Some(Self::Normal),
            "A" | "A?"=> Some(Self::Alphabetic),
            "B" | "B?"=> Some(Self::Binary),
            "C" | "C?"=> Some(Self::Closing),
            "D" | "D?"=> Some(Self::Diacritic),
            "F" | "F?"=> Some(Self::Fence),
            "G" | "G?"=> Some(Self::GlyphPart),
            "L" | "L?"=> Some(Self::Large),
            "O" | "O?"=> Some(Self::Opening),
            "P" | "P?"=> Some(Self::Punctuation),
            "R" | "R?"=> Some(Self::Relation),
            "S" | "S?"=> Some(Self::Space),
            "U" | "U?"=> Some(Self::Unary),
            "V" | "V?"=> Some(Self::Vary),
            "X" | "X?"=> Some(Self::Special),
            _ => None, // Return None for any character that does not match a known class
        }
    }
}

#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Line<'a> {
    pub codepoint : u32,
    // sometimes, with accent, our file does something weird, by presenting two chars
    pub character        : & 'a str,
    pub latex            : & 'a str,
    pub unicode_math     : & 'a str,
    pub unicode_math_char_class  : Option<UnicodeMathCharClass>,
    pub tex_math_category : Option<TexSymbolType>,
    pub providers : Vec<& 'a str>,
    pub conflicts : Vec<& 'a str>,
    pub crossrefs   : Vec<CrossRef<'a>>,
    pub description : & 'a str,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum CrossRefCategory {
    Alias,
    Similar,
    FalseFriend,
    TextMode
}

#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct CrossRef<'a> {
    pub category: CrossRefCategory,
    pub command:  & 'a str,
    pub providers   : Vec<& 'a str>,
    pub conflicters : Vec<& 'a str>,
}



pub fn parse_file(input : &str) -> IResult<&str, Vec<Line>> {
    let (input, _)     = many0(parse_comment)(input)?;
    let (input, (lines, _)) = many_till(parse_line, eof)(input)?;

    Ok((input, lines))
}


pub fn parse_line(input : &str) -> IResult<&str, Line> {
    let (input, codepoint) = parse_codepoint(input)?;
    let (input, _) = tag("^")(input)?;
    let (input, character)     = parse_character(input)?;
    let (input, latex)         = parse_string_till_hat(input)?;
    let (input, unicode_math)  = parse_string_till_hat(input)?;
    let (input, unicode_math_char_class)  = parse_unicode_math_char_class(input)?;
    let (input, tex_math_category)  = parse_tex_math_category(input)?;
    let (input, (providers, conflicts))  = parse_providers_and_conflicts(input)?;
    let (input, _) = tag("^")(input)?;
    let (input, cross_ref)  = separated_list0(tag(", "), parse_cross_ref)(input)?;
    let (input, _) = opt(tag(", "))(input)?;
    let (description, input) = input.split_once('\n').unwrap_or(("", ""));



    let line = Line { 
        codepoint, 
        character,
        latex,
        unicode_math,
        unicode_math_char_class,
        tex_math_category,
        providers,
        conflicts,
        description,
        crossrefs: cross_ref.into_iter().collect(),
    };

    Ok((input, line))
}

pub fn parse_cross_ref_category(input : &str) -> IResult<&str, CrossRefCategory> {
    alt((
        map_res(tag("#"), |_| Ok::<_, ()>(CrossRefCategory::Similar)),
        map_res(tag("="), |_| Ok::<_, ()>(CrossRefCategory::Alias)),
        map_res(tag("x"), |_| Ok::<_, ()>(CrossRefCategory::FalseFriend)),
        map_res(tag("t"), |_| Ok::<_, ()>(CrossRefCategory::TextMode)),
    ))(input)
}

pub fn parse_cross_ref(input : &str) -> IResult<&str, CrossRef> {
    let (input, category) = parse_cross_ref_category(input)?;
    let (input, _) = space1(input)?;
    // command stops at first , or space
    let (command, _) = input.split_once(&[' ', ',']).ok_or(nom::Err::Failure(Error::from_error_kind(input, nom::error::ErrorKind::Complete)))?;
    let input = &input[command.len() ..];
    let (input, providers_and_conflicters) = opt(|input| {
        let (input, _) = space0(input)?;
        let (input, _) = tag("(")(input)?;
        let (input, providers_and_conflicters) = parse_providers_and_conflicts(input)?;
        let (input, _) = tag(")")(input)?;
        Ok((input, providers_and_conflicters))
    })(input)?;
    let (providers, conflicters) = providers_and_conflicters.unwrap_or_default();
    

    Ok((
        input,
        CrossRef {
            category,
            command,
            providers,
            conflicters,
        }
    ))
}

pub fn parse_providers_and_conflicts(input: &str) -> IResult<&str, (Vec<&str>, Vec<&str>)> {
    let (input, providers_and_conflicters) = separated_list0(space1, parse_provider_or_conflict)(input)?;

    let providers = providers_and_conflicters.iter().cloned().filter_map(|(conflicts, package)| {
        if conflicts 
        { None }
        else 
        { Some(package) }
    }).collect();
    let conflicters = providers_and_conflicters.iter().cloned().filter_map(|(conflicts, package)| {
        if conflicts 
        { Some(package) }
        else 
        { None }
    }).collect();
    Ok((input, (providers, conflicters)))
}

pub fn parse_provider_or_conflict(input: &str) -> IResult<&str, (bool, &str)> {
    let (input, tag_conflict) = opt(tag("-"))(input)?;
    let conflict = tag_conflict.is_some();
    let (input, package) = alpha1(input)?;
    Ok((input, (conflict, package)))
}

pub fn parse_unicode_math_char_class(input: &str) -> IResult<&str, Option<UnicodeMathCharClass>> {
    let (input, result) = map_res(parse_string_till_hat, |s : &str| {
        if !s.is_empty() {
            UnicodeMathCharClass::from_str(s).ok_or("Unrecognized math char class").map(Some)
        }
        else {
            Ok(None)
        }
    })(input)?;
    Ok((input, result))
}

pub fn parse_tex_math_category(input: &str) -> IResult<&str, Option<TexSymbolType>> {
    let (input, result) = map_res(parse_string_till_hat, |s : &str| {
        if !s.is_empty() {
            TexSymbolType::from_str(s).ok_or("Unrecognized tex category").map(Some)
        }
        else {
            Ok(None)
        }
    })(input)?;
    Ok((input, result))
}


pub fn parse_string_till_hat(input: &str) -> IResult<&str, &str> {
    let (cell, rest) = input.split_once('^').ok_or(nom::Err::Error(Error::from_error_kind(input, nom::error::ErrorKind::Char)))?;
    Ok((rest, cell))
}

pub fn parse_character(input: &str) -> IResult<&str, &str> {
    if input.starts_with('^') {
        let (start, rest) = input.split_at(2 * '^'.len_utf8());
        Ok((rest, start))
    }
    else {
        parse_string_till_hat(input)
    }
}

fn from_hex(input: &str) -> Result<u32, std::num::ParseIntError> {
    u32::from_str_radix(input, 16)
}

fn is_hex_digit(c: char) -> bool {
    c.is_digit(16)
}

pub fn parse_codepoint(input: &str) -> IResult<&str, u32> {
    const N_DIGITS_CODEPOINT : usize = 5;
    map_res(
        take_while_m_n(N_DIGITS_CODEPOINT, N_DIGITS_CODEPOINT, is_hex_digit),
        from_hex
    )(input)
}


pub fn parse_comment(input : &str) -> IResult<&str, ()> {
    let (input, _) = tag("#")(input)?;
    Ok(match input.split_once('\n') {
        Some((_, rest)) => (rest, ()),
        None            => ("",   ()),
    })
}




