use nom::{bytes::complete::tag, character::complete::{anychar, hex_digit1, line_ending, none_of, space0}, combinator::{map_opt, map_res, recognize}, multi::{many0, many1, many_till}, IResult};

use crate::common::{TexSymbolType, OPERATOR_LIMITS};

#[derive(Debug)]
pub struct Line<'a> {
    pub codepoint    : u32,
    pub command_name : & 'a str,
    pub tex_symbol_type : TexSymbolType,
    pub description: & 'a str,
}

pub fn parse_file(input : &str) -> IResult<&str, Vec<Line>> {
    let (input, _) = many0(parse_comment)(input)?;
    let (input, _) = many0(line_ending)(input)?;
    nom::combinator::map(many_till(parse_line, line_ending), |(result, _)| result)(input)
}

fn parse_comment(input : &str) -> IResult<&str, ()> {
    let (input, _) = tag("%")(input)?;
    nom::combinator::map(many_till(anychar, line_ending), |_| ())(input)
}

fn parse_line(input : &str) -> IResult<&str, Line> {
    let (input, _) = tag("\\UnicodeMathSymbol{\"")(input)?;
    let (input, codepoint) = map_res(hex_digit1, |hex_digits: &str| {
        u32::from_str_radix(hex_digits, 16)
    })(input)?;
    let (input, _) = tag("}{\\")(input)?;
    let (input, command_name) = recognize(many1(none_of(" }")))(input)?;
    let (input, _) = space0(input)?;
    let (input, _) = tag("}{\\")(input)?;
    let (input, mut tex_symbol_type) = map_opt(
        recognize(many1(none_of(" }"))),
        TexSymbolType::from_str
    )(input)?;
    let (input, _) = tag("}{")(input)?;
    let (input, description) = recognize(many1(none_of("}")))(input)?;
    if let TexSymbolType::Operator(limits) = &mut tex_symbol_type {
        *limits = OPERATOR_LIMITS.contains(&command_name);
    }


    let (input, _) = many_till(anychar, line_ending)(input)?;

    Ok((input, Line { 
        codepoint,
        command_name,
        tex_symbol_type, 
        description,
    }))
}
