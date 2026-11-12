use regex::Regex;
use std::path::PathBuf;
use std::{env, fs};
use std::collections::BTreeMap;
use std::io::Write;

use common::{Symbol, TexSymbolType, OPERATOR_LIMITS};
use parser::lucr_math::{CrossRef, CrossRefCategory};

mod parser;
mod common;




const SUPPLEMENTAL_SYMBOLS : &[Symbol<'static>] = &[
    Symbol { codepoint: '\u{003C}', name: "le",       atom_type : TexSymbolType::Relation,    description: "less-than sign" },
    Symbol { codepoint: '\u{003E}', name: "ge",       atom_type : TexSymbolType::Relation,    description: "greater-than sign" },
    Symbol { codepoint: '\u{2260}', name: "neq",      atom_type : TexSymbolType::Relation,    description: "not equal to" },
    Symbol { codepoint: '\u{2016}', name: "lVert",    atom_type : TexSymbolType::Open,        description: "double vertical lign" }, // TODO: is this right?
    Symbol { codepoint: '\u{2016}', name: "rVert",    atom_type : TexSymbolType::Close,       description: "double vertical lign" }, // TODO: is this right?
    Symbol { codepoint: '\u{003A}', name: "colon",    atom_type : TexSymbolType::Punctuation, description: "colon" },
    Symbol { codepoint: '\u{2205}', name: "emptyset", atom_type : TexSymbolType::Ordinary,    description: "circle, dash" }, 
    Symbol { codepoint: '\u{210F}', name: "hbar",     atom_type : TexSymbolType::Alpha,       description: "Planck's constant over 2pi" },
    Symbol { codepoint: '\u{2026}', name: "hdots",    atom_type : TexSymbolType::Inner,       description: "horizontal ellipsis" },
    Symbol { codepoint: '\u{00B6}', name: "P",        atom_type : TexSymbolType::Ordinary,    description: "Pilcrow sign" },
    Symbol { codepoint: '\u{00B6}', name: "gets",     atom_type : TexSymbolType::Relation,    description: "leftwards arrow" },

];

const GREEK: &[(&str, u32)] = &[
    ("Alpha",   0x391),
    ("Beta",    0x392),
    ("Gamma",   0x393),
    ("Delta",   0x394),
    ("Epsilon", 0x395),
    ("Zeta",    0x396),
    ("Eta",     0x397),
    ("Theta",   0x398),
    ("Iota",    0x399),
    ("Kappa",   0x39A),
    ("Lambda",  0x39B),
    ("Mu",      0x39C),
    ("Nu",      0x39D),
    ("Xi",      0x39E),
    ("Omicron", 0x39F),
    ("Pi",      0x3A0),
    ("Rho",     0x3A1),

    ("Sigma",   0x3A3),
    ("Tau",     0x3A4),
    ("Upsilon", 0x3A5),
    ("Phi",     0x3A6),
    ("Chi",     0x3A7),
    ("Psi",     0x3A8),
    ("Omega",   0x3A9),

    ("alpha",   0x3B1),
    ("beta",    0x3B2),
    ("gamma",   0x3B3),
    ("delta",   0x3B4),
    ("epsilon", 0x3B5),
    ("zeta",    0x3B6),
    ("eta",     0x3B7),
    ("theta",   0x3B8),
    ("iota",    0x3B9),
    ("kappa",   0x3BA),
    ("lambda",  0x3BB),
    ("mu",      0x3BC),
    ("nu",      0x3BD),
    ("xi",      0x3BE),
    ("omicron", 0x3BF),
    ("pi",      0x3C0),
    ("rho",     0x3C1),

    ("sigma",   0x3C3),
    ("tau",     0x3C4),
    ("upsilon", 0x3C5),
    ("phi",     0x3C6),
    ("chi",     0x3C7),
    ("psi",     0x3C8),
    ("omega",   0x3C9),
];


fn main() {
    println!("cargo:rerun-if-changed=src/build.rs");
    build_symbol_table();
    build_alphanumeric_table_reserved_replacements();
}

fn build_alphanumeric_table_reserved_replacements() {
    use std::fmt::Write;
    println!("cargo:rerun-if-changed=resources/math_alphanumeric_list.html");

    let path = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("resources").join("math_alphanumeric_list.html");
    let source = String::from_utf8(fs::read(&path).unwrap()).unwrap();
    let mut out = String::new();

    writeln!(out, "[").unwrap();
    let re = Regex::new(r#"<tr><td><code><a name="[0-9A-F]+">([0-9A-F]+)</a></code></td><td class="c">&nbsp;█&nbsp;</td><td colspan="2"><span class="name">&lt;Reserved&gt;</span></td></tr>\n<tr><td>&nbsp;</td><td class="char">&nbsp;</td><td class="c">→</td><td><code>([0-9A-F]+)</code>"#).unwrap();
    // let re = Regex::new(r#"<tr><td><code><a name="[0-9A-F]+">([0-9A-F]+)<\/a><\/code><\/td><td class="c">&nbsp;█&nbsp;<\/td><td colspan="2"><span class="name">&lt;Reserved&gt;<\/span><\/td><\/tr>\n<tr><td>&nbsp;<\/td><td class="char">&nbsp;<\/td><td class="c">→<\/td><td><code>([0-9A-F]+)<\/code>"#).unwrap();
    let mut n_replacements = 0;
    for capture in re.captures_iter(&source) {
        writeln!(out,
            r"(0x{}, 0x{}),",
            &capture[1], &capture[2],
        ).unwrap();
        n_replacements += 1;
    }

    // Sanity check: there should be as many replacements as the word "Reserved" appears.
    let re = Regex::new(r#"Reserved"#).unwrap();
    assert_eq!(n_replacements, re.captures_iter(&source).count());
    writeln!(out, "]").unwrap();

    let out_path = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("math_alphanumeric_table_reserved_replacements.rs");
    fs::write(out_path, out.as_bytes()).unwrap();
}

fn build_symbol_table() {
    println!("cargo:rerun-if-changed=resources/unimathsymbols.txt");
    println!("cargo:rerun-if-changed=resources/unicode-math-table.tex");


    let path = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("resources").join("unimathsymbols.txt");
    let bytes = std::fs::read(path).unwrap();
    let input = std::str::from_utf8(&bytes).unwrap();


    let (_, lines) = parser::lucr_math::parse_file(input).unwrap();

    // -- Compiles LUCR data, taknig care of resolving any conflict between two commands that don't generate the same character
    let mut latex_to_unicode_map: BTreeMap<&str, Symbol> = create_latex_to_unicode_map(&lines);



    // -- Load unicode-math date: use to complete some missing commands plus giving more explicit TeX symbol type information
    // For instance, LUCR does not differentiate between wide and not wide accents
    let path = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("resources").join("unicode-math-table.tex");
    let bytes = std::fs::read(path).unwrap();
    let input = std::str::from_utf8(&bytes).unwrap();


    let (_, lines) = parser::unicode_math::parse_file(input).unwrap();

    let unimaths_symbols = lines
        .into_iter()
        .map(|parser::unicode_math::Line { codepoint, tex_symbol_type, command_name, description }| {
            Symbol {
                codepoint: char::from_u32(codepoint).unwrap(),
                name: command_name,
                description,
                atom_type: tex_symbol_type,
            }
        })
    ;


    // -- Merging the two data sets
    for symbol in unimaths_symbols {
        latex_to_unicode_map
            .entry(&symbol.name)
            .and_modify(|old_symbol| {
                if old_symbol.codepoint == symbol.codepoint {
                    match (&mut old_symbol.atom_type, symbol.atom_type) {
                        (TexSymbolType::Radical, _) => (), // unicode-math does not know radical
                        (s1, s2) if *s1 != s2 => {
                            eprintln!(
                                "For {}, replaced {:?} with {:?}", 
                                symbol.name,
                                *s1, s2
                            );
                            *s1 = s2;
                        }, 
                        _ => (),
                    }
                }
            })
            .or_insert_with(|| {
                eprintln!("Inserted {} from unicode-math", symbol.name);
                symbol
            })
        ;
    }


    // -- Third, we insert miscellaneous commands that don't appear to be part of our databases
    for symbol in SUPPLEMENTAL_SYMBOLS {
        latex_to_unicode_map
            .entry(&symbol.name)
            .or_insert_with(|| {
                eprintln!("Inserted {} from supplemental symbol", symbol.name);
                symbol.clone()
            })
        ;
    }



    // -- Outputting the result
    let out_path = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("symbols.rs");

    let file = std::fs::File::create(out_path).expect("Couldn't write to file:");
    let mut out = std::io::BufWriter::new(file);
    writeln!(out, "[").unwrap();
    // Because we work with BTreeMap
    for symbol in latex_to_unicode_map.into_values() {
        write!(out, "{:?},\n", symbol).unwrap();
    }
    writeln!(out, "]").unwrap();
    out.flush().unwrap();
}




const PREFERRED_PROVIDERS : &[&str] = &["", "unicode-math",  "stmaryrd", "fourier"];

struct ConflictResolutionData<'a> {
    providers   : Vec<& 'a str>,
    conflicters : Vec<& 'a str>,
}

fn create_latex_to_unicode_map<'l, 'a>(lines: & 'l [parser::lucr_math::Line<'a>]) -> BTreeMap<& 'a str, Symbol<'a>> {
    let mut to_return = BTreeMap::<&str, (Symbol, ConflictResolutionData)>::new();
    for line in lines {
        let parser::lucr_math::Line { 
            codepoint, 
            latex, 
            unicode_math, 
            tex_math_category, 
            providers, 
            conflicts, 
            crossrefs, 
            description,
            ..
        } = line;


        if let Some(new_symbol) = try_create_symbol(*codepoint, latex, description, tex_math_category.clone()) {
            insert_into_map(&mut to_return, providers, new_symbol, conflicts);
        }

        if let Some(new_symbol) = try_create_symbol(*codepoint, unicode_math, description, tex_math_category.clone()) {
            insert_into_map(&mut to_return, providers, new_symbol, conflicts);
        }




        for CrossRef { category, command, providers, conflicters } in crossrefs {
            if let CrossRefCategory::Alias = category {
                if let Some(new_symbol) = try_create_symbol(*codepoint, command, description, tex_math_category.clone()) {
                    insert_into_map(&mut to_return, providers, new_symbol, conflicters);
                }
            }
        }

    }

    to_return.into_iter().map(|(key, (symbol, _))| (key, symbol)).collect()
}

fn insert_into_map<'a>(to_return: &mut BTreeMap<& 'a str, (Symbol<'a>, ConflictResolutionData<'a>)>, providers: &[& 'a str], new_symbol: Symbol<'a>, conflicts: &[& 'a str]) {
    if let Some((symbol, ConflictResolutionData { providers : old_providers, conflicters : _old_conflicters })) = to_return.get_mut(new_symbol.name) {
        if *symbol == new_symbol {
            return;
        }
        eprintln!("================= Conflict!");
        eprintln!("New:\n {:#?}", new_symbol);
        eprintln!("{:?}", providers);

        eprintln!("Old:\n {:#?}", symbol);
        eprintln!("{:?}", old_providers);

        // Name conflict resolution principles
        //   - if the most preferred provider of the new command definition is more preferred to that of the old command definition, we prefer this one
        //   - if that results in a tie, the smallest code point is preferred
        let index_best_provider_old_command = find_best_provider(old_providers);
        let index_best_provider_new_command = find_best_provider(providers);

        let better_provider = index_best_provider_new_command < index_best_provider_old_command;
        let smaller_codepoint = index_best_provider_new_command == index_best_provider_old_command && new_symbol.codepoint < symbol.codepoint;
        if better_provider || smaller_codepoint
        {
            eprintln!("Chose new b/c {}", if better_provider { "better_provider" } else { "smaller codepoint" });
            *symbol = new_symbol;
        }
        else {
            eprintln!("Kept old");
        }
    }
    else {
        // No conflict, we may insert the symbol safely
        to_return.insert(new_symbol.name, (new_symbol, ConflictResolutionData { providers: providers.to_vec(), conflicters: conflicts.to_vec() }));
    }
}

fn find_best_provider(providers: &[&str]) -> usize {
    providers
        .iter()
        .map(|&provider| PREFERRED_PROVIDERS.iter().position(|&provider2| provider == provider2).unwrap_or(PREFERRED_PROVIDERS.len()))
        .min()
        .unwrap_or(0)
}

fn try_create_symbol<'a>(codepoint: u32, latex: & 'a str, description: & 'a str, tex_math_category: Option<TexSymbolType>) -> Option<Symbol<'a>> {
    let (_, name) = latex.split_once('\\')?;
    if name.len() != 1 && !name.chars().all(|character| character.is_ascii_alphabetic()) {
        return None;
    }
    let mut atom_type = tex_math_category?;
    // Operator limits
    if let (TexSymbolType::Operator(limits), true) = (&mut atom_type, OPERATOR_LIMITS.contains(&name)) {
        *limits = true;
    }


    Some(Symbol {
        codepoint: char::from_u32(codepoint)?,
        name,
        description,
        atom_type,
    })
}
