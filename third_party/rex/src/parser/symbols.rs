//! Mathematical symbols from TeX commands.

use unicode_math::{SYMBOLS, TexSymbolType};

/// A LateX symbol is simply a Unicode symbol and a certain category
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// The Unicode symbol
    pub codepoint: char,
    /// Category of the symbol (e.g. open delimiters, alphanumeric, etc.)
    pub atom_type: TexSymbolType
}
impl Symbol {
    /// Given a LateX name (e.g. "alpha"), returns the corresponding symbol
    pub fn from_name(name: &str) -> Option<Self> {
        others(name).or_else(|| symbol(name))
    }

    const NULL_DELIMITER : char = '.';

    /// Checks if symbol is an open delimiter
    pub fn is_open_delimiter(&self) -> bool {
        self.atom_type == TexSymbolType::Open || self.atom_type == TexSymbolType::Fence  || self.codepoint == Self::NULL_DELIMITER
    }

    /// Checks if symbol is an closing delimiter
    pub fn is_close_delimiter(&self) -> bool {
        self.atom_type == TexSymbolType::Close || self.atom_type == TexSymbolType::Fence  || self.codepoint == Self::NULL_DELIMITER
    }

    /// Checks if symbol is an closing delimiter
    pub fn is_middle_delimiter(&self) -> bool {
        self.atom_type == TexSymbolType::Fence || self.codepoint == Self::NULL_DELIMITER
    }
}

fn symbol(name: &str) -> Option<Symbol> {
    SYMBOLS.binary_search_by_key(&name, |symbol| &symbol.name).map(|symbol_index| {
        Symbol {
            codepoint: SYMBOLS[symbol_index].codepoint,
            atom_type: SYMBOLS[symbol_index].atom_type
        }
    }).ok()
}

fn others(name: &str) -> Option<Symbol> {
    let sym = match name {
        // Additional commands from TeX
        "Alpha" => Symbol { codepoint: '\u{391}', atom_type: TexSymbolType::Alpha }, // 913
        "Beta" => Symbol { codepoint: '\u{392}', atom_type: TexSymbolType::Alpha }, // 914
        "Gamma" => Symbol { codepoint: '\u{393}', atom_type: TexSymbolType::Alpha }, // 915
        "Delta" => Symbol { codepoint: '\u{394}', atom_type: TexSymbolType::Alpha }, // 916
        "Epsilon" => Symbol { codepoint: '\u{395}', atom_type: TexSymbolType::Alpha }, // 917
        "Zeta" => Symbol { codepoint: '\u{396}', atom_type: TexSymbolType::Alpha }, // 918
        "Eta" => Symbol { codepoint: '\u{397}', atom_type: TexSymbolType::Alpha }, // 919
        "Theta" => Symbol { codepoint: '\u{398}', atom_type: TexSymbolType::Alpha }, // 920
        "Iota" => Symbol { codepoint: '\u{399}', atom_type: TexSymbolType::Alpha }, // 921
        "Kappa" => Symbol { codepoint: '\u{39A}', atom_type: TexSymbolType::Alpha }, // 922
        "Lambda" => Symbol { codepoint: '\u{39B}', atom_type: TexSymbolType::Alpha }, // 923
        "Mu" => Symbol { codepoint: '\u{39C}', atom_type: TexSymbolType::Alpha }, // 924
        "Nu" => Symbol { codepoint: '\u{39D}', atom_type: TexSymbolType::Alpha }, // 925
        "Xi" => Symbol { codepoint: '\u{39E}', atom_type: TexSymbolType::Alpha }, // 926
        "Omicron" => Symbol { codepoint: '\u{39F}', atom_type: TexSymbolType::Alpha }, // 927
        "Pi" => Symbol { codepoint: '\u{3A0}', atom_type: TexSymbolType::Alpha }, // 928
        "Rho" => Symbol { codepoint: '\u{3A1}', atom_type: TexSymbolType::Alpha }, // 929
        "Sigma" => Symbol { codepoint: '\u{3A3}', atom_type: TexSymbolType::Alpha }, // 931
        "Tau" => Symbol { codepoint: '\u{3A4}', atom_type: TexSymbolType::Alpha }, // 932
        "Upsilon" => Symbol { codepoint: '\u{3A5}', atom_type: TexSymbolType::Alpha }, // 933
        "Phi" => Symbol { codepoint: '\u{3A6}', atom_type: TexSymbolType::Alpha }, // 934
        "Chi" => Symbol { codepoint: '\u{3A7}', atom_type: TexSymbolType::Alpha }, // 935
        "Psi" => Symbol { codepoint: '\u{3A8}', atom_type: TexSymbolType::Alpha }, // 936
        "Omega" => Symbol { codepoint: '\u{3A9}', atom_type: TexSymbolType::Alpha }, // 937
        "alpha" => Symbol { codepoint: '\u{3B1}', atom_type: TexSymbolType::Alpha }, // 945
        "beta" => Symbol { codepoint: '\u{3B2}', atom_type: TexSymbolType::Alpha }, // 946
        "gamma" => Symbol { codepoint: '\u{3B3}', atom_type: TexSymbolType::Alpha }, // 947
        "delta" => Symbol { codepoint: '\u{3B4}', atom_type: TexSymbolType::Alpha }, // 948
        "epsilon" => Symbol { codepoint: '\u{3B5}', atom_type: TexSymbolType::Alpha }, // 949
        "zeta" => Symbol { codepoint: '\u{3B6}', atom_type: TexSymbolType::Alpha }, // 950
        "eta" => Symbol { codepoint: '\u{3B7}', atom_type: TexSymbolType::Alpha }, // 951
        "theta" => Symbol { codepoint: '\u{3B8}', atom_type: TexSymbolType::Alpha }, // 952
        "iota" => Symbol { codepoint: '\u{3B9}', atom_type: TexSymbolType::Alpha }, // 953
        "kappa" => Symbol { codepoint: '\u{3BA}', atom_type: TexSymbolType::Alpha }, // 954
        "lambda" => Symbol { codepoint: '\u{3BB}', atom_type: TexSymbolType::Alpha }, // 955
        "mu" => Symbol { codepoint: '\u{3BC}', atom_type: TexSymbolType::Alpha }, // 956
        "nu" => Symbol { codepoint: '\u{3BD}', atom_type: TexSymbolType::Alpha }, // 957
        "xi" => Symbol { codepoint: '\u{3BE}', atom_type: TexSymbolType::Alpha }, // 958
        "omicron" => Symbol { codepoint: '\u{3BF}', atom_type: TexSymbolType::Alpha }, // 959
        "pi" => Symbol { codepoint: '\u{3C0}', atom_type: TexSymbolType::Alpha }, // 960
        "rho" => Symbol { codepoint: '\u{3C1}', atom_type: TexSymbolType::Alpha }, // 961
        "sigma" => Symbol { codepoint: '\u{3C3}', atom_type: TexSymbolType::Alpha }, // 963
        "tau" => Symbol { codepoint: '\u{3C4}', atom_type: TexSymbolType::Alpha }, // 964
        "upsilon" => Symbol { codepoint: '\u{3C5}', atom_type: TexSymbolType::Alpha }, // 965
        "phi" => Symbol { codepoint: '\u{3C6}', atom_type: TexSymbolType::Alpha }, // 966
        "chi" => Symbol { codepoint: '\u{3C7}', atom_type: TexSymbolType::Alpha }, // 967
        "psi" => Symbol { codepoint: '\u{3C8}', atom_type: TexSymbolType::Alpha }, // 968
        "omega" => Symbol { codepoint: '\u{3C9}', atom_type: TexSymbolType::Alpha }, // 969

        "varphi" => Symbol { codepoint: '\u{3C6}', atom_type: TexSymbolType::Alpha }, // codepoint: '\u{3C6}', curly or open small phi, greek
        "varsigma" => Symbol { codepoint: '\u{3C2}', atom_type: TexSymbolType::Alpha }, // codepoint: '\u{3C2}', terminal sigma, greek
        "varbeta" => Symbol { codepoint: '\u{3D0}', atom_type: TexSymbolType::Alpha }, // codepoint: '\u{3D0}', rounded small beta, greek
        "vartheta" => Symbol { codepoint: '\u{3D1}', atom_type: TexSymbolType::Alpha }, // codepoint: '\u{3D1}', /vartheta - curly or open theta
        "varpi" => Symbol { codepoint: '\u{3D6}', atom_type: TexSymbolType::Alpha }, // codepoint: '\u{3D6}', rounded small pi (pomega), greek
        "varkappa" => Symbol { codepoint: '\u{3F0}', atom_type: TexSymbolType::Alpha }, // codepoint: '\u{3F0}', rounded small kappa, greek
        "varrho" => Symbol { codepoint: '\u{3F1}', atom_type: TexSymbolType::Alpha }, // codepoint: '\u{3F1}', rounded small rho, greek
        "varTheta" => Symbol { codepoint: '\u{3F4}', atom_type: TexSymbolType::Alpha }, // codepoint: '\u{3F4}', greek capital theta symbol
        "varepsilon" => Symbol { codepoint: '\u{3F5}', atom_type: TexSymbolType::Alpha }, // codepoint: '\u{3F5}', greek lunate epsilon symbol
        "to" => Symbol { codepoint: '\u{2192}', atom_type: TexSymbolType::Relation }, // codepoint: '\u{2192}', /rightarrow /to a: rightward arrow

        // Symbols from amsmath & stmaryrd
        

        // Symbol escape shim
        "{" => Symbol { codepoint: '\u{7B}', atom_type: TexSymbolType::Open }, // 123
        "}" => Symbol { codepoint: '\u{7D}', atom_type: TexSymbolType::Close }, // 125
        "%" => Symbol { codepoint: '\u{25}', atom_type: TexSymbolType::Ordinary }, // 37
        "&" => Symbol { codepoint: '\u{26}', atom_type: TexSymbolType::Ordinary }, // 38
        "$" => Symbol { codepoint: '\u{24}', atom_type: TexSymbolType::Ordinary }, // 36
        "#" => Symbol { codepoint: '\u{23}', atom_type: TexSymbolType::Ordinary }, // 35

        // Accents shim
        "`" => Symbol { codepoint: '\u{300}', atom_type: TexSymbolType::Accent }, // 768
        "'" => Symbol { codepoint: '\u{301}', atom_type: TexSymbolType::Accent }, // 769
        "^" => Symbol { codepoint: '\u{302}', atom_type: TexSymbolType::Accent }, // 770
        "\"" => Symbol { codepoint: '\u{308}', atom_type: TexSymbolType::Accent }, // 776
        "~" => Symbol { codepoint: '\u{303}', atom_type: TexSymbolType::Accent }, // 771
        "." => Symbol { codepoint: '\u{307}', atom_type: TexSymbolType::Accent }, // 775 

        // Binary operators shim
        "circ" => Symbol { codepoint: '\u{2218}', atom_type: TexSymbolType::Binary }, // 8728
        "bullet" => Symbol { codepoint: '\u{2219}', atom_type: TexSymbolType::Binary }, // 8729
        "diamond" => Symbol { codepoint: '\u{22C4}', atom_type: TexSymbolType::Binary }, // 8900

        // dots shim
        // TODO: offer replacement if the font does not support these symbols
        "cdots" => Symbol { codepoint: '\u{22EF}', atom_type: TexSymbolType::Ordinary }, // 8943
        "dots"  => Symbol { codepoint: '\u{22EF}', atom_type: TexSymbolType::Ordinary }, // 8943
        "ldots" => Symbol { codepoint: '\u{2026}', atom_type: TexSymbolType::Ordinary }, // 8230

        // Misc symbols shim
        "|" => Symbol { codepoint: '\u{2016}', atom_type: TexSymbolType::Fence }, // 8214
        
        _ => return None
    };
    Some(sym)
}
