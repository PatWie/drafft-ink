mod common;

pub use common::{Symbol, TexSymbolType::{self, *}};
pub use common::OPERATOR_LIMITS;

/// List of symbols  
/// (GUARANTEE: the command's names are listed 'alphabetically', i.e. by byte order, to allow binary search)
pub const SYMBOLS: &'static [Symbol] = &include!(concat!(env!("OUT_DIR"), "/symbols.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_name_in_symbol_are_ordered_and_all_different() {
        for symbols in SYMBOLS.windows(2) {
            let first  = &symbols[0];
            let second = &symbols[1];

            assert!(first.name < second.name, "{} and {}", first.name, second.name);
        }
    }
}


pub const MATH_ALPHANUMERIC_TABLE_RESERVED_REPLACEMENTS: &[(u32, u32)] = &include!(concat!(env!("OUT_DIR"), "/math_alphanumeric_table_reserved_replacements.rs"));

pub fn is_italic(codepoint : char) -> bool {
    let mut codepoint = u32::from(codepoint);
    for (original, replacement) in MATH_ALPHANUMERIC_TABLE_RESERVED_REPLACEMENTS.iter() {
        if codepoint == *replacement {
            codepoint = *original;
            break;
        }
    }

    match codepoint {
        0x1d434 ..=0x1d503 => true, // From '𝐴' MATHEMATICAL CAPITAL A to '𝔃' MATHEMATICAL BOLD SCRIPT SMALL Z
        0x1d608 ..=0x1d66f => true, // From MATHEMATICAL SANS-SERIF ITALIC CAPITAL A to MATHEMATICAL SANS-SERIF ITALIC CAPITAL Z
        0x1d6e2 ..=0x1d755 => true, // From MATHEMATICAL ITALIC CAPITAL ALPHA to MATHEMATICAL SANS-SERIF ITALIC PI SYMBOL
        0x1d790 ..=0x1d7c9 => true, // From MATHEMATICAL SANS-SERIF BOLD ITALIC CAPITAL ALPHA to MATHEMATICAL SANS-SERIF BOLD ITALIC PI SYMBOL
        _ => false
    }
}